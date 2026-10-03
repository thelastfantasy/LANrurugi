//! Deciding which discovered candidates survive a subscription's rules.
//!
//! Deliberately pure: no Redis, no plugin runtime, no clock. Rule evaluation is where a quiet bug
//! costs the most — a too-strict rule produces no error, no failed download, and no symptom at all
//! until someone notices a work that never arrived — so it is isolated here to be tested
//! exhaustively.
//!
//! Every rejection names the rule responsible (FR-011) rather than returning a bare boolean. A
//! filter that silently drops things is indistinguishable from a broken subscription.

use lanrurugi_plugin::protocol::DiscoveredCandidate;
use lanrurugi_storage::subscriptions::Filters;

/// Why a candidate was rejected, in terms the user can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionRule {
    MissingRequiredTag(String),
    ExcludedTag(String),
    BelowMinimumRating,
    ExcludedCategory(String),
}

impl RejectionRule {
    /// A stable identifier for the rule, suitable for storing and for the frontend to translate.
    pub fn as_key(&self) -> String {
        match self {
            RejectionRule::MissingRequiredTag(t) => format!("missing_required_tag:{t}"),
            RejectionRule::ExcludedTag(t) => format!("excluded_tag:{t}"),
            RejectionRule::BelowMinimumRating => "below_minimum_rating".to_string(),
            RejectionRule::ExcludedCategory(c) => format!("excluded_category:{c}"),
        }
    }
}

/// What a candidate's own tags are compared against. Comparison is case-insensitive and ignores
/// surrounding whitespace, because tag text arrives from scraped listings where neither is reliable
/// — a rule that works on one source but silently fails on another would be worse than no rule.
fn tag_matches(candidate_tag: &str, rule_tag: &str) -> bool {
    candidate_tag.trim().eq_ignore_ascii_case(rule_tag.trim())
}

fn has_tag(candidate: &DiscoveredCandidate, rule_tag: &str) -> bool {
    candidate.tags.iter().any(|t| tag_matches(t, rule_tag))
}

/// Applies `filters` to one candidate.
///
/// `candidate_categories` is what the library already knows about this work (empty when it is not
/// held at all). Exclusion by category only means anything for a work already in the library — a
/// brand-new work belongs to no category yet, so that rule cannot fire for it.
///
/// Returns the rule that rejected it, or `None` when it survives.
pub fn evaluate(
    candidate: &DiscoveredCandidate,
    filters: &Filters,
    candidate_categories: &[String],
) -> Option<RejectionRule> {
    // Exclusions first: a candidate carrying an excluded tag is out regardless of what else it has,
    // and saying *that* is more useful than reporting a missing required tag it also lacked.
    for excluded in &filters.excluded_tags {
        if has_tag(candidate, excluded) {
            return Some(RejectionRule::ExcludedTag(excluded.clone()));
        }
    }

    for required in &filters.required_tags {
        if !has_tag(candidate, required) {
            return Some(RejectionRule::MissingRequiredTag(required.clone()));
        }
    }

    if let Some(minimum) = filters.minimum_rating {
        // A candidate with no rating is NOT rejected. Listings often omit ratings for recent works,
        // and treating "unknown" as "below threshold" would silently hide exactly the new works a
        // subscription exists to catch. A later check can reconsider it once a rating appears.
        if let Some(rating) = candidate.rating {
            if rating < minimum {
                return Some(RejectionRule::BelowMinimumRating);
            }
        }
    }

    for excluded in &filters.excluded_categories {
        if candidate_categories
            .iter()
            .any(|c| c.trim().eq_ignore_ascii_case(excluded.trim()))
        {
            return Some(RejectionRule::ExcludedCategory(excluded.clone()));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(tags: &[&str], rating: Option<f32>) -> DiscoveredCandidate {
        DiscoveredCandidate {
            source: "e-hentai.org/g/1/a".to_string(),
            title: Some("t".to_string()),
            posted_at: None,
            rating,
            tags: tags.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn filters(req: &[&str], exc: &[&str], min: Option<f32>, cats: &[&str]) -> Filters {
        Filters {
            required_tags: req.iter().map(|s| s.to_string()).collect(),
            excluded_tags: exc.iter().map(|s| s.to_string()).collect(),
            minimum_rating: min,
            excluded_categories: cats.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn a_candidate_with_no_rules_against_it_survives() {
        assert_eq!(
            evaluate(&candidate(&["a"], None), &Filters::default(), &[]),
            None
        );
    }

    #[test]
    fn an_excluded_tag_rejects_and_names_itself() {
        assert_eq!(
            evaluate(
                &candidate(&["a", "b"], None),
                &filters(&[], &["b"], None, &[]),
                &[]
            ),
            Some(RejectionRule::ExcludedTag("b".to_string()))
        );
    }

    #[test]
    fn a_missing_required_tag_rejects_and_names_itself() {
        assert_eq!(
            evaluate(
                &candidate(&["a"], None),
                &filters(&["z"], &[], None, &[]),
                &[]
            ),
            Some(RejectionRule::MissingRequiredTag("z".to_string()))
        );
    }

    /// When a candidate both carries an excluded tag and lacks a required one, the exclusion is the
    /// more useful thing to report — it is the rule the user most likely wrote deliberately.
    #[test]
    fn exclusion_is_reported_before_a_missing_requirement() {
        let verdict = evaluate(
            &candidate(&["bad"], None),
            &filters(&["needed"], &["bad"], None, &[]),
            &[],
        );
        assert_eq!(verdict, Some(RejectionRule::ExcludedTag("bad".to_string())));
    }

    #[test]
    fn tag_comparison_ignores_case_and_surrounding_whitespace() {
        assert_eq!(
            evaluate(
                &candidate(&["  BigTag "], None),
                &filters(&["bigtag"], &[], None, &[]),
                &[]
            ),
            None,
            "a required tag must match despite case and padding from scraped listings"
        );
        assert_eq!(
            evaluate(
                &candidate(&["BigTag"], None),
                &filters(&[], &["  bigtag"], None, &[]),
                &[]
            ),
            Some(RejectionRule::ExcludedTag("  bigtag".to_string()))
        );
    }

    #[test]
    fn a_rating_below_the_floor_is_rejected() {
        assert_eq!(
            evaluate(
                &candidate(&[], Some(3.0)),
                &filters(&[], &[], Some(4.0), &[]),
                &[]
            ),
            Some(RejectionRule::BelowMinimumRating)
        );
        assert_eq!(
            evaluate(
                &candidate(&[], Some(4.0)),
                &filters(&[], &[], Some(4.0), &[]),
                &[]
            ),
            None,
            "the floor is inclusive"
        );
    }

    /// Listings commonly omit a rating for brand-new works — exactly the ones a subscription exists
    /// to catch. Treating "unknown" as "too low" would hide them permanently.
    #[test]
    fn an_unrated_candidate_is_not_rejected_by_a_rating_floor() {
        assert_eq!(
            evaluate(
                &candidate(&[], None),
                &filters(&[], &[], Some(4.5), &[]),
                &[]
            ),
            None
        );
    }

    #[test]
    fn an_excluded_category_rejects_only_when_the_work_is_in_it() {
        let f = filters(&[], &[], None, &["Archive"]);
        assert_eq!(
            evaluate(&candidate(&[], None), &f, &["Archive".to_string()]),
            Some(RejectionRule::ExcludedCategory("Archive".to_string()))
        );
        assert_eq!(
            evaluate(&candidate(&[], None), &f, &["Other".to_string()]),
            None
        );
        assert_eq!(
            evaluate(&candidate(&[], None), &f, &[]),
            None,
            "a work not yet in the library belongs to no category, so this rule cannot fire"
        );
    }

    #[test]
    fn every_rule_produces_a_distinct_stable_key() {
        let keys = [
            RejectionRule::MissingRequiredTag("x".into()).as_key(),
            RejectionRule::ExcludedTag("x".into()).as_key(),
            RejectionRule::BelowMinimumRating.as_key(),
            RejectionRule::ExcludedCategory("x".into()).as_key(),
        ];
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len(), "rule keys must not collide");
    }
}
