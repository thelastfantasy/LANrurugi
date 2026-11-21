//! Which memory tier image work runs in (#332): chosen from the memory budget at startup, and
//! forced to the low tier by the `force_low_memory` setting. Applied to the thumbnail and page
//! pools while they run, so changing the setting takes effect without a restart.

use std::sync::{Mutex, OnceLock};

use lanrurugi_core::memory::{BudgetSource, ImageWorkPlan, MemoryBudget, MemoryTier, TierReason};
use serde_json::{json, Value};

use crate::state::AppState;

/// Read once: a container's limit only changes when the container is recreated.
fn budget() -> Option<MemoryBudget> {
    static BUDGET: OnceLock<Option<MemoryBudget>> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        let budget = lanrurugi_core::memory::detect_budget();
        if budget.is_none() {
            tracing::warn!("memory budget could not be read; image work stays in the normal tier");
        }
        budget
    })
}

/// The plan in effect, for `GET /settings`.
static CURRENT: Mutex<Option<ImageWorkPlan>> = Mutex::new(None);

/// Chooses the tier from the budget and the stored setting and applies its limits to the running
/// pools. Called at startup and whenever `force_low_memory` is saved.
pub async fn apply(state: &AppState) -> ImageWorkPlan {
    let forced = crate::settings::read_force_low_memory(state).await;
    let plan = lanrurugi_core::memory::plan_image_work(
        budget(),
        forced,
        lanrurugi_core::concurrency::thumbnail_concurrency(),
        lanrurugi_core::concurrency::page_concurrency(),
    );
    state
        .thumbnail_singleflight
        .limit()
        .set_limit(plan.thumbnails);
    state.page_singleflight.limit().set_limit(plan.pages);
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(plan);
    tracing::info!(
        tier = ?plan.tier,
        reason = ?plan.reason,
        budget_mib = plan.budget.map(|b| b.bytes >> 20),
        thumbnails = plan.thumbnails,
        pages = plan.pages,
        "memory mode"
    );
    plan
}

/// `memory_mode` in `GET /settings`: the tier in effect and what decided it.
pub fn current_json() -> Value {
    let Some(plan) = *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) else {
        return Value::Null;
    };
    json!({
        "effective": match plan.tier {
            MemoryTier::Normal => "normal",
            MemoryTier::Low => "low",
        },
        "reason": match plan.reason {
            TierReason::Forced => "forced",
            TierReason::SmallBudget => "small_budget",
            TierReason::LargeBudget => "large_budget",
            TierReason::Unknown => "unknown",
        },
        "budget_bytes": plan.budget.map(|b| b.bytes),
        "budget_source": plan.budget.map(|b| match b.source {
            BudgetSource::CgroupV2 | BudgetSource::CgroupV1 => "container",
            BudgetSource::MemTotal => "machine",
        }),
        "thumbnail_concurrency": plan.thumbnails,
        "page_concurrency": plan.pages,
    })
}
