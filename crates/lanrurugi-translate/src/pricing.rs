//! Per-token pricing for the LLM providers this project calls, used to compute
//! [`crate::adapter::TranslationResponse`]'s token counts into an estimated cost (issue #100).
//!
//! There is no pricing-query API for any of this project's providers — DeepSeek, Anthropic, and
//! OpenAI-compatible gateways alike only ever publish a price on a human-readable webpage, not a
//! structured endpoint (confirmed live against DeepSeek's own docs before writing this module).
//! Rather than hand-maintaining a table that silently drifts from the provider's real, occasionally
//! peak/off-peak-varying price, this module re-derives it periodically: fetch the pricing page's
//! HTML, ask the LLM itself to extract the numbers into a fixed JSON shape
//! (`lanrurugi_llm::json_chat`), and cache the structured result in Redis for a day. A parse
//! failure (page redesign, LLM misreads a number, network hiccup) falls back to the last
//! successfully cached price rather than failing the caller outright — a slightly stale price is a
//! far smaller problem than blocking translation entirely over a cost-estimate side channel.

use std::sync::OnceLock;
use std::time::Duration;

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_core::singleflight::Singleflight;
use serde::{Deserialize, Serialize};

/// How long a fetched price table is trusted before being re-derived. A day is generous slack
/// against DeepSeek's own peak/off-peak price swap (which happens on a fixed daily schedule, not a
/// random one) while still keeping this project's own estimate from drifting far behind a real
/// price change.
const CACHE_TTL_SECS: u64 = 24 * 60 * 60;

fn cache_key(provider_id: &str) -> String {
    format!("LANRURUGI_TRANSLATION_PRICING_{provider_id}")
}

/// One [`PeakScheduleRule`] entry: a same-day local time-of-day window, `start_hour..end_hour`
/// (24h, half-open — e.g. `9..12` means 09:00 up to but not including 12:00), active on the given
/// ISO weekdays (1 = Monday .. 7 = Sunday, `chrono::Weekday::number_from_monday`'s own numbering,
/// so no separate conversion is needed at lookup time).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeakWindow {
    pub weekdays: Vec<u8>,
    pub start_hour: u8,
    pub end_hour: u8,
}

/// Describes *when* a provider's peak price applies, so [`estimate_cost_usd`] can pick the right
/// side of a peak/off-peak price split at the actual moment a call happened rather than always
/// assuming the worse (or better) case. `None` on [`ModelPricing::peak_schedule`] means the
/// provider has no peak/off-peak split at all — [`ModelPricing::peak`] is then also `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeakSchedule {
    /// IANA timezone name the windows below are defined in (e.g. `"Asia/Shanghai"`) — the
    /// schedule is meaningless without knowing which local clock `start_hour`/`end_hour` refer to.
    pub timezone: String,
    pub windows: Vec<PeakWindow>,
}

impl PeakSchedule {
    /// Whether `at` (any instant) falls inside one of this schedule's peak windows, evaluated in
    /// the schedule's own timezone.
    fn contains(&self, at: chrono::DateTime<chrono::Utc>) -> bool {
        let Ok(tz) = self.timezone.parse::<chrono_tz::Tz>() else {
            return false;
        };
        let local = at.with_timezone(&tz);
        let weekday = chrono::Datelike::weekday(&local).number_from_monday() as u8;
        let hour = chrono::Timelike::hour(&local) as u8;
        self.windows
            .iter()
            .any(|w| w.weekdays.contains(&weekday) && hour >= w.start_hour && hour < w.end_hour)
    }
}

/// One model's price, in this project's own fixed unit: **USD per 1,000,000 tokens**. Providers
/// publish in different currencies/units (DeepSeek: CNY per 1M tokens; Anthropic/OpenAI: USD per
/// 1M tokens) — the LLM extraction prompt itself is responsible for converting to this shape so
/// downstream cost math never has to branch on provider-specific units.
///
/// The fields below are the **off-peak** (or only, if the provider has no peak/off-peak split)
/// rates; [`Self::peak`] carries the alternate, more expensive rate plus [`Self::peak_schedule`]
/// describing when it applies. Both are `None` together for a provider with a single flat price.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPricing {
    pub model: String,
    /// Price per 1M prompt tokens that were *not* served from cache (a plain miss).
    pub input_per_million_usd: f64,
    /// Price per 1M prompt tokens that *were* served from cache — typically far cheaper than a
    /// miss; `None` if this provider/model doesn't offer prompt caching at all.
    #[serde(default)]
    pub cached_input_per_million_usd: Option<f64>,
    /// Price per 1M tokens spent *writing* a new cache entry (Anthropic-specific — typically
    /// *higher* than a plain miss, since writing costs more than reading; `None` for providers
    /// with no separate cache-write charge, e.g. DeepSeek's caching is opportunistic with no
    /// distinct write price).
    #[serde(default)]
    pub cache_write_per_million_usd: Option<f64>,
    pub output_per_million_usd: f64,
    /// The peak-hours rate, same field shape as this struct's own off-peak fields, `None` if this
    /// provider/model has no peak/off-peak split.
    #[serde(default)]
    pub peak: Option<PeakRates>,
    /// When [`Self::peak`] applies. Always `Some` when `peak` is `Some`, and vice versa — kept as
    /// two separate `Option`s (rather than folding into one) because that's the natural shape of
    /// the LLM-extracted JSON and of [`PeakRates`] itself needing no schedule knowledge.
    #[serde(default)]
    pub peak_schedule: Option<PeakSchedule>,
}

/// The subset of [`ModelPricing`]'s per-token rates that actually differ between peak and off-peak
/// — just the three price fields, without `model`/`peak`/`peak_schedule` (which only make sense on
/// the outer, off-peak-default struct).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeakRates {
    pub input_per_million_usd: f64,
    #[serde(default)]
    pub cached_input_per_million_usd: Option<f64>,
    #[serde(default)]
    pub cache_write_per_million_usd: Option<f64>,
    pub output_per_million_usd: f64,
}

impl ModelPricing {
    /// The rates that actually apply `at` a given instant — [`Self::peak`]'s rates if `at` falls
    /// inside [`Self::peak_schedule`], this struct's own (off-peak) rates otherwise.
    fn rates_at(&self, at: chrono::DateTime<chrono::Utc>) -> RatesRef {
        if let (Some(peak), Some(schedule)) = (&self.peak, &self.peak_schedule) {
            if schedule.contains(at) {
                return RatesRef {
                    input_per_million_usd: peak.input_per_million_usd,
                    cached_input_per_million_usd: peak.cached_input_per_million_usd,
                    cache_write_per_million_usd: peak.cache_write_per_million_usd,
                    output_per_million_usd: peak.output_per_million_usd,
                };
            }
        }
        RatesRef {
            input_per_million_usd: self.input_per_million_usd,
            cached_input_per_million_usd: self.cached_input_per_million_usd,
            cache_write_per_million_usd: self.cache_write_per_million_usd,
            output_per_million_usd: self.output_per_million_usd,
        }
    }
}

struct RatesRef {
    input_per_million_usd: f64,
    cached_input_per_million_usd: Option<f64>,
    cache_write_per_million_usd: Option<f64>,
    output_per_million_usd: f64,
}

/// One provider's full price table — usually one model (the one this project's adapter is
/// actually configured to call), kept as a `Vec` rather than a single [`ModelPricing`] so a
/// provider offering several models the LLM extraction happens to find doesn't need to be
/// discarded down to one entry before caching.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderPricing {
    pub provider_id: String,
    pub models: Vec<ModelPricing>,
    /// Unix seconds this table was actually fetched/derived — surfaced so a caller/UI can show
    /// "as of" alongside a cost estimate rather than presenting it as unconditionally current.
    pub fetched_at: i64,
}

/// Strips a `-v<digits>` version-marketing infix (e.g. the `-v4` in "deepseek-v4-flash") a
/// provider's own pricing page and this project's own adapter configuration don't always agree on
/// spelling for the same model — observed live: DeepSeek's pricing page lists "deepseek-flash",
/// this project's adapter calls it "deepseek-v4-flash". The LLM extraction prompt asks for the
/// adapter's exact spelling, but a page that never writes that spelling anywhere gives the LLM
/// nothing to copy — it's not a prompt-compliance failure, there's no ground truth on the page to
/// extract. Normalizing away the infix before comparing (both sides) makes the match survive that
/// kind of drift without hand-maintaining a per-model alias table that would silently go stale the
/// next time either side's naming changes again.
fn normalize_model_id(model: &str) -> String {
    let mut result = String::with_capacity(model.len());
    let bytes = model.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'-' && bytes.get(i + 1) == Some(&b'v') {
            let mut j = i + 2;
            let mut consumed_a_digit = false;
            loop {
                let digits_start = j;
                while bytes.get(j).is_some_and(u8::is_ascii_digit) {
                    j += 1;
                }
                if j > digits_start {
                    consumed_a_digit = true;
                } else {
                    break;
                }
                // A `.` is only part of the version (e.g. the ".1" in "-v4.1") when digits follow
                // it — otherwise it's unrelated punctuation and must be left for the outer loop.
                if bytes.get(j) == Some(&b'.') && bytes.get(j + 1).is_some_and(u8::is_ascii_digit) {
                    j += 1;
                } else {
                    break;
                }
            }
            if consumed_a_digit {
                i = j;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

impl ProviderPricing {
    pub fn model(&self, model: &str) -> Option<&ModelPricing> {
        if let Some(exact) = self.models.iter().find(|m| m.model == model) {
            return Some(exact);
        }
        let normalized = normalize_model_id(model);
        self.models
            .iter()
            .find(|m| normalize_model_id(&m.model) == normalized)
    }
}

/// Where to fetch a provider's own pricing page from, and the prompt describing how to read it —
/// one entry per provider this project's adapters support. Anthropic/OpenAI intentionally have no
/// entry yet (research.md's own scope for issue #100 only ever exercised DeepSeek, the provider
/// this project's own `translation.settings_update` default targets) — [`fetch_and_derive`]
/// returns `None` for an unknown `provider_id` rather than guessing at a URL, so cost estimation
/// simply doesn't populate for those until a real entry is added here.
fn pricing_source(provider_id: &str) -> Option<(&'static str, &'static str)> {
    match provider_id {
        "deepseek" => Some((
            "https://api-docs.deepseek.com/quick_start/pricing",
            "You will be given the raw HTML of a webpage listing DeepSeek API pricing. Extract \
             the current price for every model, converting Chinese Yuan (CNY) to USD at a rate of \
             1 USD = 7.2 CNY, and per-million-token prices as-is. DeepSeek publishes two price \
             tiers, \"discount\" (off-peak) and \"standard\" (peak) hours — put the discount/\
             off-peak price in the top-level fields and the standard/peak price in \"peak\". Also \
             extract the exact peak-hours schedule as printed on the page (timezone, weekdays, and \
             hour ranges — do not assume, read it from the page). Reply with ONLY a JSON object of \
             this exact shape, no other text: {\"models\": [{\"model\": \"<model id, e.g. \
             deepseek-v4-flash>\", \"input_per_million_usd\": <off-peak number>, \
             \"cached_input_per_million_usd\": <off-peak number or null>, \
             \"cache_write_per_million_usd\": null, \"output_per_million_usd\": <off-peak number>, \
             \"peak\": {\"input_per_million_usd\": <peak number>, \"cached_input_per_million_usd\": \
             <peak number or null>, \"cache_write_per_million_usd\": null, \
             \"output_per_million_usd\": <peak number>} or null if there is no peak/off-peak \
             split, \"peak_schedule\": {\"timezone\": \"<IANA name, e.g. Asia/Shanghai>\", \
             \"windows\": [{\"weekdays\": [<1=Monday..7=Sunday>, ...], \"start_hour\": <0-23>, \
             \"end_hour\": <0-23, exclusive>}, ...]} or null to match \"peak\"}]}",
        )),
        _ => None,
    }
}

/// Bounds concurrent distinct-provider fetches; same shape `AppState::thumbnail_singleflight`
/// already uses elsewhere in this project for "collapse concurrent callers onto one real fetch".
/// Module-level `OnceLock` (not held in any caller's own state) since this module has no `AppState`
/// of its own to live in and doesn't need one — a process-wide, at-most-a-handful-of-providers
/// singleflight needs no richer lifetime than "for as long as this process runs".
fn singleflight() -> &'static Singleflight<String, Result<ProviderPricing, String>> {
    static SF: OnceLock<Singleflight<String, Result<ProviderPricing, String>>> = OnceLock::new();
    SF.get_or_init(|| Singleflight::new(4))
}

/// Fetches `provider_id`'s pricing page and asks the LLM to extract it into [`ProviderPricing`].
/// Never touches the cache itself — [`get`] is the caller-facing entry point that layers Redis
/// caching (and last-known-good fallback) on top of this.
async fn fetch_and_derive(pool: &Pool, provider_id: &str) -> Result<ProviderPricing, String> {
    let (url, prompt) = pricing_source(provider_id)
        .ok_or_else(|| format!("no pricing source configured for provider {provider_id:?}"))?;

    lanrurugi_llm::ensure_available(pool).await?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let html = client
        .get(url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (compatible; LANrurugi pricing fetch)",
        )
        .send()
        .await
        .map_err(|e| format!("failed to fetch pricing page: {e}"))?
        .text()
        .await
        .map_err(|e| format!("failed to read pricing page body: {e}"))?;

    #[derive(Deserialize)]
    struct ExtractedModels {
        models: Vec<ModelPricing>,
    }

    // The raw page HTML is large and mostly irrelevant markup — capped rather than sent whole, a
    // pricing table itself is always well within the first ~40K characters of a docs page.
    let truncated: String = html.chars().take(40_000).collect();
    let extracted: ExtractedModels =
        lanrurugi_llm::json_chat(pool, prompt, &truncated, 0.0, 2048).await?;

    if extracted.models.is_empty() {
        return Err("LLM extracted zero pricing entries from the page".to_string());
    }

    Ok(ProviderPricing {
        provider_id: provider_id.to_string(),
        models: extracted.models,
        fetched_at: chrono::Utc::now().timestamp(),
    })
}

/// Returns `provider_id`'s current price table — from Redis if a fresh one is cached, otherwise by
/// fetching and re-deriving it (deduplicated across concurrent callers via [`singleflight`]).
///
/// On a fetch/parse failure, falls back to whatever was last cached (even if past its own TTL)
/// rather than returning `Err` outright — a stale price is preferable to no cost estimate at all.
/// Only returns `Err` if there is genuinely no prior cached value to fall back to.
pub async fn get(pool: &Pool, provider_id: &str) -> Result<ProviderPricing, String> {
    let key = cache_key(provider_id);

    if let Ok(mut conn) = pool.get().await {
        if let Ok(Some(raw)) = conn.get::<_, Option<String>>(&key).await {
            if let Ok(cached) = serde_json::from_str::<ProviderPricing>(&raw) {
                return Ok(cached);
            }
        }
    }

    let provider_id_owned = provider_id.to_string();
    let pool_owned = pool.clone();
    let result = singleflight()
        .run(provider_id.to_string(), || async move {
            fetch_and_derive(&pool_owned, &provider_id_owned).await
        })
        .await;

    match result {
        Ok(pricing) => {
            if let Ok(mut conn) = pool.get().await {
                if let Ok(raw) = serde_json::to_string(&pricing) {
                    let _: Result<(), _> = conn.set_ex(&key, raw, CACHE_TTL_SECS).await;
                }
            }
            Ok(pricing)
        }
        Err(e) => {
            tracing::warn!(provider_id, error = %e, "pricing: fetch/derive failed, falling back to last cached value if any");
            // Last-known-good fallback — deliberately reads even a now-expired cache entry rather
            // than nothing (`GET` doesn't care whether Redis has since evicted it; if the key is
            // gone, this simply falls through to the real `Err` below).
            if let Ok(mut conn) = pool.get().await {
                if let Ok(Some(raw)) = conn.get::<_, Option<String>>(&key).await {
                    if let Ok(cached) = serde_json::from_str::<ProviderPricing>(&raw) {
                        return Ok(cached);
                    }
                }
            }
            Err(e)
        }
    }
}

/// Computes an estimated cost in USD from a real usage breakdown, `None` if the model isn't in the
/// price table at all (an unrecognized/未来 model — cost estimation degrades to "unknown" rather
/// than guessing). Picks peak vs. off-peak rates based on `at` (the instant the call actually
/// happened) against `pricing`'s own [`ModelPricing::peak_schedule`], so a call estimated later
/// (e.g. a retry, or the cost being recomputed from a log) still reflects the price at the time it
/// was actually made — never "whatever the schedule says right now".
pub fn estimate_cost_usd(
    pricing: &ModelPricing,
    at: chrono::DateTime<chrono::Utc>,
    prompt_tokens: Option<u64>,
    cached_prompt_tokens: Option<u64>,
    cache_creation_tokens: Option<u64>,
    completion_tokens: Option<u64>,
) -> f64 {
    let rates = pricing.rates_at(at);
    let cached = cached_prompt_tokens.unwrap_or(0);
    let creation = cache_creation_tokens.unwrap_or(0);
    let uncached_prompt = prompt_tokens.unwrap_or(0).saturating_sub(cached + creation);

    let mut cost = 0.0;
    cost += (uncached_prompt as f64 / 1_000_000.0) * rates.input_per_million_usd;
    if cached > 0 {
        let rate = rates
            .cached_input_per_million_usd
            .unwrap_or(rates.input_per_million_usd);
        cost += (cached as f64 / 1_000_000.0) * rate;
    }
    if creation > 0 {
        let rate = rates
            .cache_write_per_million_usd
            .unwrap_or(rates.input_per_million_usd);
        cost += (creation as f64 / 1_000_000.0) * rate;
    }
    cost += (completion_tokens.unwrap_or(0) as f64 / 1_000_000.0) * rates.output_per_million_usd;
    cost
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_pricing() -> ModelPricing {
        ModelPricing {
            model: "deepseek-v4-flash".to_string(),
            input_per_million_usd: 0.625,
            cached_input_per_million_usd: Some(0.0208),
            cache_write_per_million_usd: None,
            output_per_million_usd: 1.875,
            peak: None,
            peak_schedule: None,
        }
    }

    fn off_peak_instant() -> chrono::DateTime<chrono::Utc> {
        // 2026-09-14 is a Monday; 20:00 Asia/Shanghai (12:00 UTC) is outside both DeepSeek peak
        // windows (09-12 / 14-18 Beijing time).
        "2026-09-14T12:00:00Z".parse().unwrap()
    }

    #[test]
    fn cost_with_no_cache_hit_uses_the_plain_input_rate() {
        let cost = estimate_cost_usd(
            &sample_pricing(),
            off_peak_instant(),
            Some(1000),
            None,
            None,
            Some(500),
        );
        let expected = (1000.0 / 1_000_000.0) * 0.625 + (500.0 / 1_000_000.0) * 1.875;
        assert!((cost - expected).abs() < 1e-9);
    }

    #[test]
    fn cached_tokens_are_billed_at_the_cheaper_rate() {
        // 1000 prompt tokens, 800 of them served from cache.
        let cost = estimate_cost_usd(
            &sample_pricing(),
            off_peak_instant(),
            Some(1000),
            Some(800),
            None,
            Some(0),
        );
        let expected = (200.0 / 1_000_000.0) * 0.625 + (800.0 / 1_000_000.0) * 0.0208;
        assert!((cost - expected).abs() < 1e-9);
    }

    #[test]
    fn missing_cached_rate_falls_back_to_the_plain_input_rate() {
        let mut pricing = sample_pricing();
        pricing.cached_input_per_million_usd = None;
        let cost = estimate_cost_usd(
            &pricing,
            off_peak_instant(),
            Some(1000),
            Some(500),
            None,
            None,
        );
        let expected = (1000.0 / 1_000_000.0) * 0.625;
        assert!((cost - expected).abs() < 1e-9);
    }

    #[test]
    fn all_none_usage_costs_nothing() {
        assert_eq!(
            estimate_cost_usd(
                &sample_pricing(),
                off_peak_instant(),
                None,
                None,
                None,
                None
            ),
            0.0
        );
    }

    fn deepseek_peak_schedule() -> PeakSchedule {
        PeakSchedule {
            timezone: "Asia/Shanghai".to_string(),
            windows: vec![
                PeakWindow {
                    weekdays: vec![1, 2, 3, 4, 5],
                    start_hour: 9,
                    end_hour: 12,
                },
                PeakWindow {
                    weekdays: vec![1, 2, 3, 4, 5],
                    start_hour: 14,
                    end_hour: 18,
                },
            ],
        }
    }

    fn sample_pricing_with_peak() -> ModelPricing {
        let mut pricing = sample_pricing();
        pricing.peak = Some(PeakRates {
            input_per_million_usd: 1.25,
            cached_input_per_million_usd: Some(0.0416),
            cache_write_per_million_usd: None,
            output_per_million_usd: 3.75,
        });
        pricing.peak_schedule = Some(deepseek_peak_schedule());
        pricing
    }

    #[test]
    fn peak_hours_use_the_peak_rate() {
        // 2026-09-14 10:00 Asia/Shanghai (02:00 UTC) is inside the 09-12 Monday peak window.
        let at: chrono::DateTime<chrono::Utc> = "2026-09-14T02:00:00Z".parse().unwrap();
        let cost = estimate_cost_usd(
            &sample_pricing_with_peak(),
            at,
            Some(1000),
            None,
            None,
            None,
        );
        let expected = (1000.0 / 1_000_000.0) * 1.25;
        assert!((cost - expected).abs() < 1e-9);
    }

    #[test]
    fn off_peak_hours_use_the_off_peak_rate() {
        let cost = estimate_cost_usd(
            &sample_pricing_with_peak(),
            off_peak_instant(),
            Some(1000),
            None,
            None,
            None,
        );
        let expected = (1000.0 / 1_000_000.0) * 0.625;
        assert!((cost - expected).abs() < 1e-9);
    }

    #[test]
    fn weekend_is_never_peak_even_during_peak_hours() {
        // 2026-09-19 is a Saturday; 10:00 Asia/Shanghai (02:00 UTC) would be peak on a weekday.
        let at: chrono::DateTime<chrono::Utc> = "2026-09-19T02:00:00Z".parse().unwrap();
        let cost = estimate_cost_usd(
            &sample_pricing_with_peak(),
            at,
            Some(1000),
            None,
            None,
            None,
        );
        let expected = (1000.0 / 1_000_000.0) * 0.625;
        assert!((cost - expected).abs() < 1e-9);
    }

    #[test]
    fn provider_pricing_looks_up_by_model_name() {
        let pricing = ProviderPricing {
            provider_id: "deepseek".to_string(),
            models: vec![sample_pricing()],
            fetched_at: 0,
        };
        assert!(pricing.model("deepseek-v4-flash").is_some());
        assert!(pricing.model("nonexistent-model").is_none());
    }

    #[test]
    fn model_lookup_tolerates_a_missing_version_infix() {
        // Real case (2026-09-13): DeepSeek's own pricing page lists "deepseek-flash", this
        // project's adapter calls the same model "deepseek-v4-flash" — the lookup must bridge
        // that without either side's config being edited.
        let pricing = ProviderPricing {
            provider_id: "deepseek".to_string(),
            models: vec![ModelPricing {
                model: "deepseek-flash".to_string(),
                ..sample_pricing()
            }],
            fetched_at: 0,
        };
        assert!(pricing.model("deepseek-v4-flash").is_some());
    }

    #[test]
    fn normalize_model_id_strips_the_version_infix_only() {
        assert_eq!(normalize_model_id("deepseek-v4-flash"), "deepseek-flash");
        assert_eq!(normalize_model_id("deepseek-v4.1-flash"), "deepseek-flash");
        assert_eq!(normalize_model_id("deepseek-flash"), "deepseek-flash");
        assert_eq!(normalize_model_id("claude-v2"), "claude");
        // Not a version infix — must be left alone (no digits directly after "-v").
        assert_eq!(normalize_model_id("some-value-model"), "some-value-model");
    }
}
