// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! One cap per provider on the model calls in flight, across every job the
//! resident runs (ADR-155).
//!
//! A job bounds its own fan-out; the provider's rate limit belongs to the
//! account all jobs share. The resident holds one semaphore per capped
//! provider and hands this gate to every run it starts
//! (`RuntimeConfig::provider_gate`): each round-trip takes a slot before it
//! is sent and gives it back when it returns. A run's children share it.
//! Nothing here re-sends a call: retries stay where ADR-019 puts them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nika_providers::{ProviderCallEntry, ProviderCallGate, ProviderCallRefused, ProviderCallSlot};
use nika_types::cancel::CancelCtx;
use tokio::sync::Semaphore;

/// The most calls one provider may be capped at. Far above any account's
/// limit, and far under what one semaphore counts.
pub(crate) const MAX_PROVIDER_CONCURRENCY: usize = 65_535;

/// How often a call that waits for a slot looks at its run's stop. The
/// stop is a flag without a waker (`CancelCtx`), read the way the resident
/// reads it for the run itself.
const STOP_POLL: Duration = Duration::from_millis(25);

/// The resident's caps, one per provider its operator named.
#[derive(Debug)]
pub(crate) struct ProviderGates {
    providers: BTreeMap<String, Slots>,
}

#[derive(Debug)]
struct Slots {
    limit: usize,
    free: Arc<Semaphore>,
    waiting: AtomicUsize,
}

/// What one capped provider holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderLoad {
    pub(crate) provider: String,
    pub(crate) limit: usize,
    pub(crate) in_flight: usize,
    pub(crate) waiting: usize,
}

impl ProviderGates {
    /// One gate over `limits`: a canonical provider id and its cap.
    pub(crate) fn new(limits: &BTreeMap<String, usize>) -> Arc<Self> {
        let providers = limits
            .iter()
            .map(|(provider, limit)| {
                let limit = (*limit).min(MAX_PROVIDER_CONCURRENCY);
                let slots = Slots {
                    limit,
                    free: Arc::new(Semaphore::new(limit)),
                    waiting: AtomicUsize::new(0),
                };
                (provider.clone(), slots)
            })
            .collect();
        Arc::new(Self { providers })
    }

    /// Every capped provider, by id: its cap, the calls in flight and the
    /// calls waiting for a slot.
    pub(crate) fn load(&self) -> Vec<ProviderLoad> {
        self.providers
            .iter()
            .map(|(provider, slots)| ProviderLoad {
                provider: provider.clone(),
                limit: slots.limit,
                in_flight: slots.limit.saturating_sub(slots.free.available_permits()),
                waiting: slots.waiting.load(Ordering::Relaxed),
            })
            .collect()
    }
}

/// Counts one call in the line for as long as it waits, a cancelled wait
/// included.
struct Waiting<'a>(&'a AtomicUsize);

impl<'a> Waiting<'a> {
    fn enter(line: &'a AtomicUsize) -> Self {
        line.fetch_add(1, Ordering::Relaxed);
        Self(line)
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl ProviderCallGate for ProviderGates {
    fn enter<'a>(&'a self, provider: &'a str) -> ProviderCallEntry<'a> {
        Box::pin(async move {
            let Some(slots) = self.providers.get(provider) else {
                return Ok(ProviderCallSlot::unbounded());
            };
            let _line = Waiting::enter(&slots.waiting);
            // The semaphore is never closed, so this wait only ends with a
            // slot or with the caller dropping it.
            match Arc::clone(&slots.free).acquire_owned().await {
                Ok(permit) => Ok(ProviderCallSlot::holding(permit)),
                Err(_) => Err(ProviderCallRefused::new("the provider gate is closed")),
            }
        })
    }
}

/// One run's turn at the resident's gate: the same slots, and a wait that
/// ends when the run is stopped.
///
/// A stop is cooperative: the runtime ends a run at its next wave boundary.
/// A call that only waits for a slot would otherwise hold that boundary back
/// and be sent once a slot frees, after the stop. Withdrawn here, it sends
/// nothing, its place in line goes to the next job, and the run settles.
#[derive(Debug)]
struct RunGate {
    gate: Arc<dyn ProviderCallGate>,
    stop: CancelCtx,
}

/// `gate` as one run takes it: under the run's `stop` when it has one.
pub(crate) fn for_run(
    gate: Arc<dyn ProviderCallGate>,
    stop: Option<CancelCtx>,
) -> Arc<dyn ProviderCallGate> {
    match stop {
        Some(stop) => Arc::new(RunGate { gate, stop }),
        None => gate,
    }
}

impl ProviderCallGate for RunGate {
    fn enter<'a>(&'a self, provider: &'a str) -> ProviderCallEntry<'a> {
        Box::pin(async move {
            let stopped =
                || ProviderCallRefused::new("the run was stopped while the call waited its turn");
            if self.stop.is_cancelled() {
                return Err(stopped());
            }
            // One wait, kept across the looks at the stop: the call keeps
            // its place in line.
            let mut entry = self.gate.enter(provider);
            loop {
                tokio::select! {
                    biased;
                    slot = &mut entry => return slot,
                    () = tokio::time::sleep(STOP_POLL) => {
                        if self.stop.is_cancelled() {
                            return Err(stopped());
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod resident_tests;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn gates(limits: &[(&str, usize)]) -> Arc<ProviderGates> {
        ProviderGates::new(
            &limits
                .iter()
                .map(|(provider, limit)| ((*provider).to_owned(), *limit))
                .collect(),
        )
    }

    fn load_of(gates: &ProviderGates, provider: &str) -> (usize, usize) {
        let load = gates
            .load()
            .into_iter()
            .find(|load| load.provider == provider)
            .expect("a capped provider");
        (load.in_flight, load.waiting)
    }

    /// A capped provider admits its cap and no more; the next call waits,
    /// and enters when a slot comes back.
    #[tokio::test]
    async fn a_provider_admits_its_cap_and_the_rest_wait() {
        let gates = gates(&[("openai", 2), ("anthropic", 1)]);
        let first = gates.enter("openai").await.expect("a slot");
        let second = gates.enter("openai").await.expect("a slot");
        assert_eq!(load_of(&gates, "openai"), (2, 0));
        assert_eq!(load_of(&gates, "anthropic"), (0, 0), "one cap per provider");

        let third = {
            let gates = Arc::clone(&gates);
            tokio::spawn(async move { gates.enter("openai").await })
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while load_of(&gates, "openai").1 == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the third call waits in line");
        assert_eq!(load_of(&gates, "openai"), (2, 1));

        drop(first);
        let third = third.await.expect("join").expect("a slot");
        assert_eq!(load_of(&gates, "openai"), (2, 0), "it took the freed slot");
        drop((second, third));
        assert_eq!(load_of(&gates, "openai"), (0, 0));
    }

    /// A provider nobody capped never waits, and a wait that is cancelled
    /// leaves the line without a slot.
    #[tokio::test]
    async fn an_uncapped_provider_never_waits_and_a_cancelled_wait_leaves() {
        let gates = gates(&[("openai", 1)]);
        let _held = gates.enter("openai").await.expect("a slot");
        let _free = gates.enter("mock").await.expect("no cap, no wait");
        assert!(
            gates.load().iter().all(|load| load.provider == "openai"),
            "only capped providers are reported"
        );

        let waiting = {
            let gates = Arc::clone(&gates);
            tokio::spawn(async move { gates.enter("openai").await })
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while load_of(&gates, "openai").1 == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("it waits");
        waiting.abort();
        assert!(waiting.await.expect_err("aborted").is_cancelled());
        assert_eq!(load_of(&gates, "openai"), (1, 0), "the line is empty again");
    }

    /// A stopped run's waiting call is withdrawn: it leaves the line without
    /// a slot, and a run already stopped never joins it. A run that is not
    /// stopped keeps its place and gets its slot.
    #[tokio::test]
    async fn a_stopped_run_leaves_the_line() {
        let gates = gates(&[("openai", 1)]);
        let shared: Arc<dyn ProviderCallGate> = gates.clone();
        let held = gates.enter("openai").await.expect("a slot");

        let stop = CancelCtx::new();
        let stopped_run = for_run(Arc::clone(&shared), Some(stop.clone()));
        let other_run = for_run(Arc::clone(&shared), Some(CancelCtx::new()));
        let waiting = tokio::spawn(async move { stopped_run.enter("openai").await });
        let patient = tokio::spawn(async move { other_run.enter("openai").await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while load_of(&gates, "openai").1 < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both runs wait");

        stop.cancel();
        let refused = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("a stopped run does not keep waiting")
            .expect("join")
            .expect_err("withdrawn");
        assert_eq!(
            refused.reason(),
            "the run was stopped while the call waited its turn"
        );
        assert_eq!(
            load_of(&gates, "openai"),
            (1, 1),
            "only the other run waits"
        );

        let late = for_run(Arc::clone(&shared), Some(stop));
        assert!(
            late.enter("openai").await.is_err(),
            "a stopped run never joins"
        );
        assert_eq!(load_of(&gates, "openai"), (1, 1));

        drop(held);
        let slot = patient.await.expect("join").expect("the other run's slot");
        assert_eq!(load_of(&gates, "openai"), (1, 0));
        drop(slot);
        assert_eq!(load_of(&gates, "openai"), (0, 0));
    }
}
