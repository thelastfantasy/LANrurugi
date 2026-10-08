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
use lanrurugi_storage::subscriptions::{Condition, FieldOperator, FieldRule, Filters, RuleValue};

/// Why a candidate was rejected, in terms the user can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionRule {
    MissingRequiredTag(String),
    ExcludedTag(String),
    BelowMinimumRating,
    ExcludedCategory(String),
    /// A rule over a declared field failed. Carries the field so a too-strict rule is diagnosable.
    FieldRule(String),
    /// The rule names a field this source's candidates do not carry — reported rather than dropped
    /// (which would silently widen the subscription) or treated as satisfied (silently narrowing it).
    FieldUnavailable(String),
}

impl RejectionRule {
    /// A stable identifier for the rule, suitable for storing and for the frontend to translate.
    pub fn as_key(&self) -> String {
        match self {
            RejectionRule::MissingRequiredTag(t) => format!("missing_required_tag:{t}"),
            RejectionRule::ExcludedTag(t) => format!("excluded_tag:{t}"),
            RejectionRule::BelowMinimumRating => "below_minimum_rating".to_string(),
            RejectionRule::ExcludedCategory(c) => format!("excluded_category:{c}"),
            RejectionRule::FieldRule(f) => format!("field_rule:{f}"),
            RejectionRule::FieldUnavailable(f) => format!("field_unavailable:{f}"),
        }
    }
}

/// What a candidate's rules say about it right now.
///
/// `TooSoon` is **not** a rejection, and the distinction is load-bearing: a rejected candidate is
/// recorded as seen and never reconsidered, whereas one still inside its waiting period must be looked
/// at again on a later check. Collapsing the two would turn "wait and see" into "never".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    Rejected(RejectionRule),
    /// Published too recently; reconsider once `minimum_age_secs` has elapsed.
    TooSoon,
}

/// Parses the `posted_at` shapes a listing may use, to unix seconds.
///
/// Returns `None` for anything unrecognised, and the age rule then treats the work as old enough —
/// failing open on purpose. Failing closed would make one unparsed date format hold a subscription's
/// entire catalogue back indefinitely, with nothing in the UI explaining why.
pub fn parse_posted_at(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    // E-Hentai's listing format, verified live: "2026-10-03 13:25" (UTC).
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M") {
        return Some(dt.and_utc().timestamp());
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S") {
        return Some(dt.and_utc().timestamp());
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt.timestamp());
    }
    // Some sources hand back a bare epoch.
    raw.parse::<i64>().ok()
}

/// One candidate field's current value, as the rule engine sees it.
///
/// Absence is its own case rather than an empty value: a rule over a field a candidate does not carry
/// must hold the work for later, not reject it, and the two are indistinguishable once absence is
/// flattened into a default.
enum FieldValue<'a> {
    Number(f64),
    Text(&'a str),
    /// Several spellings of one value — a title in each language the source gave. A rule holds when
    /// any of them satisfies it.
    AnyText(Vec<&'a str>),
    /// Borrowed where the candidate already holds the list, owned where it is derived — a namespace
    /// field like `language` is built from the tags rather than stored separately.
    List(std::borrow::Cow<'a, [String]>),
    /// A date, already parsed to unix seconds.
    Date(i64),
    Absent,
}

/// Reads a candidate field by the name a rule used.
///
/// Unknown names return `Absent`, which rule 3 then reports as inapplicable — an extension that
/// stopped writing a field leaves rules referencing it visible rather than silently satisfied.
fn field_value<'a>(candidate: &'a DiscoveredCandidate, field: &str) -> FieldValue<'a> {
    match field {
        "posted_at" => candidate
            .posted_at
            .as_deref()
            .and_then(parse_posted_at)
            .map_or(FieldValue::Absent, FieldValue::Date),
        "rating" => candidate
            .rating
            .map_or(FieldValue::Absent, |r| FieldValue::Number(r as f64)),
        "pages" => candidate
            .pages
            .map_or(FieldValue::Absent, |p| FieldValue::Number(p as f64)),
        // Any of the titles may satisfy a rule: a work is the same work whichever language it is
        // named in, so a rule written against the Japanese title must not fail because the English
        // one was compared instead.
        "title" => {
            if candidate.title.is_empty() {
                FieldValue::Absent
            } else {
                FieldValue::AnyText(candidate.title.values().map(String::as_str).collect())
            }
        }
        "category" => candidate
            .category
            .as_deref()
            .map_or(FieldValue::Absent, FieldValue::Text),
        "uploader" => candidate
            .uploader
            .as_deref()
            .map_or(FieldValue::Absent, FieldValue::Text),
        "tags" => FieldValue::List(std::borrow::Cow::Borrowed(&candidate.tags)),
        // A namespace read as its own field: `language` is every `language:` tag's value. Expressing
        // "in Chinese or Japanese, or carrying no language at all" needs this — over the raw tag list
        // the last part is unsayable, since excluding specific tags cannot say "none of this kind".
        other => match namespace_values(candidate, other) {
            Some(values) => FieldValue::List(std::borrow::Cow::Owned(values)),
            None => FieldValue::Absent,
        },
    }
}

/// The values a candidate carries under one tag namespace, e.g. `["chinese", "translated"]` for
/// `language`.
///
/// Returns an empty list rather than `Absent` when the namespace is genuinely unused: "has no
/// language tag" is a real answer a rule can match on, and reporting it as unknown would hold the
/// work for reconsideration forever instead.
fn namespace_values(candidate: &DiscoveredCandidate, namespace: &str) -> Option<Vec<String>> {
    if !TAG_NAMESPACES.contains(&namespace) {
        return None;
    }
    let prefix = format!("{namespace}:");
    Some(
        candidate
            .tags
            .iter()
            .filter_map(|t| {
                let t = t.trim();
                t.get(..prefix.len())
                    .filter(|head| head.eq_ignore_ascii_case(&prefix))
                    .map(|_| t[prefix.len()..].trim().to_string())
            })
            .collect(),
    )
}

/// Tag namespaces offered as fields of their own.
///
/// A fixed set rather than whatever a listing happens to contain: the settings form has to offer
/// these before any check has run, and a source's full namespace vocabulary is not known until one
/// has. These are the ones every supported source uses.
pub const TAG_NAMESPACES: &[&str] = &[
    "language",
    "artist",
    "group",
    "parody",
    "character",
    "female",
    "male",
    "other",
];

/// How a tag written by the user is compared against one written by the source.
///
/// The two are rarely spelled identically. Beyond case and surrounding space, the same tag appears
/// full-width or half-width, in kana of either script, and in old or new kanji forms depending on who
/// typed it — a rule written as `artist:竜騎士` would otherwise never match a listing that spells it
/// `artist:龍騎士`. The library already folds these for search; the same folding is applied here so a
/// subscription rule behaves like a search for the same text.
///
/// `fold` is supplied rather than built here so this module stays free of I/O and state, and so the
/// tests can exercise the comparison without constructing one.
fn text_eq(a: &str, b: &str, fold: &dyn Fn(&str) -> String) -> bool {
    let (a, b) = (a.trim(), b.trim());
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    // Compared case-insensitively after folding too: folding normalises script and width, not case.
    fold(a).eq_ignore_ascii_case(&fold(b))
}

/// Applies one rule. `None` means the value was absent, which is neither pass nor fail.
fn rule_holds(
    value: &FieldValue<'_>,
    operator: FieldOperator,
    expected: &RuleValue,
    now: i64,
    fold: &dyn Fn(&str) -> String,
) -> Option<bool> {
    use FieldOperator as Op;
    if let FieldValue::AnyText(options) = value {
        // Any spelling satisfying the rule satisfies it; `None` only if every one was unknown.
        let mut saw_answer = false;
        for option in options {
            match rule_holds(&FieldValue::Text(option), operator, expected, now, fold) {
                Some(true) => return Some(true),
                Some(false) => saw_answer = true,
                None => {}
            }
        }
        return saw_answer.then_some(false);
    }
    match (value, operator, expected) {
        (FieldValue::Absent, _, _) => None,

        (FieldValue::Number(n), Op::Gte, RuleValue::Number(e)) => Some(n >= e),
        (FieldValue::Number(n), Op::Lte, RuleValue::Number(e)) => Some(n <= e),
        (FieldValue::Number(n), Op::Eq, RuleValue::Number(e)) => Some((n - e).abs() < f64::EPSILON),

        // Durations, not timestamps: `older_than: 10800` is "published at least three hours ago",
        // which means the same thing on every check.
        (FieldValue::Date(posted), Op::OlderThan, RuleValue::Number(secs)) => {
            Some(now.saturating_sub(*posted) as f64 >= *secs)
        }
        (FieldValue::Date(posted), Op::NewerThan, RuleValue::Number(secs)) => {
            Some((now.saturating_sub(*posted) as f64) <= *secs)
        }

        (FieldValue::Text(t), Op::Equals, RuleValue::Text(e)) => Some(text_eq(t, e, fold)),
        (FieldValue::Text(t), Op::Contains, RuleValue::Text(e)) => Some(
            fold(t)
                .to_lowercase()
                .contains(&fold(e.trim()).to_lowercase()),
        ),
        (FieldValue::Text(t), Op::In, RuleValue::List(opts)) => {
            Some(opts.iter().any(|o| text_eq(t, o, fold)))
        }
        (FieldValue::Text(t), Op::NotIn, RuleValue::List(opts)) => {
            Some(!opts.iter().any(|o| text_eq(t, o, fold)))
        }

        // Presence rather than content. The value side is ignored — the UI sends `true`, but a rule
        // stored with anything else still means the same thing.
        (FieldValue::List(have), Op::IsEmpty, _) => Some(have.is_empty()),
        (FieldValue::List(have), Op::IsNotEmpty, _) => Some(!have.is_empty()),

        (FieldValue::List(have), Op::IncludesAll, RuleValue::List(want)) => Some(
            want.iter()
                .all(|w| have.iter().any(|h| text_eq(h, w, fold))),
        ),
        (FieldValue::List(have), Op::IncludesNone, RuleValue::List(want)) => Some(
            !want
                .iter()
                .any(|w| have.iter().any(|h| text_eq(h, w, fold))),
        ),
        // A single value is accepted where a list is expected, so a one-item rule need not be wrapped.
        (FieldValue::List(have), Op::IncludesAll, RuleValue::Text(w)) => {
            Some(have.iter().any(|h| text_eq(h, w, fold)))
        }
        (FieldValue::List(have), Op::IncludesNone, RuleValue::Text(w)) => {
            Some(!have.iter().any(|h| text_eq(h, w, fold)))
        }

        (FieldValue::Number(n), Op::Is, RuleValue::Bool(e)) => Some((*n != 0.0) == *e),

        // A type/operator pairing the UI never produces. Treated as unmet rather than as passing, so a
        // malformed rule cannot silently widen a subscription.
        _ => Some(false),
    }
}

/// One node's answer. Three-valued, because "cannot tell yet" is not "no".
#[derive(Debug, Clone, PartialEq)]
enum Verdict {
    Holds,
    /// Carries why the rule failed, so a too-strict condition is diagnosable (FR-011). The reason is
    /// resolved here, where the candidate is still in scope, rather than in `evaluate_outcome` —
    /// naming the specific tag that was excluded needs the candidate's own tag list.
    Fails(RejectionRule),
    /// Premature or not yet knowable. Propagates to the whole tree — see `evaluate_condition`.
    Pending,
}

/// Why one rule failed, phrased so the user can act on it.
///
/// A tag rule names the tag itself (`excluded_tag:language:spanish`) rather than just its field
/// (`field_rule:tags`): "one of your tag rules rejected this" is not a diagnosis, and the tag is
/// exactly what the user would edit. Shape-based, not origin-based — a hand-written condition with
/// the same shape gets the same specific reason as the superseded fixed filters it mirrors.
fn rejection_for(
    rule: &FieldRule,
    candidate: &DiscoveredCandidate,
    fold: &dyn Fn(&str) -> String,
) -> RejectionRule {
    match (rule.field.as_str(), rule.operator) {
        ("tags", FieldOperator::IncludesNone) => first_list_item(rule, candidate, fold, true)
            .map_or(
                RejectionRule::FieldRule("tags".into()),
                RejectionRule::ExcludedTag,
            ),
        ("tags", FieldOperator::IncludesAll) => first_list_item(rule, candidate, fold, false)
            .map_or(
                RejectionRule::FieldRule("tags".into()),
                RejectionRule::MissingRequiredTag,
            ),
        ("rating", FieldOperator::Gte) => RejectionRule::BelowMinimumRating,
        _ => RejectionRule::FieldRule(rule.field.clone()),
    }
}

/// The first of the rule's own values that the candidate's list actually fails on — present when
/// `wanted_present` (an exclusion), absent otherwise (a requirement). `None` when the field is not a
/// list at all, which leaves the caller with the generic field-level reason.
fn first_list_item(
    rule: &FieldRule,
    candidate: &DiscoveredCandidate,
    fold: &dyn Fn(&str) -> String,
    wanted_present: bool,
) -> Option<String> {
    let FieldValue::List(have) = field_value(candidate, &rule.field) else {
        return None;
    };
    let wanted: Vec<&String> = match &rule.value {
        RuleValue::List(values) => values.iter().collect(),
        RuleValue::Text(value) => vec![value],
        _ => return None,
    };
    wanted
        .into_iter()
        .find(|w| have.iter().any(|h| text_eq(h, w, fold)) == wanted_present)
        .cloned()
}

/// Evaluates one node.
///
/// **`Pending` wins over everything, including an `Any` branch that already holds.** A rejection is
/// permanent: it records the work as seen and it is never looked at again. "Not yet three hours old"
/// and "no rating yet" are answers that change on their own, so acting on a tree containing one would
/// settle a question that was still open. Waiting costs one more check; deciding early cannot be undone.
fn evaluate_condition(
    condition: &Condition,
    candidate: &DiscoveredCandidate,
    now: i64,
    fold: &dyn Fn(&str) -> String,
) -> Verdict {
    match condition {
        Condition::Rule(rule) => {
            match rule_holds(
                &field_value(candidate, &rule.field),
                rule.operator,
                &rule.value,
                now,
                fold,
            ) {
                Some(true) => Verdict::Holds,
                Some(false) if rule.operator == FieldOperator::OlderThan => Verdict::Pending,
                Some(false) => Verdict::Fails(rejection_for(rule, candidate, fold)),
                // The field is absent — on this candidate, or because the source stopped providing
                // it. Either way the answer may differ later, and a rejection would not.
                None => Verdict::Pending,
            }
        }

        Condition::All { children } => {
            let mut first_failure = None;
            for child in children {
                match evaluate_condition(child, candidate, now, fold) {
                    Verdict::Holds => {}
                    Verdict::Pending => return Verdict::Pending,
                    Verdict::Fails(f) => {
                        // Kept rather than returned at once: a later child may still be pending, and
                        // pending outranks a failure.
                        if first_failure.is_none() {
                            first_failure = Some(f);
                        }
                    }
                }
            }
            first_failure.map_or(Verdict::Holds, Verdict::Fails)
        }

        Condition::Any { children } => {
            if children.is_empty() {
                // An empty "any" is unsatisfiable, not vacuously true: it is an unfinished condition,
                // and treating it as matching everything would silently widen the subscription.
                return Verdict::Fails(RejectionRule::FieldRule("any".into()));
            }
            let mut holds = false;
            let mut pending = false;
            let mut first_failure = None;
            for child in children {
                match evaluate_condition(child, candidate, now, fold) {
                    Verdict::Holds => holds = true,
                    Verdict::Pending => pending = true,
                    Verdict::Fails(f) => {
                        if first_failure.is_none() {
                            first_failure = Some(f);
                        }
                    }
                }
            }
            // Pending first even when a sibling already holds: the pending branch might also come to
            // hold, and the tree as a whole is not settled until nothing in it can still change.
            if pending {
                Verdict::Pending
            } else if holds {
                Verdict::Holds
            } else {
                Verdict::Fails(
                    first_failure.unwrap_or_else(|| RejectionRule::FieldRule("any".into())),
                )
            }
        }

        Condition::Not { child } => match evaluate_condition(child, candidate, now, fold) {
            Verdict::Holds => Verdict::Fails(RejectionRule::FieldRule(describe(child))),
            Verdict::Fails(_) => Verdict::Holds,
            // Negating an unknown gives an unknown: a work excluded on the strength of a value that
            // has not arrived yet would be excluded for the wrong reason.
            Verdict::Pending => Verdict::Pending,
        },
    }
}

/// A short name for a node, used when reporting which part of a condition rejected a candidate.
fn describe(condition: &Condition) -> String {
    match condition {
        Condition::Rule(r) => r.field.clone(),
        Condition::All { .. } => "all".into(),
        Condition::Any { .. } => "any".into(),
        Condition::Not { .. } => "not".into(),
    }
}

/// Full evaluation, including rules whose answer depends on the clock.
pub fn evaluate_outcome(
    candidate: &DiscoveredCandidate,
    filters: &Filters,
    candidate_categories: &[String],
    now: i64,
    fold: &dyn Fn(&str) -> String,
) -> Outcome {
    if let Some(condition) = filters.effective_condition() {
        match evaluate_condition(&condition, candidate, now, fold) {
            Verdict::Holds => {}
            Verdict::Pending => return Outcome::TooSoon,
            Verdict::Fails(rule) => return Outcome::Rejected(rule),
        }
    }

    // The excluded-category rule still runs separately: it compares against the categories the
    // *library* already has for this work, which is not a candidate field at all.
    for excluded in &filters.excluded_categories {
        if candidate_categories
            .iter()
            .any(|c| c.trim().eq_ignore_ascii_case(excluded.trim()))
        {
            return Outcome::Rejected(RejectionRule::ExcludedCategory(excluded.clone()));
        }
    }

    Outcome::Accepted
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_storage::subscriptions::FieldRule;

    fn candidate(tags: &[&str], rating: Option<f32>) -> DiscoveredCandidate {
        DiscoveredCandidate {
            source: "e-hentai.org/g/1/a".to_string(),
            title: std::collections::HashMap::from([("origin".to_string(), "t".to_string())]),
            posted_at: None,
            rating,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            category: None,
            uploader: None,
            pages: None,
        }
    }

    fn dated(posted: &str, rating: Option<f32>) -> DiscoveredCandidate {
        let mut c = candidate(&["a"], rating);
        c.posted_at = Some(posted.to_string());
        c
    }

    fn rule(field: &str, operator: FieldOperator, value: RuleValue) -> Condition {
        Condition::Rule(FieldRule {
            field: field.into(),
            operator,
            value,
        })
    }

    fn with(condition: Condition) -> Filters {
        Filters {
            condition: Some(condition),
            ..Filters::default()
        }
    }

    /// 2023-11-14 22:00 UTC is ~13 minutes before this; 12:00 the same day ~10 hours before.
    const NOW: i64 = 1_700_000_000;

    /// Most tests only care about the rule logic, so they compare without folding.
    fn plain(t: &str) -> String {
        t.to_string()
    }

    fn age(secs: f64) -> Condition {
        rule(
            "posted_at",
            FieldOperator::OlderThan,
            RuleValue::Number(secs),
        )
    }

    fn rating_at_least(min: f64) -> Condition {
        rule("rating", FieldOperator::Gte, RuleValue::Number(min))
    }

    // ── The user's own stated case ──────────────────────────────────────────────────────────────

    /// "Download only once it has been up three hours and scored at least four."
    #[test]
    fn a_work_inside_its_waiting_period_is_held_not_rejected() {
        let f = with(Condition::All {
            children: vec![age(3.0 * 3600.0), rating_at_least(4.0)],
        });
        assert_eq!(
            evaluate_outcome(&dated("2023-11-14 22:00", Some(5.0)), &f, &[], NOW, &plain),
            Outcome::TooSoon
        );
    }

    #[test]
    fn the_same_work_is_judged_on_its_rating_once_old_enough() {
        let f = with(Condition::All {
            children: vec![age(3.0 * 3600.0), rating_at_least(4.0)],
        });
        assert_eq!(
            evaluate_outcome(&dated("2023-11-14 12:00", Some(4.5)), &f, &[], NOW, &plain),
            Outcome::Accepted
        );
        assert_eq!(
            evaluate_outcome(&dated("2023-11-14 12:00", Some(2.0)), &f, &[], NOW, &plain),
            Outcome::Rejected(RejectionRule::BelowMinimumRating)
        );
    }

    // ── Tree structure: and / or / not ─────────────────────────────────────────────────────────

    #[test]
    fn any_holds_when_one_branch_holds() {
        let f = with(Condition::Any {
            children: vec![
                rating_at_least(4.5),
                rule("pages", FieldOperator::Gte, RuleValue::Number(100.0)),
            ],
        });
        let mut c = candidate(&["a"], Some(2.0));
        c.pages = Some(200);
        assert_eq!(
            evaluate_outcome(&c, &f, &[], NOW, &plain),
            Outcome::Accepted
        );
    }

    #[test]
    fn any_fails_only_when_every_branch_fails() {
        let f = with(Condition::Any {
            children: vec![
                rating_at_least(4.5),
                rule("pages", FieldOperator::Gte, RuleValue::Number(100.0)),
            ],
        });
        let mut c = candidate(&["a"], Some(2.0));
        c.pages = Some(10);
        assert!(matches!(
            evaluate_outcome(&c, &f, &[], NOW, &plain),
            Outcome::Rejected(_)
        ));
    }

    /// Nested, which is the whole reason a tree replaced a flat list: this intent cannot be written
    /// as one list joined by a single connective.
    #[test]
    fn nesting_expresses_an_intent_a_flat_list_cannot() {
        // by this artist AND (well rated OR long) AND NOT already translated
        let f = with(Condition::All {
            children: vec![
                rule(
                    "tags",
                    FieldOperator::IncludesAll,
                    RuleValue::Text("artist:foo".into()),
                ),
                Condition::Any {
                    children: vec![
                        rating_at_least(4.5),
                        rule("pages", FieldOperator::Gte, RuleValue::Number(100.0)),
                    ],
                },
                Condition::Not {
                    child: Box::new(rule(
                        "tags",
                        FieldOperator::IncludesAll,
                        RuleValue::Text("language:translated".into()),
                    )),
                },
            ],
        });

        let mut good = candidate(&["artist:foo"], Some(5.0));
        good.pages = Some(10);
        assert_eq!(
            evaluate_outcome(&good, &f, &[], NOW, &plain),
            Outcome::Accepted
        );

        let mut translated = candidate(&["artist:foo", "language:translated"], Some(5.0));
        translated.pages = Some(10);
        assert!(matches!(
            evaluate_outcome(&translated, &f, &[], NOW, &plain),
            Outcome::Rejected(_)
        ));

        let mut wrong_artist = candidate(&["artist:bar"], Some(5.0));
        wrong_artist.pages = Some(10);
        assert!(matches!(
            evaluate_outcome(&wrong_artist, &f, &[], NOW, &plain),
            Outcome::Rejected(_)
        ));
    }

    #[test]
    fn not_inverts_its_child() {
        let f = with(Condition::Not {
            child: Box::new(rule(
                "tags",
                FieldOperator::IncludesAll,
                RuleValue::Text("language:spanish".into()),
            )),
        });
        assert_eq!(
            evaluate_outcome(
                &candidate(&["language:chinese"], None),
                &f,
                &[],
                NOW,
                &plain
            ),
            Outcome::Accepted
        );
        assert!(matches!(
            evaluate_outcome(
                &candidate(&["language:spanish"], None),
                &f,
                &[],
                NOW,
                &plain
            ),
            Outcome::Rejected(_)
        ));
    }

    /// An unfinished group must not match everything — that would silently widen the subscription.
    #[test]
    fn an_empty_any_is_unsatisfiable_not_vacuously_true() {
        let f = with(Condition::Any { children: vec![] });
        assert!(matches!(
            evaluate_outcome(&candidate(&["a"], Some(5.0)), &f, &[], NOW, &plain),
            Outcome::Rejected(_)
        ));
    }

    // ── Pending propagation (the user's chosen semantics) ──────────────────────────────────────

    /// Pending outranks a sibling that already holds: the pending branch may come to hold too, and
    /// acting now would settle a question still open.
    #[test]
    fn pending_wins_over_a_branch_that_already_holds() {
        let f = with(Condition::Any {
            children: vec![age(3.0 * 3600.0), rating_at_least(1.0)],
        });
        // Rating passes outright, but the age branch is merely premature.
        assert_eq!(
            evaluate_outcome(&dated("2023-11-14 22:00", Some(5.0)), &f, &[], NOW, &plain),
            Outcome::TooSoon
        );
    }

    #[test]
    fn pending_wins_over_a_failing_sibling_in_all() {
        let f = with(Condition::All {
            children: vec![age(3.0 * 3600.0), rating_at_least(4.0)],
        });
        // Rating fails AND the age is premature — the premature answer must win, since the rating
        // itself may still change.
        assert_eq!(
            evaluate_outcome(&dated("2023-11-14 22:00", Some(1.0)), &f, &[], NOW, &plain),
            Outcome::TooSoon
        );
    }

    /// Negating an unknown gives an unknown: excluding a work on the strength of a value that has not
    /// arrived would exclude it for the wrong reason.
    #[test]
    fn not_over_a_pending_child_stays_pending() {
        let f = with(Condition::Not {
            child: Box::new(rating_at_least(4.0)),
        });
        assert_eq!(
            evaluate_outcome(&candidate(&["a"], None), &f, &[], NOW, &plain),
            Outcome::TooSoon
        );
    }

    #[test]
    fn a_rule_over_an_absent_value_holds_rather_than_rejects() {
        let f = with(rating_at_least(4.0));
        assert_eq!(
            evaluate_outcome(&candidate(&["a"], None), &f, &[], NOW, &plain),
            Outcome::TooSoon
        );
    }

    #[test]
    fn a_rule_over_an_unknown_field_is_not_silently_satisfied() {
        let f = with(rule(
            "view_count",
            FieldOperator::Gte,
            RuleValue::Number(100.0),
        ));
        assert_ne!(
            evaluate_outcome(&candidate(&["a"], Some(5.0)), &f, &[], NOW, &plain),
            Outcome::Accepted
        );
    }

    // ── Operators ──────────────────────────────────────────────────────────────────────────────

    #[test]
    fn an_unparseable_posted_at_does_not_hold_a_work_back() {
        let f = with(age(3.0 * 3600.0));
        // Unparseable reads as absent; an absent date is not evidence that the work is new.
        assert_eq!(
            evaluate_outcome(&dated("sometime last tuesday", None), &f, &[], NOW, &plain),
            Outcome::TooSoon
        );
    }

    #[test]
    fn newer_than_bounds_the_other_direction() {
        let f = with(rule(
            "posted_at",
            FieldOperator::NewerThan,
            RuleValue::Number(24.0 * 3600.0),
        ));
        assert_eq!(
            evaluate_outcome(&dated("2023-11-14 12:00", None), &f, &[], NOW, &plain),
            Outcome::Accepted
        );
        assert!(matches!(
            evaluate_outcome(&dated("2023-11-01 12:00", None), &f, &[], NOW, &plain),
            Outcome::Rejected(_)
        ));
    }

    #[test]
    fn text_and_list_operators_ignore_case_and_surrounding_space() {
        let f = with(rule(
            "tags",
            FieldOperator::IncludesAll,
            RuleValue::Text("  ARTIST:Foo ".into()),
        ));
        assert_eq!(
            evaluate_outcome(&candidate(&["artist:foo"], None), &f, &[], NOW, &plain),
            Outcome::Accepted
        );
    }

    #[test]
    fn a_mismatched_operator_and_value_does_not_pass() {
        let f = with(rule(
            "rating",
            FieldOperator::IncludesAll,
            RuleValue::Text("x".into()),
        ));
        assert_eq!(
            evaluate_outcome(&candidate(&["a"], Some(5.0)), &f, &[], NOW, &plain),
            Outcome::Rejected(RejectionRule::FieldRule("rating".into()))
        );
    }

    // ── Tag namespaces as fields of their own ──────────────────────────────────────────────────

    /// The user's own case: "in Chinese or Japanese, **or** carrying no language tag at all." The
    /// last part is what the raw tag list cannot express — excluding specific tags cannot say "none
    /// of this kind", and a work tagged in no language carries nothing to exclude.
    #[test]
    fn language_can_be_matched_as_its_own_field_including_its_absence() {
        let f = with(Condition::Any {
            children: vec![
                rule(
                    "language",
                    FieldOperator::IncludesAll,
                    RuleValue::Text("chinese".into()),
                ),
                rule(
                    "language",
                    FieldOperator::IncludesAll,
                    RuleValue::Text("japanese".into()),
                ),
                rule("language", FieldOperator::IsEmpty, RuleValue::Bool(true)),
            ],
        });

        let chinese = candidate(&["language:chinese", "artist:foo"], None);
        assert_eq!(
            evaluate_outcome(&chinese, &f, &[], NOW, &plain),
            Outcome::Accepted
        );

        let japanese = candidate(&["language:japanese"], None);
        assert_eq!(
            evaluate_outcome(&japanese, &f, &[], NOW, &plain),
            Outcome::Accepted
        );

        // No language tag at all — the case the tag list alone cannot reach.
        let untagged = candidate(&["artist:foo", "female:big breasts"], None);
        assert_eq!(
            evaluate_outcome(&untagged, &f, &[], NOW, &plain),
            Outcome::Accepted
        );

        let spanish = candidate(&["language:spanish"], None);
        assert!(matches!(
            evaluate_outcome(&spanish, &f, &[], NOW, &plain),
            Outcome::Rejected(_)
        ));
    }

    /// An empty namespace is a real answer, not a missing one: treating it as unknown would hold the
    /// work for reconsideration on every check, forever.
    #[test]
    fn an_absent_namespace_is_empty_rather_than_unknown() {
        let f = with(rule(
            "language",
            FieldOperator::IsNotEmpty,
            RuleValue::Bool(true),
        ));
        assert!(matches!(
            evaluate_outcome(&candidate(&["artist:foo"], None), &f, &[], NOW, &plain),
            Outcome::Rejected(_)
        ));
    }

    /// Only the namespace's own value is compared, not the whole tag.
    #[test]
    fn a_namespace_field_holds_values_without_their_prefix() {
        let f = with(rule(
            "artist",
            FieldOperator::IncludesAll,
            RuleValue::Text("foo".into()),
        ));
        assert_eq!(
            evaluate_outcome(&candidate(&["artist:foo"], None), &f, &[], NOW, &plain),
            Outcome::Accepted
        );
    }

    /// A name that is not a known namespace stays unknown, so a rule over it is not silently
    /// satisfied by an empty list.
    #[test]
    fn an_unknown_namespace_is_still_unknown() {
        let f = with(rule(
            "not_a_namespace",
            FieldOperator::IsEmpty,
            RuleValue::Bool(true),
        ));
        assert_ne!(
            evaluate_outcome(&candidate(&["artist:foo"], None), &f, &[], NOW, &plain),
            Outcome::Accepted
        );
    }

    #[test]
    fn parses_the_listing_date_format_verified_against_the_real_source() {
        assert_eq!(parse_posted_at("2026-10-03 13:25"), Some(1791033900));
        assert!(parse_posted_at("2026-10-03T13:25:00+00:00").is_some());
        assert_eq!(parse_posted_at("1791033900"), Some(1791033900));
        assert_eq!(parse_posted_at("not a date"), None);
    }

    // ── Legacy filters fold into the tree ──────────────────────────────────────────────────────

    /// The superseded fixed filters must keep working, via one evaluation path rather than two.
    #[test]
    fn legacy_fixed_filters_become_tree_nodes() {
        let f = Filters {
            required_tags: vec!["artist:foo".into()],
            excluded_tags: vec!["language:spanish".into()],
            minimum_rating: Some(4.0),
            ..Filters::default()
        };
        assert_eq!(
            evaluate_outcome(&candidate(&["artist:foo"], Some(5.0)), &f, &[], NOW, &plain),
            Outcome::Accepted
        );
        assert!(matches!(
            evaluate_outcome(&candidate(&["artist:bar"], Some(5.0)), &f, &[], NOW, &plain),
            Outcome::Rejected(_)
        ));
        assert!(matches!(
            evaluate_outcome(
                &candidate(&["artist:foo", "language:spanish"], Some(5.0)),
                &f,
                &[],
                NOW,
                &plain,
            ),
            Outcome::Rejected(_)
        ));
    }

    /// Legacy and new conditions combine with AND, which is what the fixed filters always meant.
    #[test]
    fn legacy_filters_and_a_condition_both_apply() {
        let f = Filters {
            required_tags: vec!["artist:foo".into()],
            condition: Some(rule("pages", FieldOperator::Gte, RuleValue::Number(50.0))),
            ..Filters::default()
        };
        let mut c = candidate(&["artist:foo"], None);
        c.pages = Some(10);
        assert!(matches!(
            evaluate_outcome(&c, &f, &[], NOW, &plain),
            Outcome::Rejected(_)
        ));
        c.pages = Some(100);
        assert_eq!(
            evaluate_outcome(&c, &f, &[], NOW, &plain),
            Outcome::Accepted
        );
    }

    #[test]
    fn no_conditions_at_all_matches_everything() {
        assert_eq!(
            evaluate_outcome(&candidate(&[], None), &Filters::default(), &[], NOW, &plain),
            Outcome::Accepted
        );
    }

    /// The library's own categories are not a candidate field, so that rule keeps its own path.
    #[test]
    fn an_excluded_library_category_still_rejects() {
        let f = Filters {
            excluded_categories: vec!["Trash".into()],
            ..Filters::default()
        };
        assert_eq!(
            evaluate_outcome(
                &candidate(&["a"], None),
                &f,
                &["trash".to_string()],
                NOW,
                &plain
            ),
            Outcome::Rejected(RejectionRule::ExcludedCategory("Trash".into()))
        );
    }

    /// A tag rejection names the tag to edit, not just the field it lives in — this key is what the
    /// subscription's own history shows, and "field_rule:tags" says nothing about which tag did it.
    #[test]
    fn a_tag_rejection_names_the_tag() {
        let excluded = Filters {
            excluded_tags: vec!["language:spanish".into()],
            ..Filters::default()
        };
        assert_eq!(
            evaluate_outcome(
                &candidate(&["artist:foo", "language:spanish"], None),
                &excluded,
                &[],
                NOW,
                &plain
            ),
            Outcome::Rejected(RejectionRule::ExcludedTag("language:spanish".into()))
        );

        let required = Filters {
            required_tags: vec!["artist:foo".into()],
            ..Filters::default()
        };
        assert_eq!(
            evaluate_outcome(
                &candidate(&["artist:bar"], None),
                &required,
                &[],
                NOW,
                &plain
            ),
            Outcome::Rejected(RejectionRule::MissingRequiredTag("artist:foo".into()))
        );

        // Same shape written by hand, same reason: the distinction is the rule's shape, not whether
        // it came from a superseded fixed filter.
        let hand_written = with(rule(
            "tags",
            FieldOperator::IncludesNone,
            RuleValue::List(vec!["language:spanish".into()]),
        ));
        assert_eq!(
            evaluate_outcome(
                &candidate(&["language:spanish"], None),
                &hand_written,
                &[],
                NOW,
                &plain
            ),
            Outcome::Rejected(RejectionRule::ExcludedTag("language:spanish".into()))
        );
    }
}
