//! LLM-backed translation for Phase 2's on-page manga translation
//! (`specs/004-ocr-manga-translation`).
//!
//! Holds the provider adapters ([`adapter`], [`openai_compat`], [`anthropic`], [`deepseek`]),
//! server-side credential/settings handling, the Terminology Glossary and its request-context
//! assembly, the re-derivable rendered-page cache, usage budgeting, and server-side compositing.
//!
//! Trust boundary (constitution Principle V): every cloud provider call is made from here,
//! server-side, so a provider credential never reaches the browser. The locally-hosted-backend path
//! deliberately does NOT route through this crate — the browser calls the user's own loopback model
//! directly, and only reports the result back for glossary capture.

pub mod adapter;
pub mod anthropic;
pub mod budget;
pub mod cache;
pub mod composite;
pub mod context_assembly;
pub mod credentials;
pub mod deepseek;
pub mod fonts;
pub mod glossary;
mod llm_prompts;
pub mod openai_compat;
pub mod pipeline;
pub mod prefetch;
pub mod pricing;
pub mod regions;
pub mod settings;
pub mod telemetry;

pub use adapter::{
    BlockId, TranslatedBlock, TranslationAdapter, TranslationBlock, TranslationContext,
    TranslationError, TranslationRequest, TranslationResponse,
};
pub use anthropic::AnthropicAdapter;
pub use budget::{BudgetRepository, UsageSnapshot};
pub use cache::{TranslationCacheKey, TranslationImageCache};
pub use composite::{
    encode_webp, finish_composite_page, prepare_page_erase, FontSet, PageErasePlan,
};
pub use credentials::{CredentialRef, CredentialStore};
pub use deepseek::DeepSeekAdapter;
pub use fonts::FontLibrary;
pub use glossary::{GlossaryRepository, TerminologyGlossary};
pub use openai_compat::OpenAiCompatAdapter;
pub use pipeline::{translate_batch, BatchOutcome, PageWork, UsageInfo};
pub use prefetch::{window_pages, PageState, PrefetchScheduler};
pub use pricing::{estimate_cost_usd, ModelPricing, ProviderPricing};
pub use regions::{RegionRepository, RegionStorageError};
pub use settings::{
    BackendCategory, CloudProvider, TranslationSettings, TranslationSettingsRepository,
};
pub use telemetry::TranslationTelemetry;
