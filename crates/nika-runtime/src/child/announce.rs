// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The trace a child run writes, told to the `for_each:` iteration that called it as soon as
//! that trace opens (NIK-385 · spec 14 law 8, spec 17 `child`). A batch that stops
//! (`fail_fast`, the budget) drops the iterations still in flight, and their children with
//! them: no settled result names the child's trace then, yet its row still does, so the trace
//! lives as long as its root.
//!
//! The iteration listens through a slot of this thread, set for the length of each poll of its
//! future and restored after it (the runtime core keeps no tokio edge, so this is not a tokio
//! task-local). The child runner's sink speaks inside that poll, on that thread: the child run
//! is the iteration's own future, never a spawned task.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

thread_local! {
    /// The iteration being polled on this thread, while it is.
    static LISTENING: RefCell<Option<Arc<ChildAnnounce>>> = const { RefCell::new(None) };
}

/// What one iteration hears: the trace its child call opened. Only a child at `depth` speaks
/// to it; a grandchild, told from inside that child, is the child's own business.
pub(crate) struct ChildAnnounce {
    /// The depth of the child the iteration calls: its run's depth, plus one.
    depth: u32,
    /// Every trace told, oldest first, each once: one per attempt of a retried call.
    traces: Mutex<Vec<String>>,
}

impl ChildAnnounce {
    pub(crate) fn new(depth: u32) -> Arc<Self> {
        Arc::new(Self {
            depth,
            traces: Mutex::new(Vec::new()),
        })
    }

    /// The trace the iteration's last child call told, if one opened.
    pub(crate) fn trace(&self) -> Option<String> {
        self.traces
            .lock()
            .ok()
            .and_then(|traces| traces.last().cloned())
    }

    /// Every trace the iteration's child calls told, oldest first: a retried call's attempts.
    pub(crate) fn traces(&self) -> Vec<String> {
        self.traces
            .lock()
            .map(|traces| traces.clone())
            .unwrap_or_default()
    }

    /// `future`, listening on every poll.
    pub(crate) fn listen<F: Future>(self: Arc<Self>, future: F) -> Listening<F> {
        Listening {
            slot: self,
            inner: Box::pin(future),
        }
    }
}

/// A future that listens for its child's trace while it is polled.
pub(crate) struct Listening<F> {
    slot: Arc<ChildAnnounce>,
    inner: Pin<Box<F>>,
}

impl<F: Future> Future for Listening<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.get_mut();
        let slot = Arc::clone(&this.slot);
        let outer = LISTENING
            .try_with(|listening| listening.replace(Some(slot)))
            .ok()
            .flatten();
        // Restored on return and on unwind alike: an enclosing iteration hears again.
        let _restore = Restore(outer);
        this.inner.as_mut().poll(cx)
    }
}

/// Puts back the iteration that was listening before a nested poll.
struct Restore(Option<Arc<ChildAnnounce>>);

impl Drop for Restore {
    fn drop(&mut self) {
        let outer = self.0.take();
        let _ = LISTENING.try_with(|listening| *listening.borrow_mut() = outer);
    }
}

/// Tell the `for_each:` iteration that made this call which trace its child run writes, once
/// that trace is open. A [`ChildRunner`](super::ChildRunner) calls it with the
/// [`ChildCall::depth`](super::ChildCall::depth) of the call it runs; outside an iteration,
/// and for a call at another depth, it does nothing.
pub fn announce_trace(depth: u32, trace_id: &str) {
    let _ = LISTENING.try_with(|listening| {
        if let Some(slot) = listening.borrow().as_ref()
            && slot.depth == depth
            && let Ok(mut traces) = slot.traces.lock()
            && !traces.iter().any(|told| told == trace_id)
        {
            traces.push(trace_id.to_owned());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Poll `future` once, as an executor would.
    fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
        let mut future = std::pin::pin!(future);
        future
            .as_mut()
            .poll(&mut Context::from_waker(std::task::Waker::noop()))
    }

    /// The child's trace reaches the iteration polling it; a retry's later attempt is the last
    /// one, and every attempt is kept, once each.
    #[test]
    fn the_listening_iteration_hears_its_child_and_the_last_attempt_wins() {
        let slot = ChildAnnounce::new(1);
        let heard = Arc::clone(&slot).listen(async {
            announce_trace(1, "a.ndjson");
            announce_trace(1, "b.ndjson");
            announce_trace(1, "a.ndjson");
        });
        assert!(poll_once(heard).is_ready());
        assert_eq!(slot.trace().as_deref(), Some("b.ndjson"));
        assert_eq!(slot.traces(), ["a.ndjson", "b.ndjson"]);
    }

    /// A call at another depth, and a call outside any iteration, change nothing.
    #[test]
    fn another_depth_and_no_iteration_are_not_heard() {
        let slot = ChildAnnounce::new(1);
        assert!(
            poll_once(Arc::clone(&slot).listen(async { announce_trace(2, "g.ndjson") })).is_ready()
        );
        announce_trace(1, "outside.ndjson");
        assert_eq!(slot.trace(), None);
    }

    /// A child's own iteration hears its grandchild; once that poll returns, the enclosing
    /// iteration hears again, and only its own child.
    #[test]
    fn a_nested_iteration_hears_its_own_and_gives_the_slot_back() {
        let outer = ChildAnnounce::new(1);
        let inner = ChildAnnounce::new(2);
        let nested = Arc::clone(&inner);
        let heard = Arc::clone(&outer).listen(async move {
            announce_trace(1, "child.ndjson");
            nested
                .listen(async { announce_trace(2, "grandchild.ndjson") })
                .await;
            announce_trace(2, "late.ndjson");
        });
        assert!(poll_once(heard).is_ready());
        assert_eq!(outer.trace().as_deref(), Some("child.ndjson"));
        assert_eq!(inner.trace().as_deref(), Some("grandchild.ndjson"));
        announce_trace(1, "after.ndjson");
        assert_eq!(
            outer.trace().as_deref(),
            Some("child.ndjson"),
            "the slot was given back"
        );
    }
}
