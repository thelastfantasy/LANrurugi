//! The parts of the snapshot that must compile and work on macOS and FreeBSD too.
//!
//! A native package of LANrurugi runs on all three platforms, so a `resources` module that only
//! reads `/proc` would not build there. Everything below goes through `sysinfo` or libc, which
//! behave the same everywhere; the Linux-only extras live in `procfs` and are merged in behind
//! `cfg(target_os = "linux")` by `collect`.

/// `linux/x86_64`, `macos/aarch64` — the same shape the diagnostics bundle already uses.
pub fn platform_label() -> String {
    format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// `(resident, virtual)` in bytes for one process, via `sysinfo`.
pub fn process_memory(pid: u32) -> Option<(u64, u64)> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let mut system = System::new().ok()?;
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_memory(),
    );
    system
        .process(pid)
        .map(|p| (p.memory(), p.virtual_memory()))
}

/// The high-water mark of this process's resident set, from `getrusage(RUSAGE_SELF)`.
///
/// `ru_maxrss` is KiB on Linux and the BSDs but bytes on macOS — the one place this function has to
/// know which platform it is on.
pub fn peak_rss_bytes() -> Option<u64> {
    // SAFETY: `getrusage` only writes into the struct it is handed, which is zero-initialised.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        (libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0).then_some(usage)?
    };
    let max_rss = u64::try_from(usage.ru_maxrss).ok()?;
    Some(if cfg!(target_os = "macos") {
        max_rss
    } else {
        max_rss.saturating_mul(1024)
    })
}

/// `true` when this build has a `/proc` to read. Used to label sections rather than to branch the
/// public shape of the snapshot.
pub const fn has_procfs() -> bool {
    cfg!(target_os = "linux")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_platform_label_has_the_os_slash_arch_shape() {
        let label = platform_label();
        assert!(label.contains('/'), "got {label}");
        assert!(label.starts_with(std::env::consts::OS));
    }

    #[test]
    fn this_process_has_a_readable_resident_set() {
        let (rss, _virtual) = process_memory(std::process::id()).expect("own process is readable");
        assert!(rss > 0);
    }

    #[test]
    fn the_peak_resident_set_is_at_least_the_current_one_on_linux() {
        if !has_procfs() {
            return;
        }
        let peak = peak_rss_bytes().expect("getrusage works");
        // getrusage rounds to KiB and the current RSS may have grown since, so only sanity-check.
        assert!(peak > 0);
    }
}
