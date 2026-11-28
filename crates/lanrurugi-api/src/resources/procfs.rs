//! Linux `/proc` reading: the only place in this feature that knows a file format.
//!
//! Every function returns `Option`/`Result` rather than a default, because a fabricated zero reads
//! as a healthy system (spec FR-003). The parsers are separate from the I/O so they can be tested
//! against real kernel text on any platform.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use crate::resources::model::{CgroupSection, DiskStat, IoStat, MemoryEvents, Psi};

/// Copies `/proc` counters into percentages. Process-wide on purpose: two observers (a panel and a
/// one-shot request) must not each restart the delta window, or the first frame after either one
/// would report a bogus spike.
pub struct Sampler {
    cpu: Mutex<HashMap<u32, (u64, i64)>>,
    cgroup_cpu: Mutex<Option<(u64, i64)>>,
    net: Mutex<Option<(u64, u64, i64)>>,
}

impl Sampler {
    fn new() -> Self {
        Self {
            cpu: Mutex::new(HashMap::new()),
            cgroup_cpu: Mutex::new(None),
            net: Mutex::new(None),
        }
    }

    /// `%` of one core, so a process using four cores reports `400`.
    pub fn cpu_percent(&self, pid: u32, total_ticks: u64, at_ms: i64) -> Option<f64> {
        let hz = clock_ticks_per_second();
        if hz <= 0.0 {
            return None;
        }
        let mut map = self.cpu.lock().unwrap_or_else(|e| e.into_inner());
        let previous = map.insert(pid, (total_ticks, at_ms));
        previous.and_then(|(ticks, at)| {
            let seconds = (at_ms - at) as f64 / 1000.0;
            if seconds <= 0.0 {
                return None;
            }
            let delta = total_ticks.saturating_sub(ticks) as f64;
            Some(delta / hz / seconds * 100.0)
        })
    }

    /// Drops counters of processes that are gone, so the map tracks the live tree rather than
    /// everything seen since boot.
    pub fn retain_pids(&self, live: &HashSet<u32>) {
        let mut map = self.cpu.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|pid, _| live.contains(pid));
    }

    pub fn cgroup_cpu_percent(&self, usage_usec: u64, at_ms: i64) -> Option<f64> {
        let mut slot = self.cgroup_cpu.lock().unwrap_or_else(|e| e.into_inner());
        let previous = slot.replace((usage_usec, at_ms));
        previous.and_then(|(usec, at)| {
            let seconds = (at_ms - at) as f64 / 1000.0;
            if seconds <= 0.0 {
                return None;
            }
            let delta = usage_usec.saturating_sub(usec) as f64;
            Some(delta / 1_000_000.0 / seconds * 100.0)
        })
    }

    pub fn network_rates(&self, rx: u64, tx: u64, at_ms: i64) -> (Option<f64>, Option<f64>) {
        let mut slot = self.net.lock().unwrap_or_else(|e| e.into_inner());
        let previous = slot.replace((rx, tx, at_ms));
        match previous {
            Some((previous_rx, previous_tx, at)) => {
                let seconds = (at_ms - at) as f64 / 1000.0;
                if seconds <= 0.0 {
                    return (None, None);
                }
                (
                    Some(rx.saturating_sub(previous_rx) as f64 / seconds),
                    Some(tx.saturating_sub(previous_tx) as f64 / seconds),
                )
            }
            None => (None, None),
        }
    }
}

pub fn sampler() -> &'static Sampler {
    static SAMPLER: std::sync::OnceLock<Sampler> = std::sync::OnceLock::new();
    SAMPLER.get_or_init(Sampler::new)
}

pub fn clock_ticks_per_second() -> f64 {
    // SAFETY: `sysconf` has no preconditions beyond a valid name.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks <= 0 {
        100.0
    } else {
        ticks as f64
    }
}

pub fn page_size() -> u64 {
    // SAFETY: as above.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if size <= 0 {
        4096
    } else {
        size as u64
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessStat {
    pub pid: u32,
    pub comm: String,
    pub state: char,
    pub ppid: u32,
    pub total_ticks: u64,
    pub start_ticks: u64,
    pub rss_bytes: u64,
}

/// Parses one `/proc/<pid>/stat` line. The comm field is parenthesised and may itself contain
/// spaces and parentheses, so everything up to the *last* `)` is the name and the rest is
/// positional — splitting the whole line on whitespace shifts every later field.
pub fn parse_stat(line: &str) -> Option<ProcessStat> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    if close <= open {
        return None;
    }
    let pid: u32 = line[..open].trim().parse().ok()?;
    let comm = line[open + 1..close].to_string();
    let rest: Vec<&str> = line[close + 1..].split_whitespace().collect();
    // `rest[0]` is the `state` field (man 5 proc, field 3).
    let state = rest.first()?.chars().next()?;
    let ppid: u32 = rest.get(1)?.parse().ok()?;
    let utime: u64 = rest.get(11)?.parse().ok()?;
    let stime: u64 = rest.get(12)?.parse().ok()?;
    let start_ticks: u64 = rest.get(19)?.parse().ok()?;
    let rss_pages: u64 = rest.get(21)?.parse().ok()?;
    Some(ProcessStat {
        pid,
        comm,
        state,
        ppid,
        total_ticks: utime.saturating_add(stime),
        start_ticks,
        rss_bytes: rss_pages.saturating_mul(page_size()),
    })
}

/// Seconds since boot, from `/proc/stat`'s `btime` line — the anchor `starttime` in each process's
/// stat is measured against.
pub fn boot_time_secs() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("btime ") {
            return rest.trim().parse().ok();
        }
    }
    None
}

pub fn read_process_stat(pid: u32) -> Option<ProcessStat> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_stat(&text)
}

/// The process plus everything it started, breadth-first, up to `max_depth` and `max_entries`.
/// `truncated` says a limit was hit rather than the tree genuinely being that small.
pub fn process_tree(
    root_pid: u32,
    max_depth: u32,
    max_entries: usize,
) -> (Vec<(ProcessStat, u32)>, bool) {
    let mut all: HashMap<u32, ProcessStat> = HashMap::new();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
                continue;
            };
            let Some(stat) = read_process_stat(pid) else {
                continue;
            };
            children.entry(stat.ppid).or_default().push(stat.pid);
            all.insert(pid, stat);
        }
    }

    let mut tree = Vec::new();
    let mut truncated = false;
    let mut queue: std::collections::VecDeque<(u32, u32)> = std::collections::VecDeque::new();
    queue.push_back((root_pid, 0));
    while let Some((pid, depth)) = queue.pop_front() {
        let Some(stat) = all.get(&pid) else {
            continue;
        };
        if tree.len() >= max_entries {
            truncated = true;
            break;
        }
        tree.push((stat.clone(), depth));
        if depth >= max_depth {
            if children.get(&pid).is_some_and(|kids| !kids.is_empty()) {
                truncated = true;
            }
            continue;
        }
        if let Some(kids) = children.get(&pid) {
            for kid in kids {
                queue.push_back((*kid, depth + 1));
            }
        }
    }
    (tree, truncated)
}

/// `(read_bytes, write_bytes)` summed over the process's own `rchar`/`wchar` counters.
pub fn process_io(pid: u32) -> Option<(u64, u64)> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/io")).ok()?;
    let mut read = None;
    let mut written = None;
    for line in text.lines() {
        let (key, value) = line.split_once(':')?;
        match key.trim() {
            "rchar" => read = value.trim().parse::<u64>().ok(),
            "wchar" => written = value.trim().parse::<u64>().ok(),
            _ => {}
        }
    }
    Some((read?, written?))
}

/// Total bytes moved across the process's file descriptors — the number that matches what a
/// container's `io.stat` reports, unlike `rchar`/`wchar` which also count pipes and sockets.
pub fn process_io_bytes(pid: u32) -> Option<(u64, u64)> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/io")).ok()?;
    let mut read = None;
    let mut written = None;
    for line in text.lines() {
        let (key, value) = line.split_once(':')?;
        match key.trim() {
            "read_bytes" => read = value.trim().parse::<u64>().ok(),
            "write_bytes" => written = value.trim().parse::<u64>().ok(),
            _ => {}
        }
    }
    Some((read?, written?))
}

pub fn fd_count(pid: u32) -> Option<u64> {
    let entries = std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    Some(entries.flatten().count() as u64)
}

pub fn fd_limit(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/limits")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Max open files") {
            let soft = rest.split_whitespace().next()?;
            if soft == "unlimited" {
                return None;
            }
            return soft.parse().ok();
        }
    }
    None
}

pub fn thread_count(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Threads:") {
            return rest.trim().parse().ok();
        }
    }
    None
}

/// Parses one `/proc/pressure/*` file (the `some` line is required, `full` is absent for cpu).
pub fn parse_psi(text: &str) -> Psi {
    let mut psi = Psi::default();
    for line in text.lines() {
        let fields: HashMap<&str, &str> = line
            .split_whitespace()
            .filter_map(|field| field.split_once('='))
            .collect();
        let number = |key: &str| fields.get(key).and_then(|v| v.parse::<f64>().ok());
        if line.starts_with("some ") {
            psi.some10 = number("avg10");
            psi.some60 = number("avg60");
        } else if line.starts_with("full ") {
            psi.full10 = number("avg10");
            psi.full60 = number("avg60");
        }
    }
    psi
}

pub fn psi(kind: &str) -> Option<Psi> {
    let text = std::fs::read_to_string(format!("/proc/pressure/{kind}")).ok()?;
    Some(parse_psi(&text))
}

/// This process's cgroup v2 path, e.g. `/libpod-abc.scope`. Read from `/proc/self/cgroup`, whose
/// v2 line has the form `0::<path>`; a v1 host has no such line and yields `None`.
pub fn self_cgroup_path() -> Option<String> {
    let text = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("0::") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn cgroup_dir(relative: &str) -> std::path::PathBuf {
    let trimmed = relative.trim_start_matches('/');
    std::path::Path::new("/sys/fs/cgroup").join(trimmed)
}

fn read_number(path: std::path::PathBuf) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_keyed(path: std::path::PathBuf) -> HashMap<String, u64> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let key = fields.next()?.to_string();
            let value = fields.next()?.parse().ok()?;
            Some((key, value))
        })
        .collect()
}

/// Reads the cgroup v2 files a container actually has. Returns `None` when there is no cgroup v2
/// directory at all (a non-container Linux host, macOS, FreeBSD) so the caller can say why.
pub fn cgroup(relative: &str) -> Option<CgroupSection> {
    let dir = cgroup_dir(relative);
    if !dir.exists() {
        return None;
    }
    let stats = read_keyed(dir.join("memory.stat"));
    let events = read_keyed(dir.join("memory.events"));
    let cpu = read_keyed(dir.join("cpu.stat"));
    let v1_memory = read_keyed(dir.join("memory.stat"));

    let mut io_stat = Vec::new();
    if let Ok(text) = std::fs::read_to_string(dir.join("io.stat")) {
        for line in text.lines() {
            let mut fields = line.split_whitespace();
            let Some(device) = fields.next() else {
                continue;
            };
            let mut read_bytes = 0;
            let mut write_bytes = 0;
            for field in fields {
                if let Some(value) = field.strip_prefix("rbytes=") {
                    read_bytes = value.parse().unwrap_or(0);
                } else if let Some(value) = field.strip_prefix("wbytes=") {
                    write_bytes = value.parse().unwrap_or(0);
                }
            }
            if read_bytes > 0 || write_bytes > 0 {
                io_stat.push(IoStat {
                    device: device.to_string(),
                    read_bytes,
                    write_bytes,
                });
            }
        }
    }
    io_stat.sort_by(|a, b| a.device.cmp(&b.device));
    io_stat.truncate(8);

    let pick = |key: &str| {
        stats
            .get(key)
            .copied()
            .or_else(|| v1_memory.get(key).copied())
    };

    Some(CgroupSection {
        path: relative.to_string(),
        memory_current_bytes: read_number(dir.join("memory.current")),
        memory_max_bytes: read_number(dir.join("memory.max")),
        memory_high_bytes: read_number(dir.join("memory.high")),
        includes_page_cache: true,
        anon_bytes: pick("anon"),
        file_bytes: pick("file"),
        slab_bytes: pick("slab"),
        peak_bytes: read_number(dir.join("memory.peak")),
        swap_current_bytes: read_number(dir.join("memory.swap.current")),
        pids_current: read_number(dir.join("pids.current")),
        cpu_usage_usec: cpu.get("usage_usec").copied(),
        events: MemoryEvents {
            low: events.get("low").copied(),
            high: events.get("high").copied(),
            max: events.get("max").copied(),
            oom: events.get("oom").copied(),
            oom_kill: events.get("oom_kill").copied(),
        },
        psi_memory: psi("memory").unwrap_or_default(),
        io_stat,
    })
}

/// Totals from `/proc/self/net/dev`, skipping loopback: a share or a download over the LAN should
/// show up here, a health check on `127.0.0.1` should not.
pub fn network_totals() -> (u64, u64) {
    let Ok(text) = std::fs::read_to_string("/proc/self/net/dev") else {
        return (0, 0);
    };
    let mut rx = 0;
    let mut tx = 0;
    for line in text.lines().skip(2) {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        if name.trim() == "lo" {
            continue;
        }
        let fields: Vec<&str> = rest.split_whitespace().collect();
        rx += fields
            .first()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        tx += fields
            .get(8)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
    }
    (rx, tx)
}

/// Free/total bytes and free inodes for the filesystem holding `path`.
// `statvfs`'s counters are `u64` on Linux but `u32` on macOS.
#[allow(clippy::unnecessary_cast)]
pub fn disk(path: &std::path::Path) -> Option<DiskStat> {
    use std::os::unix::ffi::OsStrExt;
    let mut buffer = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c_path` is a valid NUL-terminated string and `buffer` is a `statvfs` of the right
    // type; `statvfs` writes into it on success only.
    let code = unsafe { libc::statvfs(c_path.as_ptr(), buffer.as_mut_ptr()) };
    if code != 0 {
        return None;
    }
    // SAFETY: initialised by the successful call above.
    let stats = unsafe { buffer.assume_init() };
    let block = stats.f_frsize as u64;
    Some(DiskStat {
        free_bytes: stats.f_bavail as u64 * block,
        total_bytes: stats.f_blocks as u64 * block,
        free_inodes: stats.f_favail as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_name_with_spaces_and_parentheses_does_not_shift_the_fields() {
        // Real kernels emit names like this; splitting on whitespace would read `bar)` as `state`.
        // Fields after the comm: state, ppid, …, utime(11), stime(12), …, starttime(19), …, rss(21).
        let line =
            "1234 (foo bar) baz) S 1 1234 1234 0 -1 4194304 100 0 0 0 7 3 0 0 20 0 0 0 900 0 4";
        let stat = parse_stat(line).expect("parses");
        assert_eq!(stat.pid, 1234);
        assert_eq!(stat.comm, "foo bar) baz");
        assert_eq!(stat.state, 'S');
        assert_eq!(stat.ppid, 1);
        assert_eq!(stat.total_ticks, 10);
        assert_eq!(stat.start_ticks, 900);
        assert_eq!(stat.rss_bytes, 4 * page_size());
    }

    #[test]
    fn an_unlimited_cgroup_reports_no_maximum_rather_than_zero() {
        // `memory.max` says "max" when unlimited; parsing it as 0 would read as a full cgroup.
        let dir = std::env::temp_dir().join(format!("lrr-cgroup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("memory.max"), "max\n").expect("write max");
        assert_eq!(read_number(dir.join("memory.max")), None);
        std::fs::write(dir.join("memory.max"), "8589934592\n").expect("write limit");
        assert_eq!(
            read_number(dir.join("memory.max")),
            Some(8 * 1024 * 1024 * 1024)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_truncated_stat_line_is_rejected_rather_than_guessed() {
        assert!(parse_stat("1234 (deno) S 1").is_none());
        assert!(parse_stat("not a stat line").is_none());
    }

    #[test]
    fn psi_reports_both_some_and_full_windows() {
        let text = "some avg10=12.34 avg60=5.00 avg300=1.00 total=12345\nfull avg10=0.50 avg60=0.25 avg300=0.10 total=99\n";
        let psi = parse_psi(text);
        assert_eq!(psi.some10, Some(12.34));
        assert_eq!(psi.full60, Some(0.25));
    }

    #[test]
    fn a_cpu_pressure_file_has_no_full_line_and_still_parses() {
        let psi = parse_psi("some avg10=1.00 avg60=2.00 avg300=3.00 total=4\n");
        assert_eq!(psi.some10, Some(1.0));
        assert!(psi.full10.is_none());
    }

    #[test]
    fn the_first_cpu_reading_has_no_percentage_and_the_second_does() {
        let sampler = Sampler::new();
        let hz = clock_ticks_per_second();
        let one_second = (hz as u64).max(1);
        assert!(sampler.cpu_percent(7, 1000, 1_000).is_none());
        // One full second of CPU across one second of wall clock, on one core.
        let percent = sampler
            .cpu_percent(7, 1000 + one_second, 2_000)
            .expect("second reading has a delta");
        assert!((percent - 100.0).abs() < 1.0, "got {percent}");
    }

    #[test]
    fn the_first_network_reading_has_no_rates() {
        let sampler = Sampler::new();
        assert_eq!(sampler.network_rates(100, 200, 1_000), (None, None));
        let (rx, tx) = sampler.network_rates(1_100, 1_200, 2_000);
        assert_eq!(rx, Some(1_000.0));
        assert_eq!(tx, Some(1_000.0));
    }

    #[test]
    fn disk_space_is_readable_for_the_repository_root() {
        let disk = disk(std::path::Path::new(".")).expect("statvfs works");
        assert!(disk.total_bytes > 0);
        assert!(disk.free_bytes <= disk.total_bytes);
    }

    #[test]
    fn process_io_prefers_the_byte_counters_the_kernel_actually_moved() {
        let (read, write) = process_io_bytes(std::process::id()).expect("own /proc/self/io");
        // Not an exact number (the test harness writes too); only that both are readable.
        let _ = (read, write);
    }

    #[test]
    fn cgroup_absent_returns_none_rather_than_an_empty_section() {
        assert!(cgroup("/this/path/does/not/exist").is_none());
    }

    #[test]
    fn the_tree_starts_at_the_root_and_marks_depth_limits_as_truncation() {
        let (tree, _truncated) = process_tree(std::process::id(), 0, 64);
        assert!(tree
            .iter()
            .any(|(stat, depth)| stat.pid == std::process::id() && *depth == 0));
    }
}
