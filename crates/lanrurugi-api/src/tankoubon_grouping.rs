//! `POST /api/tankoubons/ai-group-suggestions` — analyzes every archive not currently a member of
//! any Tankoubon and suggests groups of them that likely belong together as one, so the Categories
//! page's "AI grouping suggestions" modal can offer "these N archives look like the same series —
//! want to make them a Tankoubon?" without the user having to notice the pattern by hand first.
//!
//! Deliberately local-model-only, no LLM call — this is a title/tag *similarity* judgment (do
//! these two archives look like the same series?), the exact kind of semantic-clustering task the
//! embedding model already handles well (issue #70's `eval_fixture.rs` measured 97.9% top-1
//! same-series accuracy against a real-title fixture using title embeddings alone). An LLM call
//! per archive pair would also be O(n²) API calls for a step that doesn't need an LLM's actual
//! strengths (instruction-following, next-volume ordering) — this endpoint only asks "same series,
//! yes or no", not "what order do these go in" (that's `ai_rename_suggestions`' job, once a group
//! has already become a real Tankoubon). Nothing here requires `hasLlmKey`/`llm_api_key`; the only
//! precondition is the local embedding model being loaded (`state.recommender.ready()`).
//!
//! Similarity score is a 50/50 blend of two signals, not title embedding alone: two archives whose
//! *tags* overlap heavily but whose titles differ a lot (a single-story doujin re-released across
//! different anthology issues, say) are exactly the case pure title-embedding similarity misses —
//! and the reverse (near-identical titles, e.g. two same-named-but-different-artist "Vol. 1"s) is
//! exactly what tag overlap alone would over-merge.
//!
//! Not every tag namespace is Jaccard-eligible — `tag_set`'s own `NOISE_NAMESPACES` drops
//! `date_added`/`timestamp`/`source`/`rating`/`uploader` unconditionally (essentially-unique
//! per-archive values that only dilute the real signal; `timestamp` carries real value elsewhere,
//! chronological chapter ordering, just not for this same-series judgment). `artist:`/`circle:`
//! stay Jaccard-eligible in general (confirmed live: a real anthology-volume miss traced back to
//! these being *claimed* excluded in an earlier version of this doc but never actually
//! implemented — worth an artist/circle overlap actually counting, most of the time) but are
//! excluded per-PAIR (`jaccard_tag_score`, not baked into `tag_set` itself) whenever either side
//! is `category:anthology` — an anthology's own contributor list is long and volume-specific, so
//! overlap there is coincidental, not a same-series signal the way it is for a single creator's
//! solo work. Every other real namespace (`category:`, `parody:`, `character:`, `other:`, ...)
//! participates in the Jaccard set uniformly, no further per-namespace tuning.
//!
//! `category:` is the one namespace that IS special-cased, but as a hard pre-filter rather than a
//! Jaccard-eligible tag: two archives whose `category:` tags are both present and share no value
//! (`category:manga` vs. `category:cosplay`) can never be suggested as a group no matter how high
//! their blended score would otherwise be — see `category_conflict`'s own docs for why this has to
//! be a gate checked before scoring rather than just another tag folded into the overlap set (a
//! handful of other shared tags would otherwise "out-vote" a genuine content-type mismatch).
//!
//! `cosplayer:` is a hard *inclusion* signal, the mirror image of `category:`'s hard exclusion —
//! two archives sharing at least one `cosplayer:` value are always treated as a similar pair,
//! blended score or no. Started as a soft +0.3 bonus added to the blended score, but that wasn't
//! strong enough on its own: confirmed live, five real same-coser archives (after
//! `artist_backfill.rs`'s LLM backfill gave
//! all five the identical `cosplayer:` tag they were missing) still fragmented into a 2-group and
//! a 3-group under the clique algorithm's own "every pairwise edge must clear the threshold"
//! requirement — the +0.3 bonus wasn't enough to individually push every pair over
//! `SIMILARITY_THRESHOLD` when each shoot's title/tags otherwise varied a lot (different
//! characters/outfits per shoot). "Same coser" is a much stronger same-series signal for cosplay
//! works than a shared `artist:` tag is for manga (the reasoning that got `artist:`/`circle:`
//! rejected below doesn't apply here — a coser's separate shoots of different characters are
//! routinely still meant to be browsed as one ongoing collection, unlike a manga artist's
//! unrelated works), which is what justifies trusting it as strongly as `category_conflict` is
//! trusted in the other direction.
//!
//! Title *tokens* (splitting on whitespace and common punctuation, not the whole-sentence
//! embedding) get a smaller soft bonus too, for the same "confirmed live, missed by the sentence
//! embedding alone" reason: real archives titled `"<coser handle> - <different subject each
//! time>"` share an obvious literal keyword (the handle) a human glances at instantly, but the
//! *sentence*-level embedding (the whole normalized title fed to the model as one string) let the
//! differing remainder dominate the vector enough that the pair never reached
//! `SIMILARITY_THRESHOLD` on `TITLE_WEIGHT`/`TAG_WEIGHT` alone. `TOKEN_OVERLAP_WEIGHT` (0.2,
//! deliberately smaller than the shared-cosplayer signal's own trust level — a shared common word
//! like "第一话" is a much weaker same-series signal than a shared structured tag value, so this scales with Jaccard
//! overlap rather than being a flat bonus for "any overlap at all") multiplies the token-set
//! Jaccard similarity and adds it to the blend — the sentence embedding stays the dominant signal
//! (`TITLE_WEIGHT` 0.5), this only nudges borderline pairs the embedding under-weighted. The
//! delimiter set specifically also splits on `_` — a real, live-confirmed bug: a bracketed handle
//! written `[handle_ ]` (trailing underscore before the closing bracket) tokenized to a *different*
//! string than the same handle written plainly elsewhere (`handle_` vs `handle`), so the very
//! creator this bonus exists for silently never got it until `_` joined the delimiter set.

mod eval;
mod labels;
mod release_markers;
pub mod review;

pub use eval::{evaluate, EvalReview, EvaluationReport};

use std::collections::{HashMap, HashSet};

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::common::error;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/tankoubons/ai-group-suggestions",
            post(ai_group_suggestions),
        )
        .route(
            "/tankoubons/ai-group-suggestions/ignore",
            post(ignore_group_suggestion).delete(unignore_group_suggestion),
        )
        .route(
            "/tankoubons/ai-group-suggestions/ignored",
            axum::routing::get(list_ignored_group_suggestions),
        )
        .route(
            "/tankoubons/ai-group-suggestions/review",
            post(review::review_suggestion),
        )
        .route(
            "/tankoubons/ai-group-suggestions/labels",
            post(labels::record_created)
                .get(labels::label_counts)
                .delete(labels::delete_labels),
        )
}

/// Same threshold `recommend_precompute.rs`'s `precompute_one` uses to decide whether a newly-
/// embedded archive is similar enough to backfill into another archive's cached Top-N list — reused
/// here for consistency (both are "is this pair the same series?" judgments over the same title-
/// embedding cosine-similarity scale), not re-derived independently.
const SIMILARITY_THRESHOLD: f32 = 0.5;
/// Title-embedding-similarity weight in the blended score (tag-overlap gets the rest — see this
/// module's own docs for why neither signal is used alone). Deliberately NOT an even 50/50 split
/// — confirmed live against real cosplay-category archives: `multilingual-e5-small`'s sentence
/// embedding scored EVERY pair in an 11-archive sample 0.84-0.94 cosine similarity, including
/// pairs with completely unrelated characters/creators, while tag Jaccard on the same pairs
/// spread more meaningfully (same-creator pairs 0.15-0.26, unrelated pairs 0.07-0.19) — at 50/50,
/// `TITLE_WEIGHT * ~0.87` alone already sat within ~0.07 of `SIMILARITY_THRESHOLD`, so the
/// high-but-undiscriminating title score was doing most of the work of crossing the threshold for
/// EVERY pair, genuinely-related and unrelated alike, with tags/bonuses only nudging an
/// already-close baseline rather than being the actual deciding signal.
const TITLE_WEIGHT: f32 = 0.3;
const TAG_WEIGHT: f32 = 0.7;
/// Multiplies the title-token-set Jaccard similarity before adding it to the blended score — see
/// this module's own top-level docs for why this scales with overlap degree rather than being a
/// flat per-pair bonus.
const TOKEN_OVERLAP_WEIGHT: f32 = 0.2;
/// A suggested group of exactly one archive isn't a suggestion (nothing to group it *with*) —
/// matches `duplicates.rs`'s own `group.len() >= 2` filter on its connected components.
const MIN_GROUP_SIZE: usize = 2;

#[derive(Serialize)]
struct GroupSuggestion {
    /// New members to add — for an `existing_tankoubon_id` suggestion, these are the *additional*
    /// archives only, never repeating the Tankoubon's own existing members.
    archive_ids: Vec<String>,
    /// `Some(tankid)` when this suggestion is "add these archives to an existing Tankoubon" rather
    /// than "group these loose archives into a new one" — see this module's own top-level docs for
    /// how an existing Tankoubon participates in the same clique algorithm as a synthetic node. A
    /// frontend that doesn't know this field yet just never sees it (`skip_serializing_if`), which
    /// degrades to the old "always a new group" behavior rather than sending a malformed shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    existing_tankoubon_id: Option<String>,
    /// The suggestion's identity for review, records and labels.
    suggestion_id: lanrurugi_core::ids::GroupSuggestionId,
    /// Every member pair's evidence (FR-012).
    evidence: Vec<PairEvidence>,
    /// Present when the group is settled (shares a creator or cosplayer); otherwise the card asks
    /// the review endpoint, which answers from the review cache when it can.
    #[serde(skip_serializing_if = "Option::is_none")]
    review: Option<review::Review>,
    /// Stored with the record for the reviewer; not sent to the client.
    #[serde(skip)]
    hints: review::ReviewHints,
}

#[cfg(test)]
impl GroupSuggestion {
    fn for_test(archive_ids: Vec<String>, existing_tankoubon_id: Option<String>) -> Self {
        let suggestion_id = lanrurugi_storage::ignored_group_suggestions::suggestion_id(
            &archive_ids,
            existing_tankoubon_id.as_deref(),
        );
        Self {
            archive_ids,
            existing_tankoubon_id,
            suggestion_id,
            evidence: Vec::new(),
            review: None,
            hints: review::ReviewHints::default(),
        }
    }
}

/// Tag namespaces excluded from the Jaccard similarity set entirely, regardless of value — real,
/// live-confirmed noise rather than same-series signal:
/// - `date_added`/`timestamp`: per-archive ingest/original-post time, essentially unique to each
///   archive even within a genuine same-series set (an anthology's own volumes are posted weeks
///   apart) — pure Jaccard-denominator noise that dilutes every other real signal. (`timestamp`
///   does carry real value elsewhere — chronological chapter ordering — just not for *this*
///   "are these the same series" judgment.)
/// - `source`: a unique per-archive URL, never shared by any two different archives at all.
/// - `rating`/`uploader`: a personal rating or who happened to upload it, neither of which bears
///   on whether two archives are creatively the same series.
///
/// Found via a real, live-confirmed miss: four volumes of the same real anthology (identical
/// `artist:` overlap, `category:manga` on all four) scored every pairwise combination above
/// `SIMILARITY_THRESHOLD` in isolation, yet one volume was silently excluded from the real
/// `ai_group_suggestions` grouping output — traced to exactly this noise diluting the Jaccard
/// score inside the wider real candidate pool, where the earlier module-doc claim that
/// `artist:`/`circle:` were "considered and rejected... not blended in" was aspirational, not
/// actually implemented — `tag_set` had never filtered anything at all.
const NOISE_NAMESPACES: [&str; 5] = ["date_added", "timestamp", "source", "rating", "uploader"];

/// Parses `namespace:value` tags into a plain set of the whole `"namespace:value"` strings (not
/// split further) — Jaccard similarity over this set treats `rating:5` and `rating:4` as
/// non-overlapping, which is correct (a group-suggestion signal shouldn't treat "differently
/// rated" as "same tag"), and naturally gives more overlap weight to archives sharing several tags
/// than ones sharing only one, without needing per-namespace weighting. `NOISE_NAMESPACES` are
/// dropped unconditionally; `artist:`/`circle:` stay in this set (unlike this module's own
/// earlier, never-actually-implemented claim) and are instead excluded per-pair only when either
/// side is an anthology — see `jaccard_tag_score`'s own docs for why that has to be a pairwise
/// decision rather than a per-archive one.
fn tag_set(tags: &str) -> HashSet<&str> {
    tags.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .filter(|t| {
            let ns = t.split(':').next().unwrap_or(t);
            !NOISE_NAMESPACES.contains(&ns)
        })
        .collect()
}

fn jaccard(a: &HashSet<&str>, b: &HashSet<&str>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let intersection = a.intersection(b).count();
    let union = a.union(b).count();
    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

/// The Jaccard tag-overlap score actually used in the blended similarity — wraps `jaccard` with
/// one pairwise-only adjustment: when EITHER side is `category:anthology`, `artist:`/`circle:`
/// values are excluded from the comparison entirely (checked here, not baked into `tag_set`
/// itself, since whether they're excluded depends on the OTHER archive in the pair, not on either
/// archive alone). An anthology's own `artist:` list is every contributor to that one volume —
/// long, and different from one volume to the next — so two anthology volumes sharing a few
/// contributors is coincidental overlap, not a same-series signal the way it is for a single
/// creator's own solo doujinshi/manga; comparing an anthology against a non-anthology work on
/// `artist:` is equally meaningless (a contributor to an anthology and the solo creator of some
/// unrelated work sharing a name proves nothing). Only `artist:`/`circle:` are stripped for this
/// pair — every other real namespace (`category:`, `parody:`, `character:`, `other:`, ...) still
/// participates normally.
#[cfg(test)]
fn jaccard_tag_score<'a>(a: &HashSet<&'a str>, b: &HashSet<&'a str>) -> f32 {
    let anthology = |tags: &HashSet<&str>| tags.contains("category:anthology");
    if anthology(a) || anthology(b) {
        let strip = |tags: &HashSet<&'a str>| -> HashSet<&'a str> {
            tags.iter()
                .copied()
                .filter(|t| creator_value(t).is_none())
                .collect()
        };
        jaccard(&strip(a), &strip(b))
    } else {
        jaccard(a, b)
    }
}

/// Every `category:`-namespaced value on an archive (legacy allows more than one — e.g. an
/// archive tagged both `category:manga` and `category:doujinshi` — so this is a set, not a single
/// value; see `category_conflict`'s own docs for how a multi-value set is compared).
fn category_values<'a>(tags: &HashSet<&'a str>) -> HashSet<&'a str> {
    tags.iter()
        .filter_map(|t| t.strip_prefix("category:"))
        .collect()
}

/// Hard exclusion: two archives that both carry `category:` tags but share none of them (e.g.
/// `category:manga` vs. `category:cosplay`) can never be suggested as the same group, regardless
/// of how similar their titles/other tags are — this is checked BEFORE the blended similarity
/// score, not folded into the Jaccard set as just one more tag to weigh, because burying a
/// content-type mismatch inside a large tag-overlap score lets enough *other* shared tags out-vote
/// it (an archive pair with 5 shared tags and 1 conflicting category would still score high on
/// Jaccard alone). An archive with NO category tag imposes no constraint either way (`is_empty()`
/// short-circuits to "no conflict") — this is a mismatch check, not a same-category requirement,
/// so untagged archives can still be grouped by title/other-tag similarity alone.
fn category_conflict(a: &HashSet<&str>, b: &HashSet<&str>) -> bool {
    let cats_a = category_values(a);
    let cats_b = category_values(b);
    if cats_a.is_empty() || cats_b.is_empty() {
        return false;
    }
    cats_a.is_disjoint(&cats_b)
}

/// Every `cosplayer:`-namespaced value on an archive — a set for the same reason
/// `category_values` is (an archive could in principle carry more than one).
fn cosplayer_values<'a>(tags: &HashSet<&'a str>) -> HashSet<&'a str> {
    tags.iter()
        .filter_map(|t| t.strip_prefix("cosplayer:"))
        .collect()
}

/// Whether two archives share at least one `cosplayer:` value — see this module's own top-level
/// docs for why this is trusted as a hard *inclusion* signal in `is_similar_pair` (the mirror of
/// `category_conflict`'s hard exclusion), not folded uniformly into the Jaccard tag set.
fn shares_cosplayer(a: &HashSet<&str>, b: &HashSet<&str>) -> bool {
    !cosplayer_values(a).is_disjoint(&cosplayer_values(b))
}

/// Splits a title into a lowercased token set on whitespace and the common delimiters real
/// titles in this library actually use to separate a series/creator prefix from the rest (ASCII
/// and full-width hyphen/colon/middle-dot, underscore, plus bracket pairs) — deliberately not the
/// same regex set `normalize_title` uses (that strips *source-format* noise like
/// `[Patreon]`/`【260529】` wholesale; this instead wants to keep every substantive word as its own
/// token, brackets and all, since a shared creator handle appearing in multiple titles is exactly
/// the signal this is for). Empty tokens (from consecutive delimiters) and pure-punctuation
/// leftovers are dropped.
fn title_tokens(title: &str) -> HashSet<&str> {
    title
        .split([
            ' ', '\t', '_', '-', '－', '–', '—', ':', '：', '·', '•', '[', ']', '【', '】', '(',
            ')', '（', '）',
        ])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect()
}

/// Whether `id` is a synthetic node standing in for an existing Tankoubon (see this module's own
/// top-level docs on how one participates in the clique algorithm), not a real archive — reuses
/// the same `TANK_` id-prefix convention `ai_group_suggestions` already relies on elsewhere to
/// tell a Tankoubon's own aggregate search-index entry apart from a real (40-lowercase-hex) archive
/// id, rather than introducing a second way to mark the same distinction.
fn is_existing_tankoubon_node(id: &str) -> bool {
    id.starts_with("TANK_")
}

/// Two existing-Tankoubon synthetic nodes are never a similar pair, regardless of what their own
/// (member-averaged) vectors/tags would otherwise score — a suggestion to merge two *already-real*
/// Tankoubons into each other isn't this endpoint's job (that's a manual "move archives between
/// Tankoubons" operation, not an AI grouping suggestion), and without this guard the clique
/// algorithm has no other reason to keep two such nodes apart. A real archive can still be pulled
/// into at most one existing Tankoubon's clique, since a clique can only contain nodes pairwise
/// similar to each other and this guard makes any two Tankoubon nodes automatically non-similar —
/// so two different existing Tankoubons' cliques never overlap on any given real archive candidate
/// either (each candidate would have to be similar to both anchor nodes AND the two anchor nodes
/// similar to each other, which this guard forbids).
fn tankoubon_pair_gate(a_id: &str, b_id: &str) -> bool {
    !(is_existing_tankoubon_node(a_id) && is_existing_tankoubon_node(b_id))
}

/// Every tunable of the grouping pipeline (`specs/016-tankoubon-grouping-harness` data-model §1).
/// `Default` is the shipped configuration; the evaluation loads candidates from JSON, any field
/// omitted keeping its default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupingSettings {
    pub title_weight: f32,
    pub tag_weight: f32,
    pub token_weight: f32,
    pub threshold: f32,
    /// FR-001: known, disjoint creators are not one series.
    pub creator_gate: bool,
    /// FR-002: release markers never count as shared title words.
    pub strip_release_markers: bool,
    /// FR-003: tag overlap weighted by rarity in the candidate pool.
    pub idf_tags: bool,
    pub review_max_members: usize,
    pub review_max_tags: usize,
    pub review_title_chars: usize,
}

impl Default for GroupingSettings {
    fn default() -> Self {
        Self {
            title_weight: TITLE_WEIGHT,
            tag_weight: TAG_WEIGHT,
            token_weight: TOKEN_OVERLAP_WEIGHT,
            threshold: SIMILARITY_THRESHOLD,
            creator_gate: true,
            strip_release_markers: true,
            idf_tags: true,
            review_max_members: 12,
            review_max_tags: 6,
            review_title_chars: 80,
        }
    }
}

/// Title tokens as the settings say to measure them: release markers removed when enabled.
pub(crate) fn title_tokens_for<'a>(
    title: &'a str,
    settings: &GroupingSettings,
) -> HashSet<&'a str> {
    let tokens = title_tokens(title);
    if settings.strip_release_markers {
        release_markers::strip(tokens)
    } else {
        tokens
    }
}

/// Inverse document frequency of each tag over one candidate pool (research.md §3): a tag most
/// candidates carry weighs little, a rare shared tag much more. Smoothed so nothing reaches zero.
pub(crate) struct TagRarity<'a> {
    idf: HashMap<&'a str, f32>,
    unseen: f32,
}

impl<'a> TagRarity<'a> {
    pub(crate) fn build<'m>(tags: impl IntoIterator<Item = &'m HashSet<&'a str>>) -> Self
    where
        'a: 'm,
    {
        let mut df: HashMap<&'a str, usize> = HashMap::new();
        let mut n = 0usize;
        for set in tags {
            n += 1;
            for &t in set {
                *df.entry(t).or_default() += 1;
            }
        }
        let idf_of = |d: usize| ((n as f32 + 1.0) / (d as f32 + 1.0)).ln() + 1.0;
        Self {
            idf: df.into_iter().map(|(t, d)| (t, idf_of(d))).collect(),
            unseen: idf_of(0),
        }
    }

    pub(crate) fn weight(&self, tag: &str) -> f32 {
        self.idf.get(tag).copied().unwrap_or(self.unseen)
    }
}

/// Namespaces naming who made a work. `group:` is e-hentai's name for a circle; this project's own
/// backfill writes `circle:`. All three are one creator set.
pub(crate) const CREATOR_NAMESPACES: [&str; 3] = ["artist", "circle", "group"];

/// The creator value of one tag, if it is a creator tag.
pub(crate) fn creator_value(tag: &str) -> Option<&str> {
    let (ns, value) = tag.split_once(':')?;
    CREATOR_NAMESPACES.contains(&ns).then_some(value)
}

/// Every creator value on an archive.
fn creator_values<'a>(tags: &HashSet<&'a str>) -> HashSet<&'a str> {
    tags.iter().filter_map(|t| creator_value(t)).collect()
}

fn is_anthology(tags: &HashSet<&str>) -> bool {
    tags.contains("category:anthology")
}

/// FR-001: both creator sets known and disjoint, neither side an anthology. Unknown is never
/// evidence of a mismatch. A shared cosplayer is checked before this and wins.
fn creator_conflict(a: &HashSet<&str>, b: &HashSet<&str>) -> bool {
    if is_anthology(a) || is_anthology(b) {
        return false;
    }
    let (ca, cb) = (creator_values(a), creator_values(b));
    !ca.is_empty() && !cb.is_empty() && ca.is_disjoint(&cb)
}

/// How two members' creators relate, for the evidence panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CreatorMatch {
    Same,
    Different,
    Unknown,
}

fn creator_match(a: &HashSet<&str>, b: &HashSet<&str>) -> CreatorMatch {
    let (ca, cb) = (creator_values(a), creator_values(b));
    if ca.is_empty() || cb.is_empty() {
        CreatorMatch::Unknown
    } else if ca.is_disjoint(&cb) {
        CreatorMatch::Different
    } else {
        CreatorMatch::Same
    }
}

/// The gate that ruled a pair out before scoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BlockedBy {
    Category,
    Creator,
}

/// One pair's evidence (data-model.md §2) — the single source of truth for both the grouping
/// decision and what the user and the evaluation see.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct PairEvidence {
    pub a: String,
    pub b: String,
    pub title: f32,
    pub tag: f32,
    pub tokens: f32,
    pub score: f32,
    pub creator_match: CreatorMatch,
    pub shared_cosplayer: bool,
    pub shared_top_tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<BlockedBy>,
    pub similar: bool,
}

/// Everything a pair judgment needs, built once per request.
pub(crate) struct Scorer<'a> {
    pub vectors: &'a HashMap<&'a str, &'a [f32]>,
    pub tags: &'a HashMap<&'a str, HashSet<&'a str>>,
    pub tokens: &'a HashMap<&'a str, HashSet<&'a str>>,
    pub settings: &'a GroupingSettings,
    pub rarity: TagRarity<'a>,
    empty: HashSet<&'a str>,
}

impl<'a> Scorer<'a> {
    pub(crate) fn new(
        vectors: &'a HashMap<&'a str, &'a [f32]>,
        tags: &'a HashMap<&'a str, HashSet<&'a str>>,
        tokens: &'a HashMap<&'a str, HashSet<&'a str>>,
        settings: &'a GroupingSettings,
    ) -> Self {
        Self {
            vectors,
            tags,
            tokens,
            settings,
            rarity: TagRarity::build(tags.values()),
            empty: HashSet::new(),
        }
    }

    fn tags_of(&self, id: &str) -> &HashSet<&'a str> {
        self.tags.get(id).unwrap_or(&self.empty)
    }

    /// Tag overlap: rarity-weighted Jaccard when enabled, plain otherwise. When either side is an
    /// anthology, creator tags are left out — an anthology's contributor list is long and
    /// volume-specific, so overlap there is coincidence, not a same-series signal.
    fn tag_overlap(&self, a: &HashSet<&'a str>, b: &HashSet<&'a str>) -> f32 {
        let keep = |t: &&str| !((is_anthology(a) || is_anthology(b)) && creator_value(t).is_some());
        let a: HashSet<&str> = a.iter().copied().filter(keep).collect();
        let b: HashSet<&str> = b.iter().copied().filter(keep).collect();
        if !self.settings.idf_tags {
            return jaccard(&a, &b);
        }
        let union: f32 = a.union(&b).map(|t| self.rarity.weight(t)).sum();
        if union == 0.0 {
            return 0.0;
        }
        a.intersection(&b)
            .map(|t| self.rarity.weight(t))
            .sum::<f32>()
            / union
    }

    /// The pair's evidence, or `None` when either side has no title vector.
    pub(crate) fn evidence(&self, a: &str, b: &str) -> Option<PairEvidence> {
        let (&a_vec, &b_vec) = (self.vectors.get(a)?, self.vectors.get(b)?);
        let (a_tags, b_tags) = (self.tags_of(a), self.tags_of(b));
        let empty = &self.empty;
        let (a_tokens, b_tokens) = (
            self.tokens.get(a).unwrap_or(empty),
            self.tokens.get(b).unwrap_or(empty),
        );
        let title = lanrurugi_recommend::embedding::cosine_similarity(a_vec, b_vec);
        let tag = self.tag_overlap(a_tags, b_tags);
        let tokens = jaccard(a_tokens, b_tokens);
        let s = self.settings;
        let score = s.title_weight * title + s.tag_weight * tag + s.token_weight * tokens;
        let shared_cosplayer = shares_cosplayer(a_tags, b_tags);
        // A shared cosplayer short-circuits to similar (a coser's sets vary per shoot but belong
        // together); a category or creator conflict short-circuits to not similar, so shared
        // boilerplate can never out-vote a real mismatch.
        let blocked_by = if !tankoubon_pair_gate(a, b) || category_conflict(a_tags, b_tags) {
            Some(BlockedBy::Category)
        } else if !shared_cosplayer && s.creator_gate && creator_conflict(a_tags, b_tags) {
            Some(BlockedBy::Creator)
        } else {
            None
        };
        let similar = blocked_by.is_none() && (shared_cosplayer || score >= s.threshold);
        let mut shared: Vec<&str> = a_tags.intersection(b_tags).copied().collect();
        shared.sort_by(|x, y| {
            self.rarity
                .weight(y)
                .total_cmp(&self.rarity.weight(x))
                .then(x.cmp(y))
        });
        Some(PairEvidence {
            a: a.to_string(),
            b: b.to_string(),
            title,
            tag,
            tokens,
            score,
            creator_match: creator_match(a_tags, b_tags),
            shared_cosplayer,
            shared_top_tags: shared.into_iter().take(5).map(String::from).collect(),
            blocked_by,
            similar,
        })
    }

    pub(crate) fn similar(&self, a: &str, b: &str) -> bool {
        self.evidence(a, b).is_some_and(|e| e.similar)
    }

    /// Ranking-only score for clique extension: a shared cosplayer outranks any blend.
    fn sort_score(&self, a: &str, b: &str) -> f32 {
        match self.evidence(a, b) {
            Some(e) if e.shared_cosplayer => 2.0,
            Some(e) => e.score,
            None => 0.0,
        }
    }
}

/// Plain connected-components DFS over the similarity relation — only to shrink the search space
/// the clique extraction has to consider, NOT the final grouping: a component alone over-merges (a
/// real case chained three different cosplayers' shoots into one 9-archive "suggestion").
fn connected_components<'a>(ids: &[&'a str], scorer: &Scorer) -> Vec<Vec<&'a str>> {
    let mut visited: HashSet<&str> = HashSet::new();
    let mut components = Vec::new();

    for &start_id in ids {
        if visited.contains(start_id) || !scorer.vectors.contains_key(start_id) {
            continue;
        }
        let mut stack = vec![start_id];
        let mut component = Vec::new();
        while let Some(node) = stack.pop() {
            if !visited.insert(node) {
                continue;
            }
            component.push(node);
            for &other_id in ids {
                if !visited.contains(other_id) && scorer.similar(node, other_id) {
                    stack.push(other_id);
                }
            }
        }
        components.push(component);
    }
    components
}

/// Greedy maximal-clique extension: repeatedly add whichever remaining candidate is similar to
/// EVERY current member, preferring the highest average fit. No backtracking — components are
/// small, and a greedy near-maximal clique is the right cost/quality trade-off.
fn largest_clique_containing<'a>(
    start_id: &'a str,
    candidates: &[&'a str],
    scorer: &Scorer,
) -> Vec<&'a str> {
    let mut clique = vec![start_id];
    let mut remaining: Vec<&str> = candidates
        .iter()
        .copied()
        .filter(|&id| id != start_id)
        .collect();
    loop {
        let mut best: Option<(&str, f32)> = None;
        for &candidate in &remaining {
            if !clique.iter().all(|&m| scorer.similar(m, candidate)) {
                continue;
            }
            let avg = clique
                .iter()
                .map(|&m| scorer.sort_score(m, candidate))
                .sum::<f32>()
                / clique.len() as f32;
            if best.is_none_or(|(_, best_avg)| avg > best_avg) {
                best = Some((candidate, avg));
            }
        }
        let Some((chosen, _)) = best else { break };
        clique.push(chosen);
        remaining.retain(|&id| id != chosen);
    }
    clique
}

/// Groups candidates into suggested Tankoubons — connected components first (cheap pruning), then
/// repeatedly the largest clique (every member pairwise similar, not just chain-connected). An
/// existing-Tankoubon node starts first, so an archive that fits it is claimed by it.
pub(crate) fn group_with(ids: &[String], scorer: &Scorer) -> Vec<Vec<String>> {
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut groups = Vec::new();
    for component in connected_components(&id_refs, scorer) {
        let mut remaining = component;
        while remaining.len() >= MIN_GROUP_SIZE {
            let start = remaining
                .iter()
                .find(|&&id| is_existing_tankoubon_node(id))
                .copied()
                .unwrap_or(remaining[0]);
            let mut clique = largest_clique_containing(start, &remaining, scorer);
            if clique.len() < MIN_GROUP_SIZE {
                // Drop the unclusterable start so it is not re-selected forever.
                remaining.retain(|&id| id != start);
                continue;
            }
            clique.sort();
            let clique_set: HashSet<&str> = clique.iter().copied().collect();
            remaining.retain(|id| !clique_set.contains(id));
            groups.push(clique.into_iter().map(String::from).collect());
        }
    }
    groups
}

/// [`group_with`] under the shipped settings — the shape the existing tests use.
#[cfg(test)]
fn group_by_similarity(
    ids: &[String],
    vectors: &HashMap<&str, &[f32]>,
    tags: &HashMap<&str, HashSet<&str>>,
    title_token_sets: &HashMap<&str, HashSet<&str>>,
) -> Vec<Vec<String>> {
    let settings = GroupingSettings::default();
    let scorer = Scorer::new(vectors, tags, title_token_sets, &settings);
    group_with(ids, &scorer)
}

/// Every member pair's evidence for one group.
pub(crate) fn group_evidence(group: &[String], scorer: &Scorer) -> Vec<PairEvidence> {
    let mut out = Vec::new();
    for (i, a) in group.iter().enumerate() {
        for b in &group[i + 1..] {
            if let Some(e) = scorer.evidence(a, b) {
                out.push(e);
            }
        }
    }
    out
}

/// Drops any suggestion whose exact fingerprint (see `lanrurugi_storage::
/// ignored_group_suggestions::fingerprint`) is in `ignored_fingerprints` — pure filtering logic,
/// pulled out of `ai_group_suggestions` itself so it's testable without a real Redis connection.
fn filter_ignored_suggestions(
    suggestions: Vec<GroupSuggestion>,
    ignored_fingerprints: &HashSet<String>,
) -> Vec<GroupSuggestion> {
    suggestions
        .into_iter()
        .filter(|s| {
            let fp = lanrurugi_storage::ignored_group_suggestions::fingerprint(
                &s.archive_ids,
                s.existing_tankoubon_id.as_deref(),
            );
            !ignored_fingerprints.contains(&fp)
        })
        .collect()
}

#[derive(Deserialize)]
struct AiGroupSuggestionsQuery {
    /// Default `false` — a previously-ignored exact combination ("Don't suggest this again",
    /// matched via `lanrurugi_storage::ignored_group_suggestions::fingerprint` against this
    /// suggestion's own archive-id set + `existing_tankoubon_id`) is filtered out unless the
    /// frontend's own "Show ignored combinations" checkbox is on, which re-requests with this set
    /// to `true`.
    #[serde(default)]
    include_ignored: bool,
}

async fn ai_group_suggestions(
    State(state): State<AppState>,
    Query(query): Query<AiGroupSuggestionsQuery>,
) -> Response {
    // No upfront "is the model ready" gate here anymore — `lanrurugi-embed-worker` (see
    // `crate::embed_worker_client`'s own doc comment) is spawned on demand by the very first
    // `embed()` call below, same as every other on-demand subprocess in this codebase
    // (`gpu_worker_client`'s own workers); there is no longer a background "still downloading the
    // model" state a request could be rejected for arriving during. A genuinely failed embed
    // still degrades this endpoint's own results per-candidate (see the `embed()` call sites
    // below), it just never blocks the whole request up front on a state that no longer exists.

    // "Not currently a member of any Tankoubon" is exactly `LRR_TANKGROUPED` membership (despite
    // the legacy-inherited name — see that key's own doc comment in lanrurugi-search::keys) minus
    // the Tankoubons' own aggregate entries (also members of this set, identifiable by their
    // `TANK_` id prefix — real archive ids are 40 lowercase-hex chars and never start with that).
    let mut search_conn = match state.redis.search.get().await {
        Ok(c) => c,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "ai_group_suggestions",
                e.to_string(),
            )
        }
    };
    let ungrouped_ids: Vec<String> = match search_conn
        .smembers::<_, Vec<String>>(lanrurugi_search::keys::TANKGROUPED_KEY)
        .await
    {
        Ok(ids) => ids
            .into_iter()
            .filter(|id| !id.starts_with("TANK_"))
            .collect(),
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "ai_group_suggestions",
                e.to_string(),
            )
        }
    };

    // Every existing Tankoubon also participates as a synthetic node (see this module's own
    // top-level docs) — fetched even when `ungrouped_ids` alone is below `MIN_GROUP_SIZE`, since a
    // single loose archive plus one existing Tankoubon is still a valid "add to existing" scenario
    // (a real archive count of exactly 1 candidate isn't "nothing to suggest" the way it would be
    // for two loose archives with nothing else around them).
    let existing_tankoubons = match state.repos.groupings.list_all().await {
        Ok(g) => g,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "ai_group_suggestions",
                e.to_string(),
            )
        }
    };

    if ungrouped_ids.is_empty()
        || (ungrouped_ids.len() < MIN_GROUP_SIZE && existing_tankoubons.is_empty())
    {
        return axum::Json(json!({ "suggestions": Vec::<GroupSuggestion>::new() })).into_response();
    }

    // Title + tags for every ungrouped archive — tags come straight from the archive record
    // (cheap, no separate cache); title *vectors* prefer the persisted recommend-cache (populated
    // by `recommend_precompute.rs` at catalogue/title-change time) and fall back to embedding
    // on-the-spot for any archive that cache doesn't have yet (a fresh install before the initial
    // backfill finishes, or an archive whose title changed since — same graceful-degradation shape
    // `recommend.rs`'s own request path uses).
    let mut tags_by_id: HashMap<String, String> = HashMap::new();
    let mut titles_by_id: HashMap<String, String> = HashMap::new();
    let mut vectors_by_id: HashMap<String, Vec<f32>> = HashMap::new();
    let mut missing_titles: Vec<(String, String)> = Vec::new();

    for id in &ungrouped_ids {
        let Ok(Some(archive)) = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
            .await
        else {
            continue;
        };
        tags_by_id.insert(id.clone(), archive.tags.clone());
        titles_by_id.insert(id.clone(), archive.title.clone());
        match state.recommend_cache.get_vector(id).await {
            Ok(Some((cached_title, vector))) if cached_title == archive.title => {
                vectors_by_id.insert(id.clone(), vector);
            }
            _ => missing_titles.push((id.clone(), archive.title)),
        }
    }

    // Each existing Tankoubon's synthetic node needs the same three inputs a real archive has
    // (vector, tags, title-token source) — built from its own members, not stored directly on
    // `Grouping` itself, so this stays in sync automatically as members/tags change over time
    // rather than needing its own separate cache invalidation story.
    let mut tankoubon_ids: Vec<String> = Vec::new();
    for tankoubon in &existing_tankoubons {
        if tankoubon.archives.is_empty() {
            // An empty Tankoubon has no member titles/tags to derive a synthetic node from —
            // nothing meaningful to suggest adding to, so it just doesn't participate at all
            // (distinct from a Tankoubon whose members simply don't have their own tags — that
            // still has a valid, if all-empty, tag set and a real average vector).
            continue;
        }
        let node_id = tankoubon.tankid.as_str().to_string();
        let mut member_tags: Vec<String> = Vec::new();
        let mut member_vectors: Vec<Vec<f32>> = Vec::new();
        for member_id in &tankoubon.archives {
            let Ok(Some(archive)) = state.repos.archives.get(member_id).await else {
                continue;
            };
            member_tags.push(archive.tags.clone());
            match state.recommend_cache.get_vector(member_id.as_str()).await {
                Ok(Some((cached_title, vector))) if cached_title == archive.title => {
                    member_vectors.push(vector);
                }
                _ => {
                    let normalized =
                        lanrurugi_recommend::recommend::normalize_title(&archive.title);
                    if let Ok(vector) = state
                        .recommender
                        .embed_worker_client()
                        .embed(&normalized)
                        .await
                    {
                        let _ = state
                            .recommend_cache
                            .put_vector(member_id.as_str(), &archive.title, &vector)
                            .await;
                        member_vectors.push(vector);
                    }
                }
            }
        }
        if member_vectors.is_empty() {
            continue;
        }
        let dims = member_vectors[0].len();
        let mut averaged = vec![0.0f32; dims];
        for v in &member_vectors {
            for (i, x) in v.iter().enumerate() {
                averaged[i] += x;
            }
        }
        let count = member_vectors.len() as f32;
        for x in &mut averaged {
            *x /= count;
        }
        vectors_by_id.insert(node_id.clone(), averaged);
        tags_by_id.insert(node_id.clone(), member_tags.join(","));
        // The Tankoubon's own name stands in for a "title" — used only to derive its synthetic
        // node's title-token set (see `title_tokens`'s own docs on what that bonus is for); a
        // freshly-created Tankoubon's name is initialized from its first member's own title (see
        // `AiSmartTankoubonModal.tsx`'s create flow), so this carries the same creator-handle/
        // series-keyword signal a real archive's own title would.
        titles_by_id.insert(node_id.clone(), tankoubon.name.clone());
        tankoubon_ids.push(node_id);
    }

    if !missing_titles.is_empty() {
        let embedder = state.recommender.embed_worker_client();
        // Sequential, not `spawn_blocking`+parallel — same reasoning as
        // `recommend_precompute.rs`'s own batch-embed loop: `embed()` is now an IPC round trip to
        // `lanrurugi-embed-worker`'s single serialized session, not in-process ONNX inference, so
        // there's no local blocking work to bounce off this handler's thread and no real
        // parallelism to gain from fanning these out.
        let mut computed: Vec<(String, String, Vec<f32>)> =
            Vec::with_capacity(missing_titles.len());
        for (id, title) in missing_titles {
            let normalized = lanrurugi_recommend::recommend::normalize_title(&title);
            if let Ok(v) = embedder.embed(&normalized).await {
                computed.push((id, title, v));
            }
        }
        for (id, title, vector) in computed {
            // Best-effort cache the freshly-computed vector for next time — same reasoning as
            // `recommend.rs`'s own miss-path fire-and-forget `precompute_one`, but done inline
            // here (already off the async reactor via spawn_blocking above, and this handler
            // doesn't have a hot per-request latency budget to protect the way the live
            // recommendations endpoint does).
            let _ = state.recommend_cache.put_vector(&id, &title, &vector).await;
            vectors_by_id.insert(id, vector);
        }
    }

    // The borrowed `&str`/`&[f32]` views `group_by_similarity` wants are built INSIDE the blocking
    // closure (not passed in from out here) — `spawn_blocking` requires a `'static` closure, and a
    // `HashMap<&str, _>` borrowing from `vectors_by_id`/`tags_by_id` can't satisfy that from the
    // outside; moving the owned maps in and re-deriving the borrowed views locally sidesteps that
    // instead of fighting the borrow checker with unnecessary cloning.
    let settings = GroupingSettings::default();
    let settings_for_task = settings.clone();
    // Kept for the inline review lookup after grouping (the originals move into the task).
    let titles_for_review = titles_by_id.clone();
    let tags_for_review = tags_by_id.clone();
    let groups = tokio::task::spawn_blocking(move || {
        let settings = settings_for_task;
        let vector_refs: HashMap<&str, &[f32]> = vectors_by_id
            .iter()
            .map(|(id, v)| (id.as_str(), v.as_slice()))
            .collect();
        let tag_refs: HashMap<&str, HashSet<&str>> = tags_by_id
            .iter()
            .map(|(id, t)| (id.as_str(), tag_set(t)))
            .collect();
        let title_token_refs: HashMap<&str, HashSet<&str>> = titles_by_id
            .iter()
            .map(|(id, t)| (id.as_str(), title_tokens_for(t, &settings)))
            .collect();
        // Every existing-Tankoubon synthetic node id is appended after the real ungrouped archive
        // ids — order doesn't matter to `group_by_similarity` itself (it only reads this as an id
        // set to iterate/filter, not as a priority ordering; the actual "prefer a Tankoubon node as
        // a clique's starting point" preference lives inside that function, driven by
        // `is_existing_tankoubon_node`, not by this list's order).
        let all_ids: Vec<String> = ungrouped_ids.into_iter().chain(tankoubon_ids).collect();
        let scorer = Scorer::new(&vector_refs, &tag_refs, &title_token_refs, &settings);
        group_with(&all_ids, &scorer)
            .into_iter()
            .map(|group| {
                let evidence = group_evidence(&group, &scorer);
                let hints = review_hints(&group, &evidence, &scorer);
                (group, evidence, hints)
            })
            .collect::<Vec<_>>()
    })
    .await;
    let groups = match groups {
        Ok(g) => g,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "ai_group_suggestions",
                e.to_string(),
            )
        }
    };

    // Split each group into its existing-Tankoubon anchor (if any — at most one, per
    // `tankoubon_pair_gate`'s own guarantee) plus the real archive ids, which become the
    // suggestion's own `existing_tankoubon_id`/`archive_ids` pair. A group with no Tankoubon node
    // at all is a plain "suggest a new Tankoubon" entry, same as before this feature existed.
    let suggestions: Vec<GroupSuggestion> = groups
        .into_iter()
        .map(|(group, evidence, hints)| {
            let mut existing_tankoubon_id = None;
            let mut archive_ids = Vec::with_capacity(group.len());
            for id in group {
                if is_existing_tankoubon_node(&id) {
                    existing_tankoubon_id = Some(id);
                } else {
                    archive_ids.push(id);
                }
            }
            let suggestion_id = lanrurugi_storage::ignored_group_suggestions::suggestion_id(
                &archive_ids,
                existing_tankoubon_id.as_deref(),
            );
            GroupSuggestion {
                archive_ids,
                existing_tankoubon_id,
                suggestion_id,
                evidence,
                review: None,
                hints,
            }
        })
        .collect();

    // Drop any suggestion the user already dismissed ("Don't suggest this again"), unless the
    // frontend explicitly asked to see them too (`?include_ignored=true`, the "Show ignored
    // combinations" checkbox). Fetched/filtered only now, after the full (possibly expensive)
    // grouping pass already ran — the ignored set is typically tiny, so gating the grouping work
    // itself behind it wouldn't save anything, and doing it as a plain post-filter here keeps
    // `group_by_similarity` itself free of a concern that has nothing to do with clustering.
    let suggestions = if query.include_ignored {
        suggestions
    } else {
        let ignored_fingerprints =
            match state.ignored_group_suggestions.ignored_fingerprints().await {
                Ok(fps) => fps,
                Err(e) => {
                    return error(
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        "ai_group_suggestions",
                        e.to_string(),
                    )
                }
            };
        filter_ignored_suggestions(suggestions, &ignored_fingerprints)
    };
    let suggestions = enrich_suggestions(
        &state,
        suggestions,
        &titles_for_review,
        &tags_for_review,
        &settings,
    )
    .await;

    axum::Json(json!({ "suggestions": suggestions })).into_response()
}

/// Repository for the harness's labels and records, on the config pool.
pub(crate) fn labels_repo(
    state: &AppState,
) -> lanrurugi_storage::grouping_labels::GroupingLabelsRepository {
    lanrurugi_storage::grouping_labels::GroupingLabelsRepository::new(state.redis.config.clone())
}

/// What is stored for each shown suggestion (data-model.md §5).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SuggestionRecord {
    pub archive_ids: Vec<String>,
    pub existing_tankoubon_id: Option<String>,
    pub evidence: serde_json::Value,
    pub hints: review::ReviewHints,
    pub settings: GroupingSettings,
    pub review: Option<review::Review>,
    pub shown_at: u64,
}

/// Each member's rarest reviewer-eligible tags and its mean tie to the rest of the group.
fn review_hints(
    group: &[String],
    evidence: &[PairEvidence],
    scorer: &Scorer,
) -> review::ReviewHints {
    let mut hints = review::ReviewHints::default();
    for id in group {
        let mut tags: Vec<&str> = scorer
            .tags_of(id)
            .iter()
            .copied()
            .filter(|t| review::eligible_review_tag(t))
            .collect();
        tags.sort_by(|a, b| {
            scorer
                .rarity
                .weight(b)
                .total_cmp(&scorer.rarity.weight(a))
                .then(a.cmp(b))
        });
        hints.top_tags.insert(
            id.clone(),
            tags.into_iter()
                .take(scorer.settings.review_max_tags)
                .map(String::from)
                .collect(),
        );
        let scores: Vec<f32> = evidence
            .iter()
            .filter(|e| &e.a == id || &e.b == id)
            .map(|e| if e.shared_cosplayer { 2.0 } else { e.score })
            .collect();
        let mean = if scores.is_empty() {
            0.0
        } else {
            scores.iter().sum::<f32>() / scores.len() as f32
        };
        hints.strength.insert(id.clone(), mean);
    }
    hints
}

/// Attaches a review where the group is settled and records
/// every returned suggestion. Never calls the provider: the first card must not wait on it.
async fn enrich_suggestions(
    state: &AppState,
    mut suggestions: Vec<GroupSuggestion>,
    titles: &HashMap<String, String>,
    tags: &HashMap<String, String>,
    settings: &GroupingSettings,
) -> Vec<GroupSuggestion> {
    let shown_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut records = Vec::with_capacity(suggestions.len());
    for s in &mut suggestions {
        let facts: Vec<review::MemberFacts> = s
            .archive_ids
            .iter()
            .map(|id| review::MemberFacts {
                id: id.clone(),
                title: titles.get(id).cloned().unwrap_or_default(),
                tags: review::split_tags(tags.get(id).map(String::as_str).unwrap_or("")),
            })
            .collect();
        let existing = s
            .existing_tankoubon_id
            .as_ref()
            .map(|t| review::ExistingTankoubon {
                name: titles.get(t).cloned().unwrap_or_default(),
                member_titles: Vec::new(),
            });
        s.review = match (&existing, review::settled_reason(&facts)) {
            (None, Some(reason)) => Some(review::Review {
                status: review::ReviewStatus::Settled,
                verdicts: Vec::new(),
                reason: Some(reason),
                judged: Vec::new(),
                prompt_version: crate::llm_prompts::GROUPING_REVIEW_PROMPT_VERSION.to_string(),
            }),
            // A cached review is fetched by the card when it comes into view, so the usage ledger
            // counts one cache hit per card actually reviewed, not one per listed suggestion.
            _ => None,
        };
        records.push((
            s.suggestion_id.clone(),
            SuggestionRecord {
                archive_ids: s.archive_ids.clone(),
                existing_tankoubon_id: s.existing_tankoubon_id.clone(),
                evidence: serde_json::to_value(&s.evidence).unwrap_or_default(),
                hints: s.hints.clone(),
                settings: settings.clone(),
                review: s.review.clone(),
                shown_at,
            },
        ));
    }
    if let Err(e) = labels_repo(state).save_records(&records).await {
        tracing::warn!(error = %e, "grouping suggestion records could not be saved");
    }
    suggestions
}

#[derive(Deserialize)]
struct IgnoreSuggestionBody {
    archive_ids: Vec<String>,
    #[serde(default)]
    existing_tankoubon_id: Option<String>,
}

/// `POST /tankoubons/ai-group-suggestions/ignore` — "Don't suggest this again" for one specific
/// suggestion. Idempotent (ignoring an already-ignored combination just overwrites its
/// `ignored_at` timestamp, per `IgnoredGroupSuggestionsRepository::ignore`'s own contract).
async fn ignore_group_suggestion(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<IgnoreSuggestionBody>,
) -> Response {
    if body.archive_ids.is_empty() {
        return error(
            axum::http::StatusCode::BAD_REQUEST,
            "ignore_group_suggestion",
            "archive_ids must not be empty",
        );
    }
    let ignored_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Err(e) = state
        .ignored_group_suggestions
        .ignore(
            &body.archive_ids,
            body.existing_tankoubon_id.as_deref(),
            ignored_at,
        )
        .await
    {
        return error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "ignore_group_suggestion",
            e.to_string(),
        );
    }
    // FR-016: "don't suggest again" is also a negative label for every member pair.
    labels::record_dismissal(
        &state,
        &body.archive_ids,
        body.existing_tankoubon_id.as_deref(),
    )
    .await;
    axum::Json(json!({ "operation": "ignore_group_suggestion", "success": 1 })).into_response()
}

/// `DELETE /tankoubons/ai-group-suggestions/ignore` — re-enables a previously-ignored suggestion
/// (the frontend's "Show ignored combinations" checklist's own "Un-ignore" button). Idempotent —
/// un-ignoring a combination that isn't currently ignored is a no-op, not an error.
async fn unignore_group_suggestion(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<IgnoreSuggestionBody>,
) -> Response {
    if let Err(e) = state
        .ignored_group_suggestions
        .unignore(&body.archive_ids, body.existing_tankoubon_id.as_deref())
        .await
    {
        return error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "unignore_group_suggestion",
            e.to_string(),
        );
    }
    axum::Json(json!({ "operation": "unignore_group_suggestion", "success": 1 })).into_response()
}

/// `GET /tankoubons/ai-group-suggestions/ignored` — backs the "Show ignored combinations"
/// checklist itself (titles are resolved client-side against the already-loaded archive list, the
/// same `titleById` map `AiSmartTankoubonModal.tsx` already builds for the main suggestion cards —
/// this endpoint only needs to return the raw ignored entries, not hydrate them server-side).
async fn list_ignored_group_suggestions(State(state): State<AppState>) -> Response {
    match state.ignored_group_suggestions.list_all().await {
        Ok(entries) => axum::Json(json!({ "ignored": entries })).into_response(),
        Err(e) => error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "list_ignored_group_suggestions",
            e.to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_set_drops_noise_namespaces() {
        let tags = tag_set(
            "date_added:1,timestamp:2,source:x,rating:5,uploader:someone,category:manga,artist:x",
        );
        assert_eq!(
            tags,
            ["category:manga", "artist:x"]
                .into_iter()
                .collect::<HashSet<_>>()
        );
    }

    #[test]
    fn tag_set_keeps_artist_and_circle_by_default() {
        let tags = tag_set("artist:x,circle:y,category:manga");
        assert!(tags.contains("artist:x"));
        assert!(tags.contains("circle:y"));
    }

    #[test]
    fn jaccard_tag_score_counts_artist_overlap_when_neither_side_is_anthology() {
        let a: HashSet<&str> = ["category:manga", "artist:shared"].into_iter().collect();
        let b: HashSet<&str> = ["category:manga", "artist:shared"].into_iter().collect();
        assert_eq!(jaccard_tag_score(&a, &b), 1.0);
    }

    #[test]
    fn jaccard_tag_score_ignores_artist_overlap_when_either_side_is_anthology() {
        let a: HashSet<&str> = ["category:anthology", "artist:shared"]
            .into_iter()
            .collect();
        let b: HashSet<&str> = ["category:manga", "artist:shared"].into_iter().collect();
        // The only shared tag was `artist:shared`, stripped because `a` is an anthology — with
        // nothing else in common, the score must be 0, not 1.0.
        assert_eq!(jaccard_tag_score(&a, &b), 0.0);
    }

    #[test]
    fn jaccard_tag_score_ignores_circle_overlap_when_either_side_is_anthology() {
        let a: HashSet<&str> = ["category:anthology", "circle:shared"]
            .into_iter()
            .collect();
        let b: HashSet<&str> = ["category:doujinshi", "circle:shared"]
            .into_iter()
            .collect();
        assert_eq!(jaccard_tag_score(&a, &b), 0.0);
    }

    #[test]
    fn jaccard_tag_score_still_counts_non_artist_overlap_for_anthology_pairs() {
        let a: HashSet<&str> = ["category:anthology", "artist:only_on_a", "parody:shared"]
            .into_iter()
            .collect();
        let b: HashSet<&str> = ["category:anthology", "artist:only_on_b", "parody:shared"]
            .into_iter()
            .collect();
        // artist: values differ anyway, but even if they matched they'd be stripped — the real
        // point here is that `parody:shared` (not artist/circle) still counts normally.
        // intersection = {category:anthology, parody:shared} = 2,
        // union (after stripping both artist: values) = {category:anthology, parody:shared} = 2
        assert_eq!(jaccard_tag_score(&a, &b), 1.0);
    }

    #[test]
    fn filter_ignored_suggestions_drops_a_matching_fingerprint() {
        let s1 = GroupSuggestion::for_test(vec!["a".to_string(), "b".to_string()], None);
        let s2 = GroupSuggestion::for_test(vec!["c".to_string(), "d".to_string()], None);
        let ignored_fp = lanrurugi_storage::ignored_group_suggestions::fingerprint(
            &s1.archive_ids,
            s1.existing_tankoubon_id.as_deref(),
        );
        let ignored: HashSet<String> = [ignored_fp].into_iter().collect();

        let result = filter_ignored_suggestions(vec![s1, s2], &ignored);
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].archive_ids,
            vec!["c".to_string(), "d".to_string()]
        );
    }

    #[test]
    fn filter_ignored_suggestions_keeps_everything_when_nothing_is_ignored() {
        let s1 = GroupSuggestion::for_test(vec!["a".to_string(), "b".to_string()], None);
        let result = filter_ignored_suggestions(vec![s1], &HashSet::new());
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn filter_ignored_suggestions_distinguishes_by_existing_tankoubon_id() {
        // Same archive_ids, but one is a plain new-group suggestion and the other is an
        // add-to-existing-Tankoubon suggestion — ignoring one must not silently drop the other,
        // since they represent genuinely different actions (see `fingerprint`'s own docs).
        let new_group = GroupSuggestion::for_test(vec!["a".to_string()], None);
        let add_to_existing =
            GroupSuggestion::for_test(vec!["a".to_string()], Some("TANK_1".to_string()));
        let ignored_fp = lanrurugi_storage::ignored_group_suggestions::fingerprint(
            &new_group.archive_ids,
            new_group.existing_tankoubon_id.as_deref(),
        );
        let ignored: HashSet<String> = [ignored_fp].into_iter().collect();

        let result = filter_ignored_suggestions(vec![new_group, add_to_existing], &ignored);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].existing_tankoubon_id, Some("TANK_1".to_string()));
    }

    #[test]
    fn jaccard_of_disjoint_sets_is_zero() {
        let a: HashSet<&str> = ["category:manga", "artist:x"].into_iter().collect();
        let b: HashSet<&str> = ["category:cosplay", "artist:y"].into_iter().collect();
        assert_eq!(jaccard(&a, &b), 0.0);
    }

    #[test]
    fn jaccard_of_identical_sets_is_one() {
        let a: HashSet<&str> = ["category:manga", "artist:x"].into_iter().collect();
        assert_eq!(jaccard(&a, &a.clone()), 1.0);
    }

    #[test]
    fn jaccard_of_both_empty_is_zero_not_nan() {
        let a: HashSet<&str> = HashSet::new();
        let b: HashSet<&str> = HashSet::new();
        assert_eq!(jaccard(&a, &b), 0.0);
    }

    #[test]
    fn jaccard_partial_overlap() {
        let a: HashSet<&str> = ["x", "y", "z"].into_iter().collect();
        let b: HashSet<&str> = ["y", "z", "w"].into_iter().collect();
        // intersection {y,z} = 2, union {x,y,z,w} = 4
        assert_eq!(jaccard(&a, &b), 0.5);
    }

    #[test]
    fn category_conflict_detects_disjoint_categories() {
        let a: HashSet<&str> = ["category:manga", "artist:x"].into_iter().collect();
        let b: HashSet<&str> = ["category:cosplay", "artist:x"].into_iter().collect();
        assert!(
            category_conflict(&a, &b),
            "manga vs cosplay must conflict even with a shared artist tag"
        );
    }

    #[test]
    fn category_conflict_false_when_categories_overlap() {
        let a: HashSet<&str> = ["category:manga", "category:doujinshi"]
            .into_iter()
            .collect();
        let b: HashSet<&str> = ["category:doujinshi"].into_iter().collect();
        assert!(
            !category_conflict(&a, &b),
            "sharing at least one category value is not a conflict"
        );
    }

    #[test]
    fn category_conflict_false_when_either_side_has_no_category() {
        let with_category: HashSet<&str> = ["category:manga"].into_iter().collect();
        let without_category: HashSet<&str> = ["artist:x"].into_iter().collect();
        assert!(
            !category_conflict(&with_category, &without_category),
            "an untagged-category archive imposes no constraint"
        );
        assert!(!category_conflict(&without_category, &with_category));
    }

    #[test]
    fn group_by_similarity_only_keeps_groups_of_two_or_more() {
        // Three archives: a-b similar (both title and tags identical → blended score 1.0), c
        // isolated (zero similarity to either).
        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let va: Vec<f32> = vec![1.0, 0.0];
        let vb: Vec<f32> = vec![1.0, 0.0];
        let vc: Vec<f32> = vec![0.0, 1.0];
        let mut vectors: HashMap<&str, &[f32]> = HashMap::new();
        vectors.insert("a", &va);
        vectors.insert("b", &vb);
        vectors.insert("c", &vc);
        let tags_a: HashSet<&str> = ["category:manga"].into_iter().collect();
        let tags_b: HashSet<&str> = ["category:manga"].into_iter().collect();
        let tags_c: HashSet<&str> = HashSet::new();
        let mut tags: HashMap<&str, HashSet<&str>> = HashMap::new();
        tags.insert("a", tags_a);
        tags.insert("b", tags_b);
        tags.insert("c", tags_c);

        let title_tokens: HashMap<&str, HashSet<&str>> = HashMap::new();
        let groups = group_by_similarity(&ids, &vectors, &tags, &title_tokens);
        assert_eq!(
            groups.len(),
            1,
            "only the a-b pair forms a group, got {groups:?}"
        );
        assert_eq!(groups[0], vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn group_by_similarity_returns_empty_when_nothing_meets_threshold() {
        let ids = vec!["a".to_string(), "b".to_string()];
        let va: Vec<f32> = vec![1.0, 0.0];
        let vb: Vec<f32> = vec![0.0, 1.0];
        let mut vectors: HashMap<&str, &[f32]> = HashMap::new();
        vectors.insert("a", &va);
        vectors.insert("b", &vb);
        let tags: HashMap<&str, HashSet<&str>> = HashMap::new();
        let title_tokens: HashMap<&str, HashSet<&str>> = HashMap::new();

        let groups = group_by_similarity(&ids, &vectors, &tags, &title_tokens);
        assert!(groups.is_empty());
    }

    #[test]
    fn group_by_similarity_never_groups_across_a_category_conflict() {
        // a and b have IDENTICAL titles and tags except for category — would blend to a score of
        // 1.0 (well above threshold) if category weren't checked first.
        let ids = vec!["a".to_string(), "b".to_string()];
        let va: Vec<f32> = vec![1.0, 0.0];
        let vb: Vec<f32> = vec![1.0, 0.0];
        let mut vectors: HashMap<&str, &[f32]> = HashMap::new();
        vectors.insert("a", &va);
        vectors.insert("b", &vb);
        let tags_a: HashSet<&str> = ["category:manga", "artist:x"].into_iter().collect();
        let tags_b: HashSet<&str> = ["category:cosplay", "artist:x"].into_iter().collect();
        let mut tags: HashMap<&str, HashSet<&str>> = HashMap::new();
        tags.insert("a", tags_a);
        tags.insert("b", tags_b);

        let title_tokens: HashMap<&str, HashSet<&str>> = HashMap::new();
        let groups = group_by_similarity(&ids, &vectors, &tags, &title_tokens);
        assert!(
            groups.is_empty(),
            "a category conflict must block grouping even at otherwise-perfect similarity, got {groups:?}"
        );
    }

    #[test]
    fn title_tokens_splits_on_common_delimiters() {
        let tokens = title_tokens("示例社团 - 示例系列 角色甲·角色乙 JK");
        assert!(tokens.contains("示例社团"));
        assert!(tokens.contains("示例系列"));
        assert!(tokens.contains("角色甲"));
        assert!(tokens.contains("角色乙"));
        assert!(tokens.contains("JK"));
        assert!(
            !tokens.contains("-"),
            "the delimiter itself must not survive as a token"
        );
    }

    #[test]
    fn title_tokens_drops_empty_tokens_from_consecutive_delimiters() {
        let tokens = title_tokens("a  --  b");
        assert!(
            !tokens.contains(""),
            "consecutive delimiters must not produce an empty token"
        );
        assert!(tokens.contains("a"));
        assert!(tokens.contains("b"));
    }

    #[test]
    fn title_tokens_splits_on_underscore() {
        // Real, live-confirmed bug: a bracketed creator handle written with a trailing underscore
        // before the closing bracket (e.g. "[handle_ ] Subtitle") tokenized to a DIFFERENT string
        // than the same handle written plainly elsewhere ("handle_" vs "handle") without '_' in the
        // delimiter set — Jaccard treated them as non-overlapping, so the token-overlap bonus
        // silently never applied to that creator's own archives at all. Uses the real two titles
        // that surfaced this (env vars, not hardcoded — see .env.example's own comment); skips
        // when unset rather than failing, same shape as embedding.rs's own real-title tests.
        let (Ok(title_bracketed), Ok(title_plain)) = (
            std::env::var("LANRURUGI_TEST_TITLE_BRACKETED_HANDLE"),
            std::env::var("LANRURUGI_TEST_TITLE_PLAIN_HANDLE"),
        ) else {
            eprintln!(
                "skipping: LANRURUGI_TEST_TITLE_BRACKETED_HANDLE/LANRURUGI_TEST_TITLE_PLAIN_HANDLE not set — see .env.example"
            );
            return;
        };
        let with_underscore = title_tokens(&title_bracketed);
        let without_underscore = title_tokens(&title_plain);
        let shared = with_underscore.intersection(&without_underscore).count();
        assert!(
            shared > 0,
            "the two titles' own shared creator handle must survive as a common token, \
             got {with_underscore:?} vs {without_underscore:?}"
        );
        assert!(
            !with_underscore.iter().any(|t| t.contains('_')),
            "no surviving token should contain the delimiter character itself, got {with_underscore:?}"
        );
    }

    #[test]
    fn shares_cosplayer_true_when_a_value_matches() {
        let a: HashSet<&str> = ["cosplayer:xansoon", "category:cosplay"]
            .into_iter()
            .collect();
        let b: HashSet<&str> = ["cosplayer:xansoon", "character:privaty"]
            .into_iter()
            .collect();
        assert!(shares_cosplayer(&a, &b));
    }

    #[test]
    fn shares_cosplayer_false_when_values_differ() {
        let a: HashSet<&str> = ["cosplayer:xansoon"].into_iter().collect();
        let b: HashSet<&str> = ["cosplayer:someoneelse"].into_iter().collect();
        assert!(!shares_cosplayer(&a, &b));
    }

    #[test]
    fn group_by_similarity_shared_title_token_alone_is_not_enough() {
        // Orthogonal vectors (title_score 0.0) and no tag overlap: TOKEN_OVERLAP_WEIGHT * 1.0
        // (both titles tokenize to one shared token, Jaccard 1.0) = 0.2, still under
        // SIMILARITY_THRESHOLD (0.5) — the token bonus nudges a borderline pair, it doesn't
        // single-handedly force a match between otherwise-unrelated archives. Uses the real
        // creator handle from LANRURUGI_TEST_TITLE_PLAIN_HANDLE (env var, not hardcoded — see
        // .env.example); skips when unset.
        let Ok(handle_title) = std::env::var("LANRURUGI_TEST_TITLE_PLAIN_HANDLE") else {
            eprintln!("skipping: LANRURUGI_TEST_TITLE_PLAIN_HANDLE not set — see .env.example");
            return;
        };
        let handle_tokens = title_tokens(&handle_title);
        let ids = vec!["a".to_string(), "b".to_string()];
        let va: Vec<f32> = vec![1.0, 0.0];
        let vb: Vec<f32> = vec![0.0, 1.0];
        let mut vectors: HashMap<&str, &[f32]> = HashMap::new();
        vectors.insert("a", &va);
        vectors.insert("b", &vb);
        let tags: HashMap<&str, HashSet<&str>> = HashMap::new();
        let mut title_tokens: HashMap<&str, HashSet<&str>> = HashMap::new();
        title_tokens.insert("a", handle_tokens.clone());
        title_tokens.insert("b", handle_tokens);

        let groups = group_by_similarity(&ids, &vectors, &tags, &title_tokens);
        assert!(
            groups.is_empty(),
            "0.2 alone must stay under the 0.5 threshold, got {groups:?}"
        );
    }

    #[test]
    fn group_by_similarity_shared_title_token_pulls_borderline_pair_over_threshold() {
        // A borderline case, loosely calibrated to real numbers measured against
        // tankoubon_grouping_real_data_score_matrix: a same-creator pair's real title-embedding
        // similarity runs ~0.87-0.94 (this suite's own sentence embedding barely discriminates
        // between titles at all — see TITLE_WEIGHT's own doc comment) with real tag Jaccard
        // ~0.15-0.28. The real point of this test is simpler than exactly reproducing those
        // numbers: confirm the token bonus still contributes a real, non-trivial amount
        // (TOKEN_OVERLAP_WEIGHT * 1.0 = 0.2) on top of title+tag scoring under the new weights,
        // not that it single-handedly decides every borderline pair the way it did before
        // TITLE_WEIGHT/TAG_WEIGHT were rebalanced (see
        // group_by_similarity_shared_title_token_alone_is_not_enough for that half of the
        // contract). Uses the real creator handle from LANRURUGI_TEST_TITLE_PLAIN_HANDLE (env
        // var, not hardcoded — see .env.example); skips when unset.
        let Ok(handle_title) = std::env::var("LANRURUGI_TEST_TITLE_PLAIN_HANDLE") else {
            eprintln!("skipping: LANRURUGI_TEST_TITLE_PLAIN_HANDLE not set — see .env.example");
            return;
        };
        let handle_tokens = title_tokens(&handle_title);
        let ids = vec!["a".to_string(), "b".to_string()];
        let va: Vec<f32> = vec![1.0, 0.0];
        let vb: Vec<f32> = vec![0.9, (1.0f32 - 0.9 * 0.9).sqrt()];
        let mut vectors: HashMap<&str, &[f32]> = HashMap::new();
        vectors.insert("a", &va);
        vectors.insert("b", &vb);
        let tags_a: HashSet<&str> = ["character:a", "female:makeup", "category:cosplay"]
            .into_iter()
            .collect();
        let tags_b: HashSet<&str> = ["character:b", "female:makeup", "category:cosplay"]
            .into_iter()
            .collect();
        let mut tags: HashMap<&str, HashSet<&str>> = HashMap::new();
        tags.insert("a", tags_a);
        tags.insert("b", tags_b);
        let mut title_tokens: HashMap<&str, HashSet<&str>> = HashMap::new();
        title_tokens.insert("a", handle_tokens.clone());
        title_tokens.insert("b", handle_tokens);

        let groups = group_by_similarity(&ids, &vectors, &tags, &title_tokens);
        assert_eq!(
            groups.len(),
            1,
            "title(0.3*0.9) + tags(0.7*0.5) + token(0.2) = 0.82 must clear the threshold, got {groups:?}"
        );
    }

    #[test]
    fn group_by_similarity_shared_cosplayer_merges_despite_low_pairwise_title_similarity() {
        // Real, live-confirmed case (after artist_backfill.rs's LLM backfill gave five same-coser
        // archives the identical cosplayer: tag they were missing): different shoots/subjects per
        // archive mean titles vary a lot pairwise, so blended score alone doesn't clear
        // SIMILARITY_THRESHOLD for every pair — under the old soft-bonus-only design this
        // fragmented into a 2-group and a 3-group instead of one 5-group. Orthogonal-ish vectors
        // here simulate that low/inconsistent pairwise title similarity; only the shared
        // cosplayer: tag ties them together. Uses the real handle from
        // LANRURUGI_TEST_TITLE_PLAIN_HANDLE (env var, not hardcoded — see .env.example); skips
        // when unset, same shape as this module's other real-title tests.
        let Ok(handle_title) = std::env::var("LANRURUGI_TEST_TITLE_PLAIN_HANDLE") else {
            eprintln!("skipping: LANRURUGI_TEST_TITLE_PLAIN_HANDLE not set — see .env.example");
            return;
        };
        let handle = handle_title
            .split(['-', ' '])
            .next()
            .unwrap_or(&handle_title)
            .trim()
            .to_string();
        let cosplayer_tag = format!("cosplayer:{handle}");

        let ids: Vec<String> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let va: Vec<f32> = vec![1.0, 0.0, 0.0, 0.0, 0.0];
        let vb: Vec<f32> = vec![0.0, 1.0, 0.0, 0.0, 0.0];
        let vc: Vec<f32> = vec![0.0, 0.0, 1.0, 0.0, 0.0];
        let vd: Vec<f32> = vec![0.0, 0.0, 0.0, 1.0, 0.0];
        let ve: Vec<f32> = vec![0.0, 0.0, 0.0, 0.0, 1.0];
        let mut vectors: HashMap<&str, &[f32]> = HashMap::new();
        vectors.insert("a", &va);
        vectors.insert("b", &vb);
        vectors.insert("c", &vc);
        vectors.insert("d", &vd);
        vectors.insert("e", &ve);
        let mut tags: HashMap<&str, HashSet<&str>> = HashMap::new();
        for id in ["a", "b", "c", "d", "e"] {
            tags.insert(
                id,
                [cosplayer_tag.as_str(), "category:cosplay"]
                    .into_iter()
                    .collect(),
            );
        }
        let title_tokens: HashMap<&str, HashSet<&str>> = HashMap::new();

        let groups = group_by_similarity(&ids, &vectors, &tags, &title_tokens);
        assert_eq!(
            groups.len(),
            1,
            "all five archives share one cosplayer: value and must merge into a single group \
             despite orthogonal titles, got {groups:?}"
        );
        assert_eq!(
            groups[0].len(),
            5,
            "the single group must contain all five, got {groups:?}"
        );
    }

    #[test]
    fn group_by_similarity_does_not_chain_merge_unrelated_pairs() {
        // a-b similar (identical vectors, score 1.0), b-c similar (identical vectors, score 1.0),
        // but a-c NOT similar (orthogonal vectors, score 0.0) — a plain connected-components
        // approach would merge all three into one group purely through the a-b-c chain, even
        // though a and c were never actually compared as similar. A real, live-confirmed case of
        // exactly this: three different cosplayers' shoots chained into one 9-archive suggestion.
        // The clique-based approach must NOT produce a 3-member group here.
        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let va: Vec<f32> = vec![1.0, 0.0];
        let vb: Vec<f32> = vec![1.0, 0.0];
        let vc: Vec<f32> = vec![0.0, 1.0];
        let mut vectors: HashMap<&str, &[f32]> = HashMap::new();
        vectors.insert("a", &va);
        vectors.insert("b", &vb);
        vectors.insert("c", &vc);
        let tags: HashMap<&str, HashSet<&str>> = HashMap::new();
        let title_tokens: HashMap<&str, HashSet<&str>> = HashMap::new();

        // a-c alone would score 0.0 (orthogonal, no tag/token overlap) — well under threshold, so
        // a and c are never a valid pair. b-c is identical to the a-b pair in
        // group_by_similarity_only_keeps_groups_of_two_or_more (score 1.0), so b is similar to
        // BOTH a and c individually, but a and c are never similar to each other.
        let groups = group_by_similarity(&ids, &vectors, &tags, &title_tokens);
        assert!(
            groups.iter().all(|g| g.len() < 3),
            "a and c must never end up in the same group as each other via a b-chain, got {groups:?}"
        );
        assert!(
            !groups.iter().any(|g| g.contains(&"a".to_string()) && g.contains(&"c".to_string())),
            "a and c specifically must never co-occur in a group (they were never a similar pair), got {groups:?}"
        );
    }

    #[test]
    fn tankoubon_pair_gate_rejects_two_tankoubon_nodes() {
        assert!(!tankoubon_pair_gate("TANK_1111111111", "TANK_2222222222"));
    }

    #[test]
    fn tankoubon_pair_gate_allows_tankoubon_and_real_archive() {
        assert!(tankoubon_pair_gate("TANK_1111111111", "abc123"));
        assert!(tankoubon_pair_gate("abc123", "TANK_1111111111"));
    }

    #[test]
    fn tankoubon_pair_gate_allows_two_real_archives() {
        assert!(tankoubon_pair_gate("abc123", "def456"));
    }

    #[test]
    fn is_existing_tankoubon_node_detects_tank_prefix() {
        assert!(is_existing_tankoubon_node("TANK_1234567890"));
        assert!(!is_existing_tankoubon_node(
            "0c4f489c24651c79911cf61c6875d94c7a4dff43"
        ));
    }

    #[test]
    fn group_by_similarity_pulls_a_real_archive_into_an_existing_tankoubon_node() {
        // The "add to existing Tankoubon" scenario: a synthetic TANK_ node (standing in for an
        // already-real Tankoubon, built from its own members' averaged vector/tags) and one loose
        // real archive similar enough to it must end up in the same group — the group-splitting
        // logic in ai_group_suggestions then reads this as "suggest adding archive to that
        // Tankoubon" rather than "suggest a brand new Tankoubon". Orthogonal-ish vectors (so pure
        // title-embedding similarity alone would score well under SIMILARITY_THRESHOLD, matching
        // this module's own TITLE_WEIGHT=0.3 not being enough on its own) — only the shared
        // cosplayer: tag (carried over from the Tankoubon's own members into its synthetic node's
        // tag union) ties them together, the same real-world case artist_backfill.rs's LLM tag
        // backfill was built for.
        let ids = vec!["TANK_1111111111".to_string(), "loose1".to_string()];
        let v_tank: Vec<f32> = vec![1.0, 0.0];
        let v_loose: Vec<f32> = vec![0.0, 1.0];
        let mut vectors: HashMap<&str, &[f32]> = HashMap::new();
        vectors.insert("TANK_1111111111", &v_tank);
        vectors.insert("loose1", &v_loose);
        let tags_tank: HashSet<&str> = ["cosplayer:someone"].into_iter().collect();
        let tags_loose: HashSet<&str> = ["cosplayer:someone"].into_iter().collect();
        let mut tags: HashMap<&str, HashSet<&str>> = HashMap::new();
        tags.insert("TANK_1111111111", tags_tank);
        tags.insert("loose1", tags_loose);
        let title_tokens: HashMap<&str, HashSet<&str>> = HashMap::new();

        let groups = group_by_similarity(&ids, &vectors, &tags, &title_tokens);
        assert_eq!(groups.len(), 1, "got {groups:?}");
        assert!(groups[0].contains(&"TANK_1111111111".to_string()));
        assert!(groups[0].contains(&"loose1".to_string()));
    }

    #[test]
    fn group_by_similarity_never_merges_two_existing_tankoubon_nodes_together() {
        // Even with identical vectors (which would otherwise score a perfect blended match), two
        // TANK_ nodes must never end up in the same group as each other — merging two already-real
        // Tankoubons isn't this endpoint's job (see tankoubon_pair_gate's own docs).
        let ids = vec!["TANK_1111111111".to_string(), "TANK_2222222222".to_string()];
        let v_a: Vec<f32> = vec![1.0, 0.0];
        let v_b: Vec<f32> = vec![1.0, 0.0];
        let mut vectors: HashMap<&str, &[f32]> = HashMap::new();
        vectors.insert("TANK_1111111111", &v_a);
        vectors.insert("TANK_2222222222", &v_b);
        let tags: HashMap<&str, HashSet<&str>> = HashMap::new();
        let title_tokens: HashMap<&str, HashSet<&str>> = HashMap::new();

        let groups = group_by_similarity(&ids, &vectors, &tags, &title_tokens);
        assert!(
            groups.is_empty(),
            "two Tankoubon nodes alone (no real archive between them) must never form a group, got {groups:?}"
        );
    }

    #[test]
    fn group_by_similarity_prefers_existing_tankoubon_over_new_group_for_a_contested_archive() {
        // "contested" is similar to BOTH the existing Tankoubon node and a separate loose archive
        // (via two DIFFERENT shared cosplayer: values — cosplayer_a with the Tankoubon,
        // cosplayer_b with other_loose), but the Tankoubon node and other_loose are NOT similar to
        // each other at all (no shared tag, orthogonal vectors) — so at most one of the two can
        // ever claim "contested" into its own clique; they can't all three merge into one group the
        // way group_by_similarity_pulls_a_real_archive_into_an_existing_tankoubon_node's fully
        // mutual case could. Without the "prefer an existing-Tankoubon node as the clique's
        // starting point" rule, `remaining[0]`'s arbitrary id order could just as easily let
        // "contested" get claimed by other_loose's new-group clique first, leaving the Tankoubon
        // node with nothing to extend into and silently missing the "add to existing" suggestion.
        let ids = vec![
            "TANK_1111111111".to_string(),
            "contested".to_string(),
            "other_loose".to_string(),
        ];
        let v_tank: Vec<f32> = vec![1.0, 0.0, 0.0];
        let v_contested: Vec<f32> = vec![0.0, 1.0, 0.0];
        let v_other: Vec<f32> = vec![0.0, 0.0, 1.0];
        let mut vectors: HashMap<&str, &[f32]> = HashMap::new();
        vectors.insert("TANK_1111111111", &v_tank);
        vectors.insert("contested", &v_contested);
        vectors.insert("other_loose", &v_other);
        let tags_tank: HashSet<&str> = ["cosplayer:a"].into_iter().collect();
        let tags_contested: HashSet<&str> = ["cosplayer:a", "cosplayer:b"].into_iter().collect();
        let tags_other: HashSet<&str> = ["cosplayer:b"].into_iter().collect();
        let mut tags: HashMap<&str, HashSet<&str>> = HashMap::new();
        tags.insert("TANK_1111111111", tags_tank);
        tags.insert("contested", tags_contested);
        tags.insert("other_loose", tags_other);
        let title_tokens: HashMap<&str, HashSet<&str>> = HashMap::new();

        let groups = group_by_similarity(&ids, &vectors, &tags, &title_tokens);
        assert_eq!(groups.len(), 1, "got {groups:?}");
        assert!(
            groups[0].contains(&"TANK_1111111111".to_string()),
            "the single group must be anchored on the existing Tankoubon node, got {groups:?}"
        );
        assert!(
            groups[0].contains(&"contested".to_string()),
            "contested must be claimed by the Tankoubon node's clique, got {groups:?}"
        );
        assert!(
            !groups[0].contains(&"other_loose".to_string()),
            "other_loose shares no signal with the Tankoubon node itself and must be left out \
             once contested is claimed first, got {groups:?}"
        );
    }

    /// Debug tool, not a correctness test: prints every pairwise blended score (and its
    /// components) for a real, live-problematic set of archives, loaded from a JSON fixture file
    /// via `LANRURUGI_TEST_TANKOUBON_GROUPING_FIXTURE_PATH` (see `.env.example`'s own comment on
    /// that var — real titles/tags don't belong hardcoded in source). Lets
    /// `SIMILARITY_THRESHOLD`/`COSPLAYER_BONUS`/`TOKEN_OVERLAP_WEIGHT` tuning be checked against
    /// real numbers instead of guessing from live browser round-trips (which need a full
    /// container rebuild to pick up any Rust change). Skips (doesn't fail) when either the model
    /// files or the fixture path are absent, same shape as `embedding.rs`'s own
    /// `real_model_embeds_and_normalizes`.
    #[test]
    fn tankoubon_grouping_real_data_score_matrix() {
        let models_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/models");
        let model = models_dir.join("multilingual-e5-small_quantized.onnx");
        let tok = models_dir.join("e5-tokenizer.json");
        if !model.exists() || !tok.exists() {
            eprintln!("skipping: model files not present under data/models/ — run the model download first");
            return;
        }
        let Ok(fixture_path) = std::env::var("LANRURUGI_TEST_TANKOUBON_GROUPING_FIXTURE_PATH")
        else {
            eprintln!(
                "skipping: LANRURUGI_TEST_TANKOUBON_GROUPING_FIXTURE_PATH not set — see .env.example"
            );
            return;
        };
        #[derive(serde::Deserialize)]
        struct FixtureArchive {
            id: String,
            title: String,
            tags: String,
        }
        #[derive(serde::Deserialize)]
        struct Fixture {
            archives: Vec<FixtureArchive>,
        }
        // `cargo test -p <crate>` runs the test binary with its CWD set to that crate's own
        // manifest directory, NOT the workspace root — confirmed live (a relative
        // `testdata/...` path from `.env.local`, which reads correctly relative to the repo
        // root in every *other* context — a plain `cargo test` from the workspace root, `cargo
        // run --example`, this repo's own CI — silently failed to resolve here specifically).
        // Anchoring on `CARGO_MANIFEST_DIR` (this crate's own directory, known at compile time)
        // instead sidesteps needing to know which of those invocation shapes actually ran.
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture: Fixture = serde_json::from_str(
            &std::fs::read_to_string(workspace_root.join(&fixture_path)).unwrap(),
        )
        .unwrap();

        let embedder = lanrurugi_recommend::embedding::Embedder::load(&model, &tok, 1)
            .expect("model must load");

        // (id, title, embedding vector, tag set, title-token set) per fixture archive.
        type ScoredArchive<'a> = (String, String, Vec<f32>, HashSet<&'a str>, HashSet<&'a str>);
        let vectors: Vec<ScoredArchive> = fixture
            .archives
            .iter()
            .map(|a| {
                let normalized = lanrurugi_recommend::recommend::normalize_title(&a.title);
                let vector = embedder.embed(&normalized).expect("embed must succeed");
                (
                    a.id.clone(),
                    a.title.clone(),
                    vector,
                    tag_set(&a.tags),
                    title_tokens(&a.title),
                )
            })
            .collect();

        println!(
            "\n{:<12} {:<12} {:>6} {:>6} {:>5} {:>5} {:>6}  titles",
            "id_a", "id_b", "title", "tag", "cosp", "tok", "total"
        );
        for i in 0..vectors.len() {
            for j in (i + 1)..vectors.len() {
                let (id_a, title_a, vec_a, tags_a, tokens_a) = &vectors[i];
                let (id_b, title_b, vec_b, tags_b, tokens_b) = &vectors[j];
                let conflict = category_conflict(tags_a, tags_b);
                let title_score = lanrurugi_recommend::embedding::cosine_similarity(vec_a, vec_b);
                let tag_score = jaccard_tag_score(tags_a, tags_b);
                // Reference-only: `shares_cosplayer` now short-circuits `is_similar_pair` straight
                // to "similar" (see that function's own docs) rather than contributing this as a
                // score bonus — 0.3 is kept here purely so this debug printout still shows what
                // the old soft-bonus contribution would have been, for comparison against `total`.
                let cosp_bonus = if shares_cosplayer(tags_a, tags_b) {
                    0.3
                } else {
                    0.0
                };
                let tok_bonus = TOKEN_OVERLAP_WEIGHT * jaccard(tokens_a, tokens_b);
                let total =
                    TITLE_WEIGHT * title_score + TAG_WEIGHT * tag_score + cosp_bonus + tok_bonus;
                println!(
                    "{:<12} {:<12} {:>6.3} {:>6.3} {:>5.2} {:>5.2} {:>6.3}{}  {} <-> {}",
                    &id_a[..8.min(id_a.len())],
                    &id_b[..8.min(id_b.len())],
                    title_score,
                    tag_score,
                    cosp_bonus,
                    tok_bonus,
                    total,
                    if conflict { " [CONFLICT]" } else { "" },
                    title_a,
                    title_b,
                );
            }
        }
    }

    /// Runs the production grouping over a synthetic pool of `(id, title vector, title, tags)`.
    fn group_pool(pool: &[(&str, Vec<f32>, &str, &[&str])]) -> Vec<Vec<String>> {
        let settings = GroupingSettings::default();
        let ids: Vec<String> = pool.iter().map(|(id, ..)| id.to_string()).collect();
        let vectors: HashMap<&str, &[f32]> =
            pool.iter().map(|(id, v, ..)| (*id, v.as_slice())).collect();
        let tags: HashMap<&str, HashSet<&str>> = pool
            .iter()
            .map(|(id, _, _, t)| (*id, t.iter().copied().collect()))
            .collect();
        let tokens: HashMap<&str, HashSet<&str>> = pool
            .iter()
            .map(|(id, _, title, _)| (*id, title_tokens_for(title, &settings)))
            .collect();
        let scorer = Scorer::new(&vectors, &tags, &tokens, &settings);
        group_with(&ids, &scorer)
    }

    fn set<'a>(v: &[&'a str]) -> HashSet<&'a str> {
        v.iter().copied().collect()
    }

    #[test]
    fn creator_conflict_needs_two_known_disjoint_creator_sets() {
        assert!(creator_conflict(
            &set(&["artist:one"]),
            &set(&["artist:two"])
        ));
        // artist, circle and group are one set: the same handle under any of them matches.
        assert!(!creator_conflict(
            &set(&["artist:one"]),
            &set(&["circle:one"])
        ));
        assert!(!creator_conflict(
            &set(&["group:one"]),
            &set(&["circle:one"])
        ));
        assert!(creator_conflict(&set(&["group:one"]), &set(&["group:two"])));
        assert!(!creator_conflict(
            &set(&["artist:one", "circle:c"]),
            &set(&["artist:two", "circle:c"])
        ));
        // Unknown on either side is never evidence of a mismatch.
        assert!(!creator_conflict(
            &set(&["artist:one"]),
            &set(&["parody:p"])
        ));
        // Anthologies have volume-specific contributor lists.
        assert!(!creator_conflict(
            &set(&["artist:one", "category:anthology"]),
            &set(&["artist:two"])
        ));
    }

    #[test]
    fn a_shared_cosplayer_wins_over_a_creator_conflict() {
        let groups = group_pool(&[
            (
                "d1",
                vec![1.0, 0.0],
                "Set one",
                &["cosplayer:k", "artist:p1", "category:cosplay"],
            ),
            (
                "d2",
                vec![0.0, 1.0],
                "Set two",
                &["cosplayer:k", "artist:p2", "category:cosplay"],
            ),
        ]);
        assert_eq!(groups.len(), 1, "{groups:?}");
    }

    #[test]
    fn creator_gate_records_why_a_pair_was_blocked() {
        let settings = GroupingSettings::default();
        let (va, vb) = (vec![1.0, 0.0], vec![1.0, 0.0]);
        let vectors: HashMap<&str, &[f32]> = [("a", va.as_slice()), ("b", vb.as_slice())].into();
        let tags: HashMap<&str, HashSet<&str>> = [
            (
                "a",
                set(&["artist:one", "female:x", "parody:p", "character:c"]),
            ),
            (
                "b",
                set(&["artist:two", "female:x", "parody:p", "character:c"]),
            ),
        ]
        .into();
        let tokens: HashMap<&str, HashSet<&str>> = HashMap::new();
        let scorer = Scorer::new(&vectors, &tags, &tokens, &settings);
        let e = scorer.evidence("a", "b").unwrap();
        assert_eq!(e.blocked_by, Some(BlockedBy::Creator));
        assert_eq!(e.creator_match, CreatorMatch::Different);
        assert!(!e.similar);

        let off = GroupingSettings {
            creator_gate: false,
            ..GroupingSettings::default()
        };
        let scorer = Scorer::new(&vectors, &tags, &tokens, &off);
        assert!(scorer.evidence("a", "b").unwrap().similar);
    }

    #[test]
    fn a_tag_on_every_candidate_weighs_less_than_a_tag_on_two() {
        let pool = [
            set(&["language:chinese", "parody:p"]),
            set(&["language:chinese", "parody:p"]),
            set(&["language:chinese"]),
            set(&["language:chinese"]),
        ];
        let rarity = TagRarity::build(pool.iter());
        assert!(rarity.weight("language:chinese") < rarity.weight("parody:p"));
        assert!(rarity.weight("language:chinese") > 0.0);
    }

    #[test]
    fn release_markers_add_nothing_to_title_overlap() {
        let settings = GroupingSettings::default();
        let a = title_tokens_for("[Creator One] Story A [中国翻訳] [DL版]", &settings);
        let b = title_tokens_for("[Creator Two] Tale B [中国翻訳] [DL版]", &settings);
        let shared: HashSet<&str> = a.intersection(&b).copied().collect();
        assert_eq!(shared, set(&["Creator"]));
    }

    /// quickstart.md Q1: boilerplate look-alikes stay apart, real series still group.
    #[test]
    fn boilerplate_look_alikes_are_not_grouped_but_series_are() {
        let generic: &[&str] = &["language:chinese", "language:translated", "female:x"];
        let a_tags: Vec<&str> = [generic, &["artist:creator one"][..]].concat();
        let b_tags: Vec<&str> = [generic, &["artist:creator two"][..]].concat();
        let c_tags: &[&str] = &[
            "artist:creator three",
            "parody:p",
            "character:c",
            "language:chinese",
        ];
        let groups = group_pool(&[
            (
                "A",
                vec![0.9, 0.1, 0.0, 0.0],
                "[Creator One] Story A [中国翻訳] [DL版]",
                a_tags.as_slice(),
            ),
            (
                "B",
                vec![0.9, 0.1, 0.0, 0.0],
                "[Creator Two] Story B [中国翻訳] [DL版]",
                b_tags.as_slice(),
            ),
            (
                "C1",
                vec![0.0, 1.0, 0.0, 0.0],
                "[Creator Three] Series 1",
                c_tags,
            ),
            (
                "C2",
                vec![0.0, 1.0, 0.05, 0.0],
                "[Creator Three] Series 2",
                c_tags,
            ),
            (
                "C3",
                vec![0.0, 1.0, 0.1, 0.0],
                "[Creator Three] Series 3",
                c_tags,
            ),
            (
                "D1",
                vec![0.0, 0.0, 1.0, 0.0],
                "Shoot one",
                &["cosplayer:k", "artist:p1"],
            ),
            (
                "D2",
                vec![0.0, 0.0, 0.0, 1.0],
                "Shoot two",
                &["cosplayer:k", "artist:p2"],
            ),
            (
                "E1",
                vec![0.5, 0.0, 0.0, 0.5],
                "Anthology Vol.1",
                &[
                    "category:anthology",
                    "artist:x1",
                    "parody:q",
                    "language:chinese",
                ],
            ),
            (
                "E2",
                vec![0.5, 0.0, 0.0, 0.5],
                "Anthology Vol.2",
                &[
                    "category:anthology",
                    "artist:x2",
                    "parody:q",
                    "language:chinese",
                ],
            ),
        ]);
        let find = |id: &str| groups.iter().find(|g| g.iter().any(|m| m == id));
        let a_group = find("A");
        assert!(
            a_group.is_none_or(|g| !g.iter().any(|m| m == "B")),
            "A and B must never share a group: {groups:?}"
        );
        let c = find("C1").expect("C series grouped");
        assert_eq!(c.len(), 3, "{groups:?}");
        let d = find("D1").expect("cosplay sets grouped");
        assert!(d.iter().any(|m| m == "D2"), "{groups:?}");
        let e = find("E1").expect("anthology issues still groupable");
        assert!(e.iter().any(|m| m == "E2"), "{groups:?}");
    }
}
