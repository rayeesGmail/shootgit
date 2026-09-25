//! Resident set size of this process, read on demand.
//!
//! Peaks come from the kernel's own high-water marks (`ru_maxrss`,
//! `PeakWorkingSetSize`), so nothing has to sample in the background. Every
//! function returns `None` where the platform gives no answer, never a made
//! up number.

/// Current resident set, in bytes.
pub fn current() -> Option<u64> {
    imp::current()
}

/// Peak resident set since the process started, in bytes.
pub fn peak() -> Option<u64> {
    imp::peak()
}

/// Peak resident set of the largest child process waited for so far, in
/// bytes. `None` on Windows, which keeps no such figure.
pub fn children_peak() -> Option<u64> {
    imp::children_peak()
}

#[cfg(unix)]
mod imp {
    use std::mem::MaybeUninit;

    /// `ru_maxrss` in bytes. The kernel reports kibibytes everywhere but on
    /// macOS, where it is bytes.
    fn max_rss(who: libc::c_int) -> Option<u64> {
        let mut usage = MaybeUninit::<libc::rusage>::zeroed();
        // SAFETY: `usage` is a writable buffer of exactly one `rusage` for
        // the duration of the call, and getrusage writes nothing else.
        let rc = unsafe { libc::getrusage(who, usage.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }
        // SAFETY: getrusage returned 0, so it filled the struct.
        let usage = unsafe { usage.assume_init() };
        let raw = u64::try_from(usage.ru_maxrss).ok()?;
        let scale = if cfg!(target_os = "macos") { 1 } else { 1024 };
        raw.checked_mul(scale).filter(|&bytes| bytes > 0)
    }

    pub(super) fn peak() -> Option<u64> {
        max_rss(libc::RUSAGE_SELF)
    }

    pub(super) fn children_peak() -> Option<u64> {
        max_rss(libc::RUSAGE_CHILDREN)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn current() -> Option<u64> {
        // statm: size resident shared text lib data dt, in pages.
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        // SAFETY: sysconf takes an integer and touches no memory.
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let page_size = u64::try_from(page_size).ok()?;
        resident_pages
            .checked_mul(page_size)
            .filter(|&bytes| bytes > 0)
    }

    #[cfg(target_os = "macos")]
    pub(super) fn current() -> Option<u64> {
        let mut info = MaybeUninit::<libc::proc_taskinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_taskinfo>();
        // SAFETY: `info` is a writable buffer of exactly `size` bytes, which
        // is what PROC_PIDTASKINFO fills for a pid; ours always exists.
        let written = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTASKINFO,
                0,
                info.as_mut_ptr().cast(),
                size as libc::c_int,
            )
        };
        if usize::try_from(written).ok()? < size {
            return None;
        }
        // SAFETY: proc_pidinfo reported that it wrote the whole struct.
        let info = unsafe { info.assume_init() };
        Some(info.pti_resident_size).filter(|&bytes| bytes > 0)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) fn current() -> Option<u64> {
        None
    }
}

#[cfg(windows)]
mod imp {
    use std::mem::size_of;

    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    fn counters() -> Option<PROCESS_MEMORY_COUNTERS> {
        let size = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: size,
            ..PROCESS_MEMORY_COUNTERS::default()
        };
        // SAFETY: GetCurrentProcess returns a pseudo-handle that is always
        // valid; `counters` is a writable struct of the `size` passed.
        let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) };
        (ok != 0).then_some(counters)
    }

    pub(super) fn current() -> Option<u64> {
        counters()
            .map(|c| c.WorkingSetSize as u64)
            .filter(|&bytes| bytes > 0)
    }

    pub(super) fn peak() -> Option<u64> {
        counters()
            .map(|c| c.PeakWorkingSetSize as u64)
            .filter(|&bytes| bytes > 0)
    }

    pub(super) fn children_peak() -> Option<u64> {
        None
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    pub(super) fn current() -> Option<u64> {
        None
    }

    pub(super) fn peak() -> Option<u64> {
        None
    }

    pub(super) fn children_peak() -> Option<u64> {
        None
    }
}
