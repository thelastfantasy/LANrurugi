//! Benchmark report types (T087), matching `contracts/benchmark-report.md`'s shape exactly for
//! the required fields, plus one additive extension (`interactive_load`, T088/SC-008) — additive
//! fields beyond a contract's documented minimum are permitted per this project's
//! additive-only-extension convention (`research.md` #12).

use super::interactive_load::InteractiveLoadResult;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LibraryScale {
    pub archive_count: u64,
    pub total_size_bytes: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Hardware {
    pub cpu_cores: u32,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    FullLibraryScanIngestion,
    DuplicateRepairReindex,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemResult {
    pub wall_clock_seconds: f64,
    pub throughput_archives_per_second: f64,
}

impl SystemResult {
    pub fn new(wall_clock: std::time::Duration, archive_count: u64) -> Self {
        let wall_clock_seconds = wall_clock.as_secs_f64();
        let throughput_archives_per_second = if wall_clock_seconds > 0.0 {
            archive_count as f64 / wall_clock_seconds
        } else {
            0.0
        };
        SystemResult {
            wall_clock_seconds,
            throughput_archives_per_second,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OperationResult {
    pub operation: OperationKind,
    pub legacy: SystemResult,
    pub new: SystemResult,
    /// `legacy.wall_clock_seconds / new.wall_clock_seconds` — greater than 1.0 means the new
    /// system is faster. Per the contract, this MUST NOT be presented elsewhere as a fixed
    /// promised number: it's whatever this specific run measured, nothing more.
    pub speedup_factor: f64,
}

impl OperationResult {
    pub fn new(operation: OperationKind, legacy: SystemResult, new: SystemResult) -> Self {
        // Single-core-host edge case (spec.md Edge Cases): `new`'s wall clock may be within
        // noise of `legacy`'s rather than dramatically faster — the report is still produced
        // with whatever ratio that implies, not suppressed or clamped to a "should" value.
        let speedup_factor = if new.wall_clock_seconds > 0.0 {
            legacy.wall_clock_seconds / new.wall_clock_seconds
        } else {
            0.0
        };
        OperationResult {
            operation,
            legacy,
            new,
            speedup_factor,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BenchmarkReport {
    pub report_id: String,
    pub generated_at: String,
    pub library_scale: LibraryScale,
    pub hardware: Hardware,
    pub operations: Vec<OperationResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interactive_load: Option<InteractiveLoadResult>,
}
