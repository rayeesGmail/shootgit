//! Killing a child together with every process it started.
//!
//! git runs aliases, hooks, `ssh` and credential helpers as its own children.
//! Killing only git leaves them running. They hold our stdout/stderr pipes
//! open, so our reads do not finish. On Windows those reads run on tokio's
//! blocking pool, so every timeout would leave a pool thread stuck, and
//! dropping the runtime would block, until the grandchild exited on its own.
//!
//! [`ProcessTree`] is attached right after spawn. It kills the whole tree
//! when [`ProcessTree::kill`] is called (timeout) or when it is dropped while
//! still armed (the `output()` future was cancelled, or failed mid-run).
//! [`ProcessTree::disarm`] is called once the child has finished normally,
//! so anything it deliberately left running (an fsmonitor daemon) survives.
//!
//! - Unix: the child is spawned as the leader of a new process group
//!   ([`configure`] sets `process_group(0)`), and the tree is killed with
//!   `kill(-pgid, SIGKILL)`. A descendant that calls `setsid` or `setpgid`
//!   itself (a daemon) leaves the group and is not killed.
//! - Windows: the child is assigned to a Job Object with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`; its descendants join the job
//!   automatically, and `TerminateJobObject` or closing the handle kills
//!   them. Known race: a descendant started before the assignment (in the
//!   microseconds between spawn and `AssignProcessToJobObject`) escapes the
//!   job; git reads its config first, so in practice nothing is started yet.

/// Prepares a command so that [`ProcessTree`] can kill its whole tree.
pub(super) fn configure(command: &mut tokio::process::Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(not(unix))]
    let _ = command;
}

#[cfg(unix)]
pub(super) use unix::ProcessTree;
#[cfg(windows)]
pub(super) use windows::ProcessTree;

#[cfg(unix)]
mod unix {
    /// The process group of a spawned child. See the module docs.
    #[derive(Debug)]
    pub(in crate::process) struct ProcessTree {
        /// `Some(pgid)` while armed.
        pgid: Option<libc::pid_t>,
    }

    impl ProcessTree {
        /// Attaches to `child`, which [`super::configure`] made a group leader.
        pub(in crate::process) fn attach(child: &tokio::process::Child) -> Self {
            let pgid = child
                .id()
                .and_then(|pid| libc::pid_t::try_from(pid).ok())
                .filter(|&pid| pid > 0);
            Self { pgid }
        }

        /// Kills every process still in the group, then disarms.
        ///
        /// Call it before the leader is reaped: while the leader is unreaped
        /// its pid, and so the group id, cannot be reused.
        pub(in crate::process) fn kill(&mut self) {
            let Some(pgid) = self.pgid.take() else {
                return;
            };
            // SAFETY: kill(2) takes plain integers and touches no memory we
            // own. A negative pid addresses the process group `pgid`, which
            // is the group we created for this child; the checks in `attach`
            // guarantee `pgid > 0`, so this is never kill(-1) or kill(0).
            let rc = unsafe { libc::kill(-pgid, libc::SIGKILL) };
            if rc != 0 {
                let error = std::io::Error::last_os_error();
                // ESRCH: the group is already empty, which is the goal.
                if error.raw_os_error() != Some(libc::ESRCH) {
                    tracing::warn!(pgid, %error, "could not kill process group");
                }
            }
        }

        /// The child finished normally: leave the rest of the group alone.
        pub(in crate::process) fn disarm(&mut self) {
            self.pgid = None;
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) {
            self.kill();
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::mem::size_of;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// An owned Job Object handle.
    #[derive(Debug)]
    struct Job(HANDLE);

    // SAFETY: a Job Object handle is a kernel handle; the job APIs are
    // thread-safe and the handle is not tied to the thread that created it.
    // `Job` owns it exclusively and closes it exactly once in `Drop`.
    unsafe impl Send for Job {}
    // SAFETY: as above; `&Job` exposes no method at all.
    unsafe impl Sync for Job {}

    impl Job {
        fn create() -> std::io::Result<Self> {
            // SAFETY: null attributes and a null name are documented as
            // valid (default security, anonymous job).
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let job = Self(handle);
            job.set_kill_on_close(true)?;
            Ok(job)
        }

        fn set_kill_on_close(&self, on: bool) -> std::io::Result<()> {
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            if on {
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            }
            // SAFETY: `self.0` is a live job handle; `info` is a properly
            // initialised JOBOBJECT_EXTENDED_LIMIT_INFORMATION that outlives
            // the call, and the length passed is its exact size.
            let ok = unsafe {
                SetInformationJobObject(
                    self.0,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&info).cast::<c_void>(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }

        fn assign(&self, process: HANDLE) -> std::io::Result<()> {
            // SAFETY: both handles are live: `self.0` is owned by `self`,
            // `process` comes from a `Child` the caller still borrows.
            if unsafe { AssignProcessToJobObject(self.0, process) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }

        fn terminate(&self) -> std::io::Result<()> {
            // SAFETY: `self.0` is a live job handle.
            if unsafe { TerminateJobObject(self.0, 1) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a live handle owned by `self` and is not
            // used after this. With KILL_ON_JOB_CLOSE still set, closing the
            // last handle kills every process in the job.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// The Job Object holding a spawned child. See the module docs.
    #[derive(Debug)]
    pub(in crate::process) struct ProcessTree {
        /// `None` if the job could not be set up; then only the direct child
        /// is killed (by tokio's `kill_on_drop` / `Child::kill`).
        job: Option<Job>,
        armed: bool,
    }

    impl ProcessTree {
        pub(in crate::process) fn attach(child: &tokio::process::Child) -> Self {
            let job = child.raw_handle().and_then(|process| {
                let result = Job::create().and_then(|job| {
                    job.assign(process.cast())?;
                    Ok(job)
                });
                match result {
                    Ok(job) => Some(job),
                    Err(error) => {
                        tracing::warn!(%error, "could not put child in a job object; only it will be killed");
                        None
                    }
                }
            });
            Self { job, armed: true }
        }

        /// Kills every process in the job, then disarms.
        pub(in crate::process) fn kill(&mut self) {
            if !std::mem::take(&mut self.armed) {
                return;
            }
            if let Some(job) = &self.job {
                if let Err(error) = job.terminate() {
                    tracing::warn!(%error, "could not terminate job object");
                }
            }
        }

        /// The child finished normally: closing the job must not kill what
        /// it left running.
        pub(in crate::process) fn disarm(&mut self) {
            if !std::mem::take(&mut self.armed) {
                return;
            }
            let failed = self
                .job
                .as_ref()
                .and_then(|job| job.set_kill_on_close(false).err());
            if let Some(error) = failed {
                tracing::warn!(%error, "could not clear kill-on-close; leaking the job handle");
                // Leak the handle rather than kill what the child left running.
                if let Some(job) = self.job.take() {
                    std::mem::forget(job);
                }
            }
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) {
            // Armed: closing the handle below (KILL_ON_JOB_CLOSE) kills the
            // tree. Disarmed: the limit was cleared and nothing is killed.
            if self.armed {
                self.kill();
            }
        }
    }
}
