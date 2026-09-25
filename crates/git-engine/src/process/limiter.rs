//! The shared concurrency limiter for `git` spawns (§4 Low-resource
//! operation, design rule 3 "Adaptive concurrency"; ADR 0004).
//!
//! At most [`cap_for`]`(available_parallelism())` git processes run at once:
//! 2 on a low-end machine (≤ 4 hardware threads), 4 otherwise. Every
//! [`GitCommand`](super::GitCommand) holds a [`Permit`] from
//! [`Limiter::shared`] for the whole life of its process.
//!
//! Two priority lanes: a permit that frees up goes to the oldest
//! [`Priority::Visible`] waiter, and to a [`Priority::Background`] waiter only
//! when no visible one is queued. Each lane is FIFO. Background work can
//! starve behind steady visible work; that is the intended trade — the
//! visible view wins.
//!
//! Waiting is cancellable. [`Limiter::acquire`] returns as soon as its token
//! is cancelled and removes its queue entry, and a permit that was handed to
//! it in the same instant is released again rather than leaked. Dropping the
//! `acquire` future has the same effect.
//!
//! Cost: one `std::sync::Mutex` held for a few instructions per acquire and
//! release, never across an `.await`; one `oneshot` channel per waiter that
//! had to queue. Memory is bounded by the number of live `acquire` futures.

use std::collections::VecDeque;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::runtime;

/// Which lane a git spawn waits in when the limiter is full.
///
/// The default is [`Visible`](Self::Visible): a spawn nobody classified is
/// most likely a user action, and the safe mistake is to serve it early, not
/// to park it behind bulk work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Priority {
    /// Work the user is looking at: the active view's status, diff, log page.
    #[default]
    Visible,
    /// Work nobody is waiting for: prefetch, a hidden view's refresh, polling.
    Background,
}

impl Priority {
    fn lane(self) -> usize {
        match self {
            Priority::Visible => 0,
            Priority::Background => 1,
        }
    }
}

/// The cap for a machine with `parallelism` hardware threads: 2 on a low-end
/// machine (≤ 4), 4 otherwise (§4 "Adaptive concurrency").
pub fn cap_for(parallelism: usize) -> usize {
    if parallelism <= 4 {
        2
    } else {
        4
    }
}

/// [`Limiter::acquire`] was cancelled before it obtained a permit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cancelled while waiting for a git slot")]
pub struct Cancelled;

/// A queued `acquire`: the permit is delivered through `tx`.
struct Waiter {
    id: u64,
    tx: oneshot::Sender<Permit>,
}

struct State {
    in_flight: usize,
    next_id: u64,
    /// Indexed by [`Priority::lane`]; visible first.
    lanes: [VecDeque<Waiter>; 2],
}

struct Inner {
    cap: usize,
    state: Mutex<State>,
    peak: AtomicUsize,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        // The state is only ever mutated under the lock in straight-line
        // code, so it is consistent even if a holder panicked.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Gives a permit back: to the oldest visible waiter, else the oldest
    /// background waiter, else to the pool.
    fn release(self: &Arc<Self>) {
        let mut state = self.lock();
        for lane in &mut state.lanes {
            while let Some(waiter) = lane.pop_front() {
                match waiter.tx.send(Permit::new(self)) {
                    // Handed over: `in_flight` is unchanged.
                    Ok(()) => return,
                    // That waiter dropped its future before we got here;
                    // give the permit back without releasing it twice.
                    Err(unclaimed) => unclaimed.forget(),
                }
            }
        }
        state.in_flight -= 1;
    }
}

/// A bounded pool of git slots with two priority lanes. See the module docs.
///
/// Cloning is cheap and shares the pool; use [`Limiter::shared`] for the
/// process-wide one.
#[derive(Clone)]
pub struct Limiter {
    inner: Arc<Inner>,
}

impl Limiter {
    /// A limiter of its own with `cap` slots, for tests and tools. Production
    /// code uses [`Limiter::shared`].
    pub fn new(cap: NonZeroUsize) -> Self {
        Self {
            inner: Arc::new(Inner {
                cap: cap.get(),
                state: Mutex::new(State {
                    in_flight: 0,
                    next_id: 0,
                    lanes: [VecDeque::new(), VecDeque::new()],
                }),
                peak: AtomicUsize::new(0),
            }),
        }
    }

    /// The process-wide limiter every `GitCommand` goes through, sized by
    /// [`cap_for`] from [`runtime::available_parallelism`].
    pub fn shared() -> &'static Limiter {
        static SHARED: LazyLock<Limiter> = LazyLock::new(|| {
            let cap = cap_for(runtime::available_parallelism());
            Limiter::new(NonZeroUsize::new(cap).unwrap_or(NonZeroUsize::MIN))
        });
        &SHARED
    }

    /// How many permits can be out at once.
    pub fn cap(&self) -> usize {
        self.inner.cap
    }

    /// Permits currently held (including one in transit to a waiter).
    pub fn in_flight(&self) -> usize {
        self.inner.lock().in_flight
    }

    /// Requests queued in `priority`'s lane right now.
    pub fn waiting(&self, priority: Priority) -> usize {
        self.inner.lock().lanes[priority.lane()].len()
    }

    /// The most permits ever held at once by this limiter.
    pub fn peak_in_flight(&self) -> usize {
        self.inner.peak.load(Ordering::Relaxed)
    }

    /// Waits for a slot. Returns [`Cancelled`] as soon as `cancel` fires,
    /// including when it already had.
    pub async fn acquire(
        &self,
        priority: Priority,
        cancel: &CancellationToken,
    ) -> Result<Permit, Cancelled> {
        if cancel.is_cancelled() {
            return Err(Cancelled);
        }
        let (id, rx) = {
            let mut state = self.inner.lock();
            if state.in_flight < self.inner.cap {
                state.in_flight += 1;
                self.inner
                    .peak
                    .fetch_max(state.in_flight, Ordering::Relaxed);
                return Ok(Permit::new(&self.inner));
            }
            let id = state.next_id;
            state.next_id += 1;
            let (tx, rx) = oneshot::channel();
            state.lanes[priority.lane()].push_back(Waiter { id, tx });
            (id, rx)
        };
        // Removes the queue entry again if this future ends before the
        // permit arrived (cancelled, or dropped by the caller).
        let _slot = QueueSlot {
            inner: &self.inner,
            lane: priority.lane(),
            id,
        };
        tokio::select! {
            biased;
            // Cancellation first: a permit that arrived in the same instant
            // is dropped with `rx` below, which releases it.
            () = cancel.cancelled() => Err(Cancelled),
            // `Err` would mean the sender went away without sending, which
            // only `_slot` does, after this point.
            permit = rx => permit.map_err(|_gone| Cancelled),
        }
    }
}

impl fmt::Debug for Limiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.inner.lock();
        f.debug_struct("Limiter")
            .field("cap", &self.inner.cap)
            .field("in_flight", &state.in_flight)
            .field("waiting_visible", &state.lanes[0].len())
            .field("waiting_background", &state.lanes[1].len())
            .finish()
    }
}

/// Drops a queued waiter's entry unless it was already popped by a release.
struct QueueSlot<'a> {
    inner: &'a Arc<Inner>,
    lane: usize,
    id: u64,
}

impl Drop for QueueSlot<'_> {
    fn drop(&mut self) {
        let mut state = self.inner.lock();
        let lane = &mut state.lanes[self.lane];
        if let Some(position) = lane.iter().position(|waiter| waiter.id == self.id) {
            lane.remove(position);
        }
    }
}

/// One git slot. Dropping it hands the slot to the next waiter.
pub struct Permit {
    /// `None` once the permit has been released or forgotten.
    inner: Option<Arc<Inner>>,
}

impl Permit {
    fn new(inner: &Arc<Inner>) -> Self {
        Self {
            inner: Some(Arc::clone(inner)),
        }
    }

    /// Discards the permit without releasing the slot, for a permit that was
    /// never really handed out.
    fn forget(mut self) {
        self.inner = None;
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            inner.release();
        }
    }
}

impl fmt::Debug for Permit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Permit")
            .field("held", &self.inner.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(cap: usize) -> Limiter {
        Limiter::new(NonZeroUsize::new(cap).unwrap())
    }

    #[tokio::test]
    async fn permits_come_back_when_dropped() {
        let limiter = limiter(2);
        let token = CancellationToken::new();
        let a = limiter.acquire(Priority::Visible, &token).await.unwrap();
        let b = limiter.acquire(Priority::Background, &token).await.unwrap();
        assert_eq!(limiter.in_flight(), 2);
        drop(a);
        assert_eq!(limiter.in_flight(), 1);
        drop(b);
        assert_eq!(limiter.in_flight(), 0);
        assert_eq!(limiter.peak_in_flight(), 2);
    }

    #[tokio::test]
    async fn dropping_a_queued_future_removes_its_entry() {
        let limiter = limiter(1);
        let token = CancellationToken::new();
        let held = limiter.acquire(Priority::Visible, &token).await.unwrap();

        let task = {
            let limiter = limiter.clone();
            tokio::spawn(async move {
                let token = CancellationToken::new();
                limiter.acquire(Priority::Background, &token).await
            })
        };
        while limiter.waiting(Priority::Background) == 0 {
            tokio::task::yield_now().await;
        }
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());

        assert_eq!(limiter.waiting(Priority::Background), 0);
        drop(held);
        assert_eq!(limiter.in_flight(), 0);
    }

    #[test]
    fn debug_output_shows_the_pool_state() {
        let limiter = limiter(3);
        assert_eq!(
            format!("{limiter:?}"),
            "Limiter { cap: 3, in_flight: 0, waiting_visible: 0, waiting_background: 0 }"
        );
    }
}
