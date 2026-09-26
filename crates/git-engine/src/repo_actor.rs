//! One actor per open repository (§4 Process model): a tokio task that owns
//! the [`Repo`] and runs the operations submitted to it, reads side by side
//! and writes one at a time.
//!
//! # Ordering
//!
//! Operations start in the order [`RepoActor::read`] and
//! [`RepoActor::write`] were called, with one relaxation: reads submitted
//! one after another run at the same time. So:
//!
//! - a write starts once every operation submitted before it has finished,
//!   and nothing else runs until it has finished;
//! - a read sees every write submitted before it and none submitted after
//!   it;
//! - the reads between two writes all run concurrently (the git limiter
//!   still caps how many git processes that makes, ADR 0004).
//!
//! It is a readers-writer discipline with first-come, first-served
//! fairness: later reads never overtake a queued write, so a stream of reads
//! cannot starve writes, and a write holds reads back only while it runs.
//!
//! The order covers what is submitted here. Other programs (an editor, the
//! git CLI, a hook) change the repository whenever they like; the watcher
//! (P0-10) reports that.
//!
//! # Re-entrancy
//!
//! An operation must not submit a write to its own actor and await it, and a
//! write must not submit anything to its own actor and await it: either one
//! deadlocks. A write waits for every running read, including the read that
//! is waiting for it, and while a write runs the actor takes no new job. A
//! read may submit a read and await it. For a follow-up such as "stage, then
//! refresh status", return from the operation and submit the next one from
//! the caller.
//!
//! # Mechanics
//!
//! Submitting pushes a job onto the mailbox, an unbounded channel, before
//! `read` or `write` returns, so the order is the order of the calls, not of
//! the first poll of the futures they return. The actor takes one job at a
//! time. A read is spawned as a task of its own, tracked in a `JoinSet`,
//! and the actor moves on at once. A write first waits for every tracked
//! read, then is spawned and awaited before the actor takes the next job.
//! Spawning each operation keeps a panic inside it: its caller gets
//! [`GitError::Aborted`] and the actor carries on.
//!
//! The mailbox is unbounded so that submitting never waits. A queued job is
//! one small boxed closure; the producers are user actions and debounced
//! watcher events; and a superseded read still in the queue ends at once
//! when its turn comes, because its cancellation token has fired.
//!
//! Everything runs on the runtime that called [`RepoActor::spawn`], which in
//! the app is the single runtime handed to Tauri (ADR 0004).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinError, JoinSet};

use crate::error::GitError;
use crate::repo::Repo;

/// A submitted operation, with its result already routed to its caller.
type Operation = Box<dyn FnOnce(Arc<Repo>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// Whether an operation may run beside others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    Write,
}

struct Job {
    access: Access,
    operation: Operation,
}

/// The handle to one open repository's actor. Cloning it is cheap and every
/// clone talks to the same actor.
///
/// The actor runs until every clone has been dropped, and then until every
/// operation already submitted has finished.
#[derive(Debug, Clone)]
pub struct RepoActor {
    repo: Arc<Repo>,
    mailbox: mpsc::UnboundedSender<Job>,
}

impl RepoActor {
    /// Starts the actor for `repo` as a task on the current tokio runtime.
    ///
    /// # Panics
    ///
    /// When called outside a tokio runtime, as `tokio::spawn` does.
    pub fn spawn(repo: Repo) -> Self {
        let repo = Arc::new(repo);
        let (mailbox, jobs) = mpsc::unbounded_channel();
        drop(tokio::spawn(run(Arc::clone(&repo), jobs)));
        Self { repo, mailbox }
    }

    /// The repository this actor owns.
    pub fn repo(&self) -> &Repo {
        &self.repo
    }

    /// Submits `op`, an operation that only reads the repository, and returns
    /// its result.
    ///
    /// `op` starts once every write submitted before it has finished, runs
    /// alongside the other reads submitted between the same two writes, and
    /// finishes before any write submitted after it starts (see the module
    /// docs). It is queued when `read` is called: dropping the returned
    /// future does not withdraw it. To stop a read, give `op` a
    /// `CancellationToken` and cancel that.
    ///
    /// Awaiting it inside a write on the same actor deadlocks; see
    /// [Re-entrancy](crate::repo_actor#re-entrancy).
    ///
    /// Fails with [`GitError::Aborted`] if `op` panics.
    pub fn read<T, F, Fut>(
        &self,
        op: F,
    ) -> impl Future<Output = Result<T, GitError>> + Send + 'static
    where
        F: FnOnce(Arc<Repo>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, GitError>> + Send + 'static,
        T: Send + 'static,
    {
        self.submit(Access::Read, op)
    }

    /// Submits `op`, an operation that changes the repository, and returns
    /// its result.
    ///
    /// `op` starts once every operation submitted before it has finished,
    /// and runs alone (see the module docs). It is queued when `write` is
    /// called, and runs even if the returned future is dropped: a write is
    /// never abandoned halfway because nobody is waiting for it.
    ///
    /// Awaiting it inside any operation on the same actor deadlocks; see
    /// [Re-entrancy](crate::repo_actor#re-entrancy).
    ///
    /// Fails with [`GitError::Aborted`] if `op` panics.
    pub fn write<T, F, Fut>(
        &self,
        op: F,
    ) -> impl Future<Output = Result<T, GitError>> + Send + 'static
    where
        F: FnOnce(Arc<Repo>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, GitError>> + Send + 'static,
        T: Send + 'static,
    {
        self.submit(Access::Write, op)
    }

    fn submit<T, F, Fut>(
        &self,
        access: Access,
        op: F,
    ) -> impl Future<Output = Result<T, GitError>> + Send + 'static
    where
        F: FnOnce(Arc<Repo>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, GitError>> + Send + 'static,
        T: Send + 'static,
    {
        let (reply, answer) = oneshot::channel();
        let operation: Operation = Box::new(move |repo| {
            Box::pin(async move {
                let result = op(repo).await;
                if reply.send(result).is_err() {
                    tracing::trace!(
                        ?access,
                        "nobody waited for the repository operation's result"
                    );
                }
            })
        });
        // Fails only when the actor task is gone: its runtime shut down.
        let queued = self.mailbox.send(Job { access, operation }).is_ok();
        async move {
            if !queued {
                return Err(GitError::Aborted);
            }
            // A dropped reply means the operation panicked or its task was
            // cancelled by a runtime shutdown.
            answer.await.unwrap_or(Err(GitError::Aborted))
        }
    }
}

/// The actor: takes jobs in order until the mailbox closes, then waits for
/// the reads still running.
async fn run(repo: Arc<Repo>, mut jobs: mpsc::UnboundedReceiver<Job>) {
    let mut reads = JoinSet::new();
    loop {
        let job = tokio::select! {
            job = jobs.recv() => job,
            // Collect finished reads as they end, so the set holds only the
            // running ones.
            Some(done) = reads.join_next(), if !reads.is_empty() => {
                report(done);
                continue;
            }
        };
        let Some(Job { access, operation }) = job else {
            break;
        };
        match access {
            Access::Read => {
                reads.spawn(operation(Arc::clone(&repo)));
            }
            Access::Write => {
                while let Some(done) = reads.join_next().await {
                    report(done);
                }
                report(tokio::spawn(operation(Arc::clone(&repo))).await);
            }
        }
    }
    while let Some(done) = reads.join_next().await {
        report(done);
    }
    tracing::debug!(workdir = ?repo.workdir(), "repository actor stopped");
}

/// Logs an operation that did not finish normally. Its caller has already
/// been told: its reply was dropped.
fn report(done: Result<(), JoinError>) {
    if let Err(error) = done {
        if error.is_panic() {
            tracing::error!(%error, "repository operation panicked");
        } else {
            tracing::debug!(%error, "repository operation cancelled");
        }
    }
}
