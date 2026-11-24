// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2024-2026 SuperNovae Studio <contact@supernovae.studio>

//! The resident's execution queue (ADR-155).
//!
//! One lane per owner, first in first out inside a lane. The next worker goes
//! to the owner with the fewest jobs running, in turn among equals: owners
//! share the workers evenly, and an owner that arrives during another's
//! burst starts at the next free worker. Jobs that state no owner share one
//! anonymous lane. A run the resident already
//! started (a scheduled fire, durably `running` before it is queued) waits in
//! a lane of its own that is served first, so its wait is bounded by the
//! other started runs alone: that is what its observer's deadline assumes.
//!
//! A place is reserved before the durable record exists and released when the
//! admission does not land, exactly as the channel permit this replaces. The
//! queue is plain data behind one mutex; nothing waits while it holds it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::Notify;

use crate::JobId;

/// The owner whose lane a job waits in; `None` is the anonymous lane.
type Owner = Option<String>;

/// What the queue holds at most.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct QueueBounds {
    /// Places in the whole queue: reservations and waiting jobs.
    pub(super) capacity: usize,
    /// Places one owner may hold; `None` leaves the capacity alone.
    pub(super) queued_per_owner: Option<usize>,
    /// Jobs one owner may have running; `None` leaves the workers alone.
    pub(super) running_per_owner: Option<usize>,
}

/// Why a place was not given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QueueRefusal {
    /// Every place of the queue is taken.
    Full,
    /// This owner holds every place one owner may hold.
    OwnerFull,
    /// The resident is stopping.
    Closed,
}

/// How many jobs one owner has waiting and running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OwnerLoad {
    pub(super) owner: Option<String>,
    pub(super) queued: usize,
    pub(super) running: usize,
}

/// What the queue holds now, by owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct QueueLoad {
    pub(super) queued: usize,
    pub(super) running: usize,
    /// The busiest owners first, at most the number asked for.
    pub(super) owners: Vec<OwnerLoad>,
    /// Owners with a job waiting or running that `owners` leaves out.
    pub(super) owners_omitted: usize,
}

/// The bounded fair queue of one resident.
pub(super) struct FairQueue<T> {
    bounds: QueueBounds,
    inner: Mutex<Inner<T>>,
    wake: Notify,
}

struct Inner<T> {
    closed: bool,
    /// Places taken: reservations and waiting jobs, every lane.
    held: usize,
    /// Runs the resident already started, served before every lane.
    started: VecDeque<(Owner, JobId, T)>,
    lanes: BTreeMap<Owner, Lane<T>>,
    /// The owners with a waiting job, in the order they are served.
    ring: VecDeque<Owner>,
    /// Every job that waits, so a replay never queues it twice.
    waiting: BTreeSet<JobId>,
    /// Every job a worker holds. The store says `queued` until the worker
    /// has claimed it, and a replay in between must queue nothing.
    in_hand: BTreeSet<JobId>,
}

struct Lane<T> {
    waiting: VecDeque<(JobId, T)>,
    reserved: usize,
    running: usize,
}

impl<T> Default for Lane<T> {
    fn default() -> Self {
        Self {
            waiting: VecDeque::new(),
            reserved: 0,
            running: 0,
        }
    }
}

impl<T> Lane<T> {
    fn idle(&self) -> bool {
        self.waiting.is_empty() && self.reserved == 0 && self.running == 0
    }
}

impl<T> Inner<T> {
    fn forget_idle(&mut self, owner: &Owner) {
        if self.lanes.get(owner).is_some_and(Lane::idle) {
            self.lanes.remove(owner);
        }
    }

    /// The next job to run: a started run first, then the owner with the
    /// fewest jobs running among those that may run one more, the earliest
    /// in turn among equals. The served owner goes to the back; an owner
    /// passed over keeps its turn.
    fn pick(&mut self, bounds: QueueBounds) -> Option<(Owner, JobId, T)> {
        if let Some((owner, id, task)) = self.started.pop_front() {
            self.waiting.remove(&id);
            self.in_hand.insert(id.clone());
            self.held = self.held.saturating_sub(1);
            self.lanes.entry(owner.clone()).or_default().running += 1;
            return Some((owner, id, task));
        }
        let (_, turn) = self
            .ring
            .iter()
            .enumerate()
            .filter_map(|(turn, owner)| {
                let running = self.lanes.get(owner)?.running;
                bounds
                    .running_per_owner
                    .is_none_or(|most| running < most)
                    .then_some((running, turn))
            })
            .min()?;
        let owner = self.ring.remove(turn)?;
        let lane = self.lanes.get_mut(&owner)?;
        let (id, task) = lane.waiting.pop_front()?;
        lane.running += 1;
        if !lane.waiting.is_empty() {
            self.ring.push_back(owner.clone());
        }
        self.waiting.remove(&id);
        self.in_hand.insert(id.clone());
        self.held = self.held.saturating_sub(1);
        Some((owner, id, task))
    }
}

impl<T> FairQueue<T> {
    pub(super) fn new(bounds: QueueBounds) -> Arc<Self> {
        Arc::new(Self {
            bounds,
            inner: Mutex::new(Inner {
                closed: false,
                held: 0,
                started: VecDeque::new(),
                lanes: BTreeMap::new(),
                ring: VecDeque::new(),
                waiting: BTreeSet::new(),
                in_hand: BTreeSet::new(),
            }),
            wake: Notify::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner<T>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Take one place for a job `owner` submits, before its record exists.
    pub(super) fn reserve(
        self: &Arc<Self>,
        owner: Option<&str>,
    ) -> Result<Reservation<T>, QueueRefusal> {
        self.take_place(owner.map(str::to_owned), false)
    }

    /// Take one place for a run the resident starts itself. Only the whole
    /// queue bounds it: a schedule is the operator's, never refused or held
    /// back by what its owner submitted.
    pub(super) fn reserve_started(
        self: &Arc<Self>,
        owner: Option<&str>,
    ) -> Result<Reservation<T>, QueueRefusal> {
        self.take_place(owner.map(str::to_owned), true)
    }

    fn take_place(
        self: &Arc<Self>,
        owner: Owner,
        started: bool,
    ) -> Result<Reservation<T>, QueueRefusal> {
        let mut inner = self.lock();
        if inner.closed {
            return Err(QueueRefusal::Closed);
        }
        if inner.held >= self.bounds.capacity {
            return Err(QueueRefusal::Full);
        }
        if !started {
            let lane = inner.lanes.get(&owner);
            let taken = lane.map_or(0, |lane| lane.waiting.len() + lane.reserved);
            if self
                .bounds
                .queued_per_owner
                .is_some_and(|most| taken >= most)
            {
                return Err(QueueRefusal::OwnerFull);
            }
            inner.lanes.entry(owner.clone()).or_default().reserved += 1;
        }
        inner.held += 1;
        Ok(Reservation {
            queue: Arc::clone(self),
            owner,
            started,
            live: true,
        })
    }

    /// Queue a job the store already holds as `queued`, unless it waits
    /// already or a worker holds it. A replay takes no second place, and it
    /// is never refused for a place its job already has.
    pub(super) fn requeue(
        self: &Arc<Self>,
        owner: Option<&str>,
        id: JobId,
        task: T,
    ) -> Result<(), QueueRefusal> {
        {
            let inner = self.lock();
            if inner.waiting.contains(&id) || inner.in_hand.contains(&id) {
                return Ok(());
            }
        }
        self.reserve(owner)?.send(id, task);
        Ok(())
    }

    /// Give back the place of a job that left `queued` without running (a
    /// cancellation), so its owner's next admission is not refused for it.
    pub(super) fn forget(&self, id: &JobId) {
        let mut inner = self.lock();
        if !inner.waiting.remove(id) {
            return;
        }
        inner.held = inner.held.saturating_sub(1);
        if let Some(at) = inner.started.iter().position(|(_, held, _)| held == id) {
            inner.started.remove(at);
        } else if let Some((owner, at)) = inner.lanes.iter().find_map(|(owner, lane)| {
            let at = lane.waiting.iter().position(|(held, _)| held == id)?;
            Some((owner.clone(), at))
        }) {
            if let Some(lane) = inner.lanes.get_mut(&owner) {
                lane.waiting.remove(at);
                if lane.waiting.is_empty() {
                    inner.ring.retain(|waiting| *waiting != owner);
                }
            }
            inner.forget_idle(&owner);
        }
        drop(inner);
        self.wake.notify_one();
    }

    /// Re-enter a job that was queued when the resident last stopped. It was
    /// admitted under the bounds of its day and is never refused now.
    pub(super) fn recover(&self, owner: Option<&str>, id: JobId, task: T) {
        let mut inner = self.lock();
        inner.held += 1;
        Self::enqueue(&mut inner, owner.map(str::to_owned), id, task, false);
        drop(inner);
        self.wake.notify_one();
    }

    /// Put a job in its lane. Its place is already counted in `held`.
    fn enqueue(inner: &mut Inner<T>, owner: Owner, id: JobId, task: T, started: bool) {
        if !inner.waiting.insert(id.clone()) {
            inner.held = inner.held.saturating_sub(1);
            inner.forget_idle(&owner);
            return;
        }
        if started {
            inner.started.push_back((owner, id, task));
            return;
        }
        let lane = inner.lanes.entry(owner.clone()).or_default();
        lane.waiting.push_back((id, task));
        if lane.waiting.len() == 1 {
            inner.ring.push_back(owner);
        }
    }

    /// The next job to run and the token that keeps its owner's running
    /// count. `None` once the queue is closed and nothing waits or is
    /// reserved. Dropping the future loses nothing.
    pub(super) async fn next(self: &Arc<Self>) -> Option<Dispatched<T>> {
        loop {
            {
                let mut inner = self.lock();
                if let Some((owner, id, task)) = inner.pick(self.bounds) {
                    return Some(Dispatched {
                        task,
                        running: Running {
                            queue: Arc::clone(self),
                            owner,
                            id,
                        },
                    });
                }
                if inner.closed && inner.held == 0 {
                    return None;
                }
            }
            self.wake.notified().await;
        }
    }

    /// Refuse every new place. What waits is still served.
    pub(super) fn close(&self) {
        self.lock().closed = true;
        self.wake.notify_one();
    }

    /// What the queue holds now, with its `most` busiest owners.
    pub(super) fn load(&self, most: usize) -> QueueLoad {
        let inner = self.lock();
        let mut owners: BTreeMap<&Owner, (usize, usize)> = inner
            .lanes
            .iter()
            .map(|(owner, lane)| (owner, (lane.waiting.len(), lane.running)))
            .collect();
        for (owner, _, _) in &inner.started {
            owners.entry(owner).or_default().0 += 1;
        }
        let mut owners: Vec<OwnerLoad> = owners
            .into_iter()
            .filter(|(_, (queued, running))| queued + running > 0)
            .map(|(owner, (queued, running))| OwnerLoad {
                owner: owner.clone(),
                queued,
                running,
            })
            .collect();
        let queued = owners.iter().map(|load| load.queued).sum();
        let running = owners.iter().map(|load| load.running).sum();
        // Busiest first; the lane order (anonymous, then by name) breaks ties.
        owners.sort_by_key(|load| std::cmp::Reverse(load.queued + load.running));
        let owners_omitted = owners.len().saturating_sub(most);
        owners.truncate(most);
        QueueLoad {
            queued,
            running,
            owners,
            owners_omitted,
        }
    }
}

/// One place in the queue, held until its job is sent or the admission ends.
pub(super) struct Reservation<T> {
    queue: Arc<FairQueue<T>>,
    owner: Owner,
    started: bool,
    live: bool,
}

impl<T> Reservation<T> {
    /// Queue the job this place was taken for.
    pub(super) fn send(mut self, id: JobId, task: T) {
        self.live = false;
        let owner = self.owner.take();
        let mut inner = self.queue.lock();
        if !self.started
            && let Some(lane) = inner.lanes.get_mut(&owner)
        {
            lane.reserved = lane.reserved.saturating_sub(1);
        }
        FairQueue::enqueue(&mut inner, owner, id, task, self.started);
        drop(inner);
        self.queue.wake.notify_one();
    }
}

impl<T> Drop for Reservation<T> {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        let mut inner = self.queue.lock();
        inner.held = inner.held.saturating_sub(1);
        if !self.started {
            if let Some(lane) = inner.lanes.get_mut(&self.owner) {
                lane.reserved = lane.reserved.saturating_sub(1);
            }
            inner.forget_idle(&self.owner);
        }
        drop(inner);
        // A closed queue ends once its last reservation is gone.
        self.queue.wake.notify_one();
    }
}

/// A job handed to a worker.
pub(super) struct Dispatched<T> {
    pub(super) task: T,
    pub(super) running: Running<T>,
}

/// Held while a job runs: its owner's running count. Dropping it lets the
/// owner's next job start.
pub(super) struct Running<T> {
    queue: Arc<FairQueue<T>>,
    owner: Owner,
    id: JobId,
}

impl<T> Drop for Running<T> {
    fn drop(&mut self) {
        let mut inner = self.queue.lock();
        inner.in_hand.remove(&self.id);
        if let Some(lane) = inner.lanes.get_mut(&self.owner) {
            lane.running = lane.running.saturating_sub(1);
        }
        inner.forget_idle(&self.owner);
        drop(inner);
        self.queue.wake.notify_one();
    }
}

/// The one reader of the queue. Dropping it closes the queue, so an admission
/// that outlives the resident's loop meets `Closed`, never a queue nobody
/// serves.
pub(super) struct QueueReader<T>(Arc<FairQueue<T>>);

impl<T> QueueReader<T> {
    pub(super) fn new(queue: Arc<FairQueue<T>>) -> Self {
        Self(queue)
    }

    pub(super) async fn next(&self) -> Option<Dispatched<T>> {
        self.0.next().await
    }

    pub(super) fn close(&self) {
        self.0.close();
    }

    /// The next job alone, for a test that runs it by hand.
    #[cfg(test)]
    pub(super) async fn recv(&self) -> Option<T> {
        self.next().await.map(|dispatched| dispatched.task)
    }
}

impl<T> Drop for QueueReader<T> {
    fn drop(&mut self) {
        self.0.close();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const UNBOUNDED: QueueBounds = QueueBounds {
        capacity: 64,
        queued_per_owner: None,
        running_per_owner: None,
    };

    fn put(queue: &Arc<FairQueue<&'static str>>, owner: Option<&str>, task: &'static str) -> JobId {
        let id = JobId::random();
        queue
            .reserve(owner)
            .expect("a free place")
            .send(id.clone(), task);
        id
    }

    /// Serve every waiting job, each finishing before the next starts.
    async fn served(queue: &Arc<FairQueue<&'static str>>, count: usize) -> Vec<&'static str> {
        let mut order = Vec::new();
        for _ in 0..count {
            let dispatched = queue.next().await.expect("a waiting job");
            order.push(dispatched.task);
        }
        order
    }

    async fn nothing_to_serve(queue: &Arc<FairQueue<&'static str>>) -> bool {
        tokio::time::timeout(Duration::from_millis(20), queue.next())
            .await
            .is_err()
    }

    /// One owner's burst waits in its own lane: every owner with a waiting
    /// job is served in turn, and one lane keeps its admission order.
    #[tokio::test]
    async fn owners_are_served_in_turn_and_a_lane_in_order() {
        let queue = FairQueue::new(UNBOUNDED);
        for task in ["a1", "a2", "a3", "a4", "a5"] {
            put(&queue, Some("a"), task);
        }
        put(&queue, Some("b"), "b1");
        put(&queue, None, "n1");
        put(&queue, Some("c"), "c1");
        put(&queue, Some("c"), "c2");
        put(&queue, None, "n2");
        assert_eq!(
            served(&queue, 10).await,
            ["a1", "b1", "n1", "c1", "a2", "n2", "c2", "a3", "a4", "a5"]
        );
        assert!(nothing_to_serve(&queue).await);
    }

    /// An owner that submits after a burst starts is served at the next turn,
    /// not after the burst.
    #[tokio::test]
    async fn a_late_owner_waits_one_turn_not_the_whole_burst() {
        let queue = FairQueue::new(UNBOUNDED);
        for task in ["a1", "a2", "a3", "a4"] {
            put(&queue, Some("a"), task);
        }
        assert_eq!(served(&queue, 1).await, ["a1"]);
        put(&queue, Some("b"), "b1");
        assert_eq!(served(&queue, 2).await, ["a2", "b1"]);
    }

    /// The next free worker goes to the owner with the fewest jobs running:
    /// an owner that arrives while another holds the workers starts first.
    #[tokio::test]
    async fn the_owner_with_the_fewest_running_jobs_goes_first() {
        let queue = FairQueue::new(UNBOUNDED);
        for task in ["a1", "a2", "a3", "a4"] {
            put(&queue, Some("a"), task);
        }
        let a1 = queue.next().await.expect("a1");
        let a2 = queue.next().await.expect("a2");
        put(&queue, Some("b"), "b1");
        put(&queue, Some("b"), "b2");
        // A worker frees: `a` still runs one job, `b` none.
        drop(a1);
        let b1 = queue.next().await.expect("b1");
        assert_eq!(b1.task, "b1");
        // One each now: the earliest in turn among equals is `a`.
        drop(a2);
        let a3 = queue.next().await.expect("a3");
        assert_eq!(a3.task, "a3");
        // `a` runs one and `b` one: `b` was served longer ago.
        let b2 = queue.next().await.expect("b2");
        assert_eq!(b2.task, "b2");
        assert_eq!(queue.next().await.expect("a4").task, "a4");
    }

    /// A job cancelled while it waits gives its place back at once and is
    /// never served.
    #[tokio::test]
    async fn a_forgotten_job_gives_its_place_back() {
        let queue = FairQueue::new(QueueBounds {
            capacity: 8,
            queued_per_owner: Some(2),
            running_per_owner: None,
        });
        put(&queue, Some("a"), "a1");
        let cancelled = put(&queue, Some("a"), "a2");
        let only = put(&queue, Some("b"), "b1");
        assert_eq!(
            queue.reserve(Some("a")).err(),
            Some(QueueRefusal::OwnerFull)
        );
        queue.forget(&cancelled);
        queue.forget(&cancelled);
        queue.forget(&JobId::random());
        put(&queue, Some("a"), "a3");
        queue.forget(&only);
        assert_eq!(served(&queue, 2).await, ["a1", "a3"]);
        assert!(nothing_to_serve(&queue).await);
        let started = JobId::random();
        queue
            .reserve_started(None)
            .expect("a place")
            .send(started.clone(), "scheduled");
        queue.forget(&started);
        assert!(nothing_to_serve(&queue).await);
        assert_eq!(queue.load(8).queued, 0);
    }

    #[test]
    fn one_owner_holds_at_most_its_places() {
        let queue: Arc<FairQueue<&str>> = FairQueue::new(QueueBounds {
            capacity: 64,
            queued_per_owner: Some(2),
            running_per_owner: None,
        });
        let first = queue.reserve(Some("a")).expect("first place");
        put(&queue, Some("a"), "a2");
        assert_eq!(
            queue.reserve(Some("a")).err(),
            Some(QueueRefusal::OwnerFull),
            "a reservation and a waiting job are two places"
        );
        // Another owner and the anonymous lane each have their own places.
        put(&queue, Some("b"), "b1");
        put(&queue, None, "n1");
        put(&queue, None, "n2");
        assert_eq!(queue.reserve(None).err(), Some(QueueRefusal::OwnerFull));
        // An admission that does not land gives its place back.
        drop(first);
        assert!(queue.reserve(Some("a")).is_ok());
    }

    #[test]
    fn the_whole_queue_is_bounded_whoever_asks() {
        let queue: Arc<FairQueue<&str>> = FairQueue::new(QueueBounds {
            capacity: 2,
            queued_per_owner: Some(2),
            running_per_owner: None,
        });
        put(&queue, Some("a"), "a1");
        let held = queue.reserve(Some("b")).expect("second place");
        assert_eq!(queue.reserve(Some("c")).err(), Some(QueueRefusal::Full));
        assert_eq!(
            queue.reserve_started(Some("c")).err(),
            Some(QueueRefusal::Full)
        );
        drop(held);
        assert!(queue.reserve(Some("c")).is_ok());
    }

    /// An owner at its running bound waits while the others run, keeps its
    /// turn, and starts its next job when one of its own ends.
    #[tokio::test]
    async fn an_owner_at_its_running_bound_waits_for_its_own_job() {
        let queue = FairQueue::new(QueueBounds {
            capacity: 64,
            queued_per_owner: None,
            running_per_owner: Some(1),
        });
        put(&queue, Some("a"), "a1");
        put(&queue, Some("a"), "a2");
        put(&queue, Some("b"), "b1");
        put(&queue, Some("b"), "b2");
        let a1 = queue.next().await.expect("a1");
        let b1 = queue.next().await.expect("b1");
        assert_eq!((a1.task, b1.task), ("a1", "b1"));
        assert!(
            nothing_to_serve(&queue).await,
            "both owners run their one job"
        );
        drop(b1.running);
        let b2 = queue.next().await.expect("b2");
        assert_eq!(b2.task, "b2", "the owner whose job ended runs its next");
        assert!(nothing_to_serve(&queue).await);
        drop(a1.running);
        assert_eq!(queue.next().await.expect("a2").task, "a2");
    }

    /// A run the resident started is served before every lane, is not held
    /// back by its owner's bounds, and counts in its owner's running jobs.
    #[tokio::test]
    async fn a_started_run_is_served_first_whatever_its_owner_holds() {
        let queue = FairQueue::new(QueueBounds {
            capacity: 64,
            queued_per_owner: Some(1),
            running_per_owner: Some(1),
        });
        put(&queue, Some("a"), "a1");
        put(&queue, Some("b"), "b1");
        let a1 = queue.next().await.expect("a1");
        queue
            .reserve_started(Some("a"))
            .expect("a started run is bounded by the queue alone")
            .send(JobId::random(), "a-scheduled");
        let scheduled = queue.next().await.expect("the started run");
        assert_eq!(scheduled.task, "a-scheduled");
        let load = queue.load(8);
        assert_eq!(
            load.owners,
            [
                OwnerLoad {
                    owner: Some("a".to_owned()),
                    queued: 0,
                    running: 2
                },
                OwnerLoad {
                    owner: Some("b".to_owned()),
                    queued: 1,
                    running: 0
                },
            ]
        );
        // The owner's own queue waits for both of its running jobs.
        put(&queue, Some("a"), "a2");
        assert_eq!(queue.next().await.expect("b1").task, "b1");
        drop(a1.running);
        assert!(nothing_to_serve(&queue).await);
        drop(scheduled.running);
        assert_eq!(queue.next().await.expect("a2").task, "a2");
    }

    /// A replay of a job that waits takes no second place and runs once.
    #[tokio::test]
    async fn a_waiting_job_is_never_queued_twice() {
        let queue = FairQueue::new(QueueBounds {
            capacity: 64,
            queued_per_owner: Some(1),
            running_per_owner: None,
        });
        let id = put(&queue, Some("a"), "a1");
        queue
            .requeue(Some("a"), id.clone(), "a1-replayed")
            .expect("a waiting job needs no place");
        assert_eq!(queue.load(8).queued, 1);
        assert_eq!(queue.next().await.expect("a1").task, "a1");
        assert!(nothing_to_serve(&queue).await);
        // Once it left the queue, a replay of the same id is queued again:
        // the store, not the queue, says whether it may still run.
        queue.requeue(Some("a"), id, "a1-again").expect("a place");
        assert_eq!(queue.next().await.expect("again").task, "a1-again");
    }

    /// Between the moment a worker takes a job and the moment the store says
    /// `running`, a replay still reads `queued`: it queues nothing, and it is
    /// not refused for the place its job already had.
    #[tokio::test]
    async fn a_job_a_worker_holds_is_not_queued_again() {
        let queue = FairQueue::new(QueueBounds {
            capacity: 1,
            queued_per_owner: Some(1),
            running_per_owner: None,
        });
        let id = put(&queue, Some("a"), "a1");
        let held = queue.next().await.expect("a1");
        // The owner's next job takes the only place.
        put(&queue, Some("a"), "a2");
        queue
            .requeue(Some("a"), id, "a1-replayed")
            .expect("a replay is not refused for a job a worker holds");
        assert_eq!(queue.load(8).queued, 1, "only a2 waits");
        drop(held);
        assert_eq!(queue.next().await.expect("a2").task, "a2");
        assert!(nothing_to_serve(&queue).await);
    }

    /// A job queued when the resident stopped re-enters its lane past every
    /// bound, and counts against them afterwards.
    #[tokio::test]
    async fn recovered_jobs_re_enter_their_lane_past_the_bounds() {
        let queue = FairQueue::new(QueueBounds {
            capacity: 2,
            queued_per_owner: Some(1),
            running_per_owner: None,
        });
        for task in ["a1", "a2", "a3"] {
            queue.recover(Some("a"), JobId::random(), task);
        }
        queue.recover(None, JobId::random(), "n1");
        assert_eq!(queue.reserve(Some("b")).err(), Some(QueueRefusal::Full));
        assert_eq!(served(&queue, 4).await, ["a1", "n1", "a2", "a3"]);
        assert!(queue.reserve(Some("b")).is_ok());
    }

    /// Closing refuses every new place, serves what waits, and ends once the
    /// last reservation is gone.
    #[tokio::test]
    async fn a_closed_queue_serves_what_waits_then_ends() {
        let queue = FairQueue::new(UNBOUNDED);
        put(&queue, Some("a"), "a1");
        let reserved = queue.reserve(Some("b")).expect("a place");
        let late = queue.reserve(Some("c")).expect("a place");
        queue.close();
        assert_eq!(queue.reserve(Some("d")).err(), Some(QueueRefusal::Closed));
        assert_eq!(
            queue.reserve_started(None).err(),
            Some(QueueRefusal::Closed)
        );
        assert_eq!(served(&queue, 1).await, ["a1"]);
        assert!(
            nothing_to_serve(&queue).await,
            "two admissions are still in flight"
        );
        reserved.send(JobId::random(), "b1");
        assert_eq!(served(&queue, 1).await, ["b1"]);
        drop(late);
        assert!(queue.next().await.is_none());
    }

    #[test]
    fn dropping_the_reader_closes_the_queue() {
        let queue: Arc<FairQueue<&str>> = FairQueue::new(UNBOUNDED);
        let reader = QueueReader::new(Arc::clone(&queue));
        assert!(queue.reserve(None).is_ok());
        drop(reader);
        assert_eq!(queue.reserve(None).err(), Some(QueueRefusal::Closed));
    }

    #[tokio::test]
    async fn the_load_names_the_busiest_owners_and_counts_the_rest() {
        let queue = FairQueue::new(UNBOUNDED);
        assert_eq!(
            queue.load(8),
            QueueLoad {
                queued: 0,
                running: 0,
                owners: Vec::new(),
                owners_omitted: 0
            }
        );
        put(&queue, Some("a"), "a1");
        for task in ["b1", "b2", "b3"] {
            put(&queue, Some("b"), task);
        }
        put(&queue, None, "n1");
        put(&queue, None, "n2");
        let _reserved = queue.reserve(Some("c")).expect("a place");
        let a1 = queue.next().await.expect("a1");
        let load = queue.load(2);
        assert_eq!((load.queued, load.running), (5, 1));
        assert_eq!(
            load.owners,
            [
                OwnerLoad {
                    owner: Some("b".to_owned()),
                    queued: 3,
                    running: 0
                },
                OwnerLoad {
                    owner: None,
                    queued: 2,
                    running: 0
                },
            ]
        );
        assert_eq!(
            load.owners_omitted, 1,
            "a reservation alone is not a job: only `a` is left out"
        );
        drop(a1);
        assert_eq!(queue.load(8).running, 0);
    }
}
