//! Process memory: what it holds (for the logs) and giving freed heap back to the OS (#330).
//!
//! Decoding a page allocates tens of megabytes at once; once a burst of covers or pages is done the
//! allocator keeps those pages mapped, so the process stayed at its peak RSS for good (2.5 GiB
//! resident on a 7,677-archive library, almost all of it free heap). [`release_free_heap`] hands it
//! back; [`release_free_heap_soon`] does so once image work has stayed idle for a moment.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// How long image work has to stay idle before freed heap is handed back: long enough that a
/// homepage's covers or a reader's prefetch window count as one burst, short enough that the
/// memory comes back while it still matters.
const IDLE_BEFORE_RELEASE: Duration = Duration::from_secs(3);

/// Bumped by every request to release; a scheduled release only runs if it is still the latest.
static RELEASE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Returns free heap to the OS now. `malloc_trim` on glibc (every arena since glibc 2.8); a no-op
/// on other platforms, whose allocators already return memory on their own or offer no such call.
pub fn release_free_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: `malloc_trim` only releases memory the allocator already considers free.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// [`release_free_heap`] after [`IDLE_BEFORE_RELEASE`], unless more work asked for it in between
/// (the latest request wins), then logs what the process holds. Needs a Tokio runtime; without one
/// it does nothing.
pub fn release_free_heap_soon(reason: &'static str) {
    let generation = RELEASE_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async move {
        tokio::time::sleep(IDLE_BEFORE_RELEASE).await;
        if RELEASE_GENERATION.load(Ordering::Relaxed) != generation {
            return;
        }
        let before = snapshot();
        tokio::task::spawn_blocking(release_free_heap).await.ok();
        let after = snapshot();
        if let (Some(before), Some(after)) = (before, after) {
            tracing::info!(
                reason,
                rss_before_mib = before.rss_kib / 1024,
                rss_after_mib = after.rss_kib / 1024,
                peak_rss_mib = after.peak_rss_kib / 1024,
                anon_huge_mib = after.anon_huge_kib / 1024,
                "memory: returned free heap to the OS"
            );
        }
    });
}

/// What the process holds, in KiB, from `/proc/self/status` and `/proc/self/smaps_rollup`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySnapshot {
    /// Resident now (`VmRSS`).
    pub rss_kib: u64,
    /// The highest resident size since start (`VmHWM`).
    pub peak_rss_kib: u64,
    /// Transparent huge pages backing anonymous memory (`AnonHugePages`); 0 when unknown.
    pub anon_huge_kib: u64,
}

/// `None` off Linux, or when `/proc` cannot be read.
pub fn snapshot() -> Option<MemorySnapshot> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let rollup = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
    Some(MemorySnapshot {
        rss_kib: kib_field(&status, "VmRSS:")?,
        peak_rss_kib: kib_field(&status, "VmHWM:")?,
        anon_huge_kib: kib_field(&rollup, "AnonHugePages:").unwrap_or(0),
    })
}

/// Logs [`snapshot`] once, e.g. at startup.
pub fn log_snapshot(context: &'static str) {
    if let Some(snapshot) = snapshot() {
        tracing::info!(
            context,
            rss_mib = snapshot.rss_kib / 1024,
            peak_rss_mib = snapshot.peak_rss_kib / 1024,
            anon_huge_mib = snapshot.anon_huge_kib / 1024,
            "memory"
        );
    }
}

/// The value of a `Name:   1234 kB` line.
fn kib_field(text: &str, name: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(name))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse().ok())
}

/// Where the memory budget came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetSource {
    /// A container limit (`memory.max`, cgroup v2).
    CgroupV2,
    /// A container limit (`memory.limit_in_bytes`, cgroup v1).
    CgroupV1,
    /// The machine's own memory (`MemTotal`): no container limit applies.
    MemTotal,
}

/// How much memory this process may count on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryBudget {
    pub bytes: u64,
    pub source: BudgetSource,
}

/// The smaller of the container's memory limit and the machine's memory. `None` when neither can
/// be read (not Linux, or `/proc`/`/sys` hidden).
pub fn detect_budget() -> Option<MemoryBudget> {
    let read = |path: &str| std::fs::read_to_string(path).ok();
    budget_from(
        read("/sys/fs/cgroup/memory.max").as_deref(),
        read("/sys/fs/cgroup/memory/memory.limit_in_bytes").as_deref(),
        read("/proc/meminfo").as_deref(),
    )
}

fn budget_from(
    cgroup_v2: Option<&str>,
    cgroup_v1: Option<&str>,
    meminfo: Option<&str>,
) -> Option<MemoryBudget> {
    let total = meminfo
        .and_then(|text| kib_field(text, "MemTotal:"))
        .map(|kib| MemoryBudget {
            bytes: kib * 1024,
            source: BudgetSource::MemTotal,
        });
    // "max" (v2) and the near-`i64::MAX` page-rounded value (v1) both mean "no limit".
    let limit = |text: Option<&str>, source| {
        text.and_then(|t| t.trim().parse::<u64>().ok())
            .filter(|&bytes| bytes > 0 && bytes < 1 << 60)
            .map(|bytes| MemoryBudget { bytes, source })
    };
    let container = limit(cgroup_v2, BudgetSource::CgroupV2)
        .or_else(|| limit(cgroup_v1, BudgetSource::CgroupV1));
    match (container, total) {
        (Some(container), Some(total)) if total.bytes < container.bytes => Some(total),
        (Some(container), _) => Some(container),
        (None, total) => total,
    }
}

/// How hard image work may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryTier {
    Normal,
    /// Fewer decodes at once; images load slower, memory stays low.
    Low,
}

/// Why [`plan_image_work`] chose its tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierReason {
    /// The "force low-memory mode" setting.
    Forced,
    /// The budget is at most [`LOW_MEMORY_BUDGET`].
    SmallBudget,
    /// The budget is larger.
    LargeBudget,
    /// No budget could be read; the normal tier is the safe default.
    Unknown,
}

/// The image-work limits to run with, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageWorkPlan {
    pub tier: MemoryTier,
    pub reason: TierReason,
    pub budget: Option<MemoryBudget>,
    pub thumbnails: usize,
    pub pages: usize,
}

/// At or under this budget (a 2–4 GB box, or a container capped that low) the low tier is chosen
/// on its own.
pub const LOW_MEMORY_BUDGET: u64 = 4 << 30;
/// What one full-size page decode holds at its peak (a 3000×4200 scan: the decoded frame plus the
/// resize and encoder copies), measured with `scripts/memory-baseline.sh`.
const BYTES_PER_PAGE_DECODE: u64 = 150 << 20;
/// Decodes the low tier allows at once, for thumbnails and pages each.
const LOW_TIER_CONCURRENCY: usize = 2;

/// Chooses the image-work limits for `budget`. `normal_thumbnails`/`normal_pages` are the limits
/// the normal tier starts from (cores, or an operator's environment override). Pure, so the rules
/// are tested without a machine of each size.
pub fn plan_image_work(
    budget: Option<MemoryBudget>,
    force_low: bool,
    normal_thumbnails: usize,
    normal_pages: usize,
) -> ImageWorkPlan {
    let reason = if force_low {
        TierReason::Forced
    } else {
        match budget {
            None => TierReason::Unknown,
            Some(b) if b.bytes <= LOW_MEMORY_BUDGET => TierReason::SmallBudget,
            Some(_) => TierReason::LargeBudget,
        }
    };
    let low = matches!(reason, TierReason::Forced | TierReason::SmallBudget);
    let (thumbnails, pages) = if low {
        (
            normal_thumbnails.min(LOW_TIER_CONCURRENCY),
            normal_pages.min(LOW_TIER_CONCURRENCY),
        )
    } else {
        // A quarter of the budget for decoded pages; the rest stays with Redis, scans and
        // whatever else the machine runs.
        let by_budget = budget
            .map(|b| (b.bytes / 4 / BYTES_PER_PAGE_DECODE) as usize)
            .unwrap_or(usize::MAX);
        (normal_thumbnails, normal_pages.min(by_budget).max(1))
    };
    ImageWorkPlan {
        tier: if low {
            MemoryTier::Low
        } else {
            MemoryTier::Normal
        },
        reason,
        budget,
        thumbnails: thumbnails.max(1),
        pages: pages.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    fn budget(bytes: u64) -> Option<MemoryBudget> {
        Some(MemoryBudget {
            bytes,
            source: BudgetSource::MemTotal,
        })
    }

    #[test]
    fn budget_is_the_smaller_of_the_container_limit_and_the_machine() {
        let meminfo = "MemTotal:       65011712 kB\nMemFree:  1 kB\n";
        // A 2 GiB container on a 62 GiB machine.
        assert_eq!(
            budget_from(Some("2147483648\n"), None, Some(meminfo)),
            Some(MemoryBudget {
                bytes: 2 * GIB,
                source: BudgetSource::CgroupV2
            })
        );
        // "max" means no limit: the machine counts.
        assert_eq!(
            budget_from(Some("max\n"), None, Some(meminfo)),
            Some(MemoryBudget {
                bytes: 65_011_712 * 1024,
                source: BudgetSource::MemTotal
            })
        );
        // cgroup v1's "unlimited" is a huge page-rounded number.
        assert_eq!(
            budget_from(None, Some("9223372036854771712\n"), Some(meminfo)).map(|b| b.source),
            Some(BudgetSource::MemTotal)
        );
        assert_eq!(
            budget_from(None, Some("1073741824"), Some(meminfo)).map(|b| b.source),
            Some(BudgetSource::CgroupV1)
        );
        // A limit above the machine's memory is no limit at all.
        assert_eq!(
            budget_from(Some("137438953472"), None, Some(meminfo)).map(|b| b.source),
            Some(BudgetSource::MemTotal)
        );
        assert_eq!(budget_from(None, None, None), None);
    }

    #[test]
    fn plans_image_work_by_budget_and_the_setting() {
        // 32 cores / 62 GB, nothing forced: the normal limits, untouched.
        let big = plan_image_work(budget(62 * GIB), false, 4, 8);
        assert_eq!(
            (big.tier, big.reason, big.thumbnails, big.pages),
            (MemoryTier::Normal, TierReason::LargeBudget, 4, 8)
        );

        // A 2 GB container: low tier on its own.
        let small = plan_image_work(budget(2 * GIB), false, 4, 8);
        assert_eq!(
            (small.tier, small.reason, small.thumbnails, small.pages),
            (MemoryTier::Low, TierReason::SmallBudget, 2, 2)
        );

        // Forced on a big machine, and forced on a small one: the same low tier, not stacked.
        let forced = plan_image_work(budget(62 * GIB), true, 4, 8);
        assert_eq!(
            (forced.tier, forced.reason, forced.thumbnails, forced.pages),
            (MemoryTier::Low, TierReason::Forced, 2, 2)
        );
        let forced_small = plan_image_work(budget(2 * GIB), true, 4, 8);
        assert_eq!((forced_small.thumbnails, forced_small.pages), (2, 2));

        // Unreadable budget: normal tier, never a failure.
        let unknown = plan_image_work(None, false, 4, 8);
        assert_eq!(
            (unknown.tier, unknown.reason, unknown.pages),
            (MemoryTier::Normal, TierReason::Unknown, 8)
        );

        // Just over the threshold: normal tier, but pages still fit a quarter of the budget
        // (5 GiB / 4 / 150 MiB = 8).
        let medium = plan_image_work(budget(5 * GIB), false, 4, 16);
        assert_eq!((medium.tier, medium.pages), (MemoryTier::Normal, 8));

        // An operator's lower override is never raised, and nothing drops under one.
        let override_low = plan_image_work(budget(62 * GIB), false, 1, 1);
        assert_eq!((override_low.thumbnails, override_low.pages), (1, 1));
    }

    #[test]
    fn reads_kib_fields_from_proc_text() {
        let status = "Name:\tlanrurugi\nVmHWM:\t 5976116 kB\nVmRSS:\t 2611200 kB\n";
        assert_eq!(kib_field(status, "VmHWM:"), Some(5_976_116));
        assert_eq!(kib_field(status, "VmRSS:"), Some(2_611_200));
        assert_eq!(kib_field(status, "VmSwap:"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn snapshots_this_process() {
        let snapshot = snapshot().expect("/proc/self is readable on Linux");
        assert!(snapshot.rss_kib > 0);
        assert!(snapshot.peak_rss_kib >= snapshot.rss_kib);
        release_free_heap();
    }
}
