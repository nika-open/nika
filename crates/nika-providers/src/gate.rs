// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! A bound on the provider calls in flight, shared by every run of one host.
//!
//! A run bounds its own fan-out (`max_parallel`, the wave width). A host that
//! runs many jobs at once needs one more bound, per provider and ACROSS its
//! runs: a provider's rate limit belongs to the account, not to a run. The
//! host implements [`ProviderCallGate`] and hands it to the registry
//! ([`crate::ProviderRegistry::with_call_gate`]). Every round-trip a resolved
//! provider sends then takes one slot before it starts and gives it back when
//! it returns, so the effective width of a `for_each` is the smaller of its
//! `max_parallel` and the slots that are free.
//!
//! The gate only bounds what is in flight. It never re-sends and owns no
//! retry (ADR-019): a call that its verb will retry has given its slot back
//! before the backoff starts, so a waiting retry holds none. A call the
//! gate refuses was never sent and is never retried. It lives beside
//! the call it bounds, like [`crate::InferenceAdmission`], and a registry
//! without one sends exactly as it did.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// One call slot on a provider. Dropping it gives the slot back.
#[must_use = "a slot dropped at once bounds nothing"]
pub struct ProviderCallSlot(Option<Box<dyn Send + Sync>>);

impl ProviderCallSlot {
    /// A slot that keeps `held` until it drops: the gate's own permit.
    pub fn holding(held: impl Send + Sync + 'static) -> Self {
        Self(Some(Box::new(held)))
    }

    /// The slot of a provider the gate does not bound.
    pub const fn unbounded() -> Self {
        Self(None)
    }
}

impl fmt::Debug for ProviderCallSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() {
            "ProviderCallSlot(held)"
        } else {
            "ProviderCallSlot(unbounded)"
        })
    }
}

/// A call that was given no slot: the host withdrew it while it waited (its
/// run was stopped). Nothing was sent to the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProviderCallRefused {
    reason: String,
}

impl ProviderCallRefused {
    /// A refusal that says why the call left the line.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    /// Why the call left the line.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for ProviderCallRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

/// The future [`ProviderCallGate::enter`] returns.
pub type ProviderCallEntry<'a> =
    Pin<Box<dyn Future<Output = Result<ProviderCallSlot, ProviderCallRefused>> + Send + 'a>>;

/// Bounds the provider calls in flight across every run that shares it.
///
/// Object-safe on purpose: one gate is shared as `Arc<dyn ProviderCallGate>`
/// by every registry of a process, whatever its http effect.
pub trait ProviderCallGate: Send + Sync + fmt::Debug {
    /// Wait for one slot on `provider`, its canonical id (`openai`,
    /// `anthropic`, `mock`). A provider the gate does not bound answers
    /// [`ProviderCallSlot::unbounded`] at once.
    ///
    /// A gate may withdraw a call that waits (the host stopped its run):
    /// the refusal sends nothing and the call fails without a retry.
    ///
    /// CANCEL SAFETY: a future dropped while it waits takes no slot and
    /// loses no place another waiter depends on. A run that is dropped
    /// while one of its calls waits simply leaves the line.
    fn enter<'a>(&'a self, provider: &'a str) -> ProviderCallEntry<'a>;
}
