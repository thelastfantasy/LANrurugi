//! Labels from the user's grouping decisions (`specs/016-tankoubon-grouping-harness` US4,
//! contracts/grouping-api.md): creating a suggestion and "don't suggest again" become pair
//! labels — the evaluation's ground truth.

use std::collections::HashSet;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use lanrurugi_core::ids::GroupSuggestionId;
use lanrurugi_storage::grouping_labels::{pairs, LabelSource, PairLabel, PairLabelKind};
use serde::Deserialize;

use super::labels_repo;
use crate::common::error;
use crate::AppState;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The pair labels one created suggestion yields: ticked–ticked positive, ticked–unticked
/// negative. `anchors` (an existing tankoubon's members) count as ticked.
pub(crate) fn created_labels(
    kept: &[String],
    unticked: &[String],
    anchors: &[String],
    suggestion_id: &GroupSuggestionId,
    at: u64,
) -> Vec<(String, String, PairLabel)> {
    let label = |kind| PairLabel {
        label: kind,
        source: LabelSource::Created,
        suggestion_id: suggestion_id.clone(),
        at,
    };
    let mut ticked: Vec<String> = kept.iter().chain(anchors).cloned().collect();
    ticked.sort();
    ticked.dedup();
    let unticked: HashSet<&String> = unticked.iter().filter(|u| !ticked.contains(u)).collect();
    let mut out: Vec<(String, String, PairLabel)> = pairs(&ticked)
        .map(|(a, b)| (a.to_string(), b.to_string(), label(PairLabelKind::Pos)))
        .collect();
    for t in &ticked {
        for u in &unticked {
            out.push((t.clone(), (*u).clone(), label(PairLabelKind::Neg)));
        }
    }
    out
}

/// Every pair of a dismissed suggestion is a negative label.
pub(crate) fn dismissed_labels(
    members: &[String],
    suggestion_id: &GroupSuggestionId,
    at: u64,
) -> Vec<(String, String, PairLabel)> {
    pairs(members)
        .map(|(a, b)| {
            (
                a.to_string(),
                b.to_string(),
                PairLabel {
                    label: PairLabelKind::Neg,
                    source: LabelSource::Dismissed,
                    suggestion_id: suggestion_id.clone(),
                    at,
                },
            )
        })
        .collect()
}

/// An existing tankoubon's members, so "add to existing" labels pair new members with them.
pub(super) async fn tankoubon_members(state: &AppState, tank_id: Option<&str>) -> Vec<String> {
    let Some(tank_id) = tank_id else {
        return Vec::new();
    };
    state
        .repos
        .groupings
        .get(&lanrurugi_core::ids::TankId(tank_id.to_string()))
        .await
        .ok()
        .flatten()
        .map(|t| t.archives.iter().map(|a| a.as_str().to_string()).collect())
        .unwrap_or_default()
}

/// Records negatives for a dismissed suggestion. Best effort: the dismissal itself already
/// succeeded and must not fail over its labels.
pub(super) async fn record_dismissal(
    state: &AppState,
    archive_ids: &[String],
    existing_tankoubon_id: Option<&str>,
) {
    let id = lanrurugi_storage::ignored_group_suggestions::suggestion_id(
        archive_ids,
        existing_tankoubon_id,
    );
    let mut members = archive_ids.to_vec();
    members.extend(tankoubon_members(state, existing_tankoubon_id).await);
    if let Err(e) = labels_repo(state)
        .record_pairs(dismissed_labels(&members, &id, now()))
        .await
    {
        tracing::warn!(error = %e, "grouping dismissal labels could not be saved");
    }
}

#[derive(Deserialize)]
pub(super) struct CreatedBody {
    archive_ids: Vec<String>,
    #[serde(default)]
    existing_tankoubon_id: Option<String>,
    kept: Vec<String>,
    #[serde(default)]
    unticked: Vec<String>,
    #[serde(default)]
    reviewer_rejected_kept: Vec<String>,
}

/// `POST /tankoubons/ai-group-suggestions/labels`.
pub(super) async fn record_created(
    State(state): State<AppState>,
    Json(body): Json<CreatedBody>,
) -> Response {
    let members: HashSet<&String> = body.archive_ids.iter().collect();
    if body
        .kept
        .iter()
        .chain(&body.unticked)
        .any(|id| !members.contains(id))
    {
        return error(
            StatusCode::BAD_REQUEST,
            "record_group_labels",
            "kept/unticked must be members of the suggestion",
        );
    }
    let id = lanrurugi_storage::ignored_group_suggestions::suggestion_id(
        &body.archive_ids,
        body.existing_tankoubon_id.as_deref(),
    );
    let at = now();
    let anchors = tankoubon_members(&state, body.existing_tankoubon_id.as_deref()).await;
    let repo = labels_repo(&state);
    if let Err(e) = repo
        .record_pairs(created_labels(
            &body.kept,
            &body.unticked,
            &anchors,
            &id,
            at,
        ))
        .await
    {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "record_group_labels",
            e.to_string(),
        );
    }
    for archive_id in body
        .reviewer_rejected_kept
        .iter()
        .filter(|a| body.kept.contains(a))
    {
        if let Err(e) = repo.record_disagreement(&id, archive_id, at).await {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "record_group_labels",
                e.to_string(),
            );
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

/// `GET /tankoubons/ai-group-suggestions/labels`.
pub(super) async fn label_counts(State(state): State<AppState>) -> Response {
    match labels_repo(&state).counts().await {
        Ok(counts) => Json(counts).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "group_label_counts",
            e.to_string(),
        ),
    }
}

/// `DELETE /tankoubons/ai-group-suggestions/labels` (FR-017).
pub(super) async fn delete_labels(State(state): State<AppState>) -> Response {
    match labels_repo(&state).delete_all().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_group_labels",
            e.to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn created_suggestion_yields_positive_and_negative_pairs() {
        let sid = GroupSuggestionId("s".into());
        let labels = created_labels(&ids(&["a", "b"]), &ids(&["c"]), &[], &sid, 1);
        let as_tuples: Vec<(&str, &str, PairLabelKind)> = labels
            .iter()
            .map(|(a, b, l)| (a.as_str(), b.as_str(), l.label))
            .collect();
        assert_eq!(
            as_tuples,
            [
                ("a", "b", PairLabelKind::Pos),
                ("a", "c", PairLabelKind::Neg),
                ("b", "c", PairLabelKind::Neg),
            ]
        );
    }

    #[test]
    fn existing_members_anchor_the_new_ones() {
        let sid = GroupSuggestionId("s".into());
        let labels = created_labels(&ids(&["n1"]), &ids(&["n2"]), &ids(&["e1"]), &sid, 1);
        assert!(labels
            .iter()
            .any(|(a, b, l)| a == "e1" && b == "n1" && l.label == PairLabelKind::Pos));
        assert!(labels
            .iter()
            .any(|(a, b, l)| a == "e1" && b == "n2" && l.label == PairLabelKind::Neg));
    }

    #[test]
    fn dismissed_suggestion_yields_only_negatives() {
        let sid = GroupSuggestionId("s".into());
        let labels = dismissed_labels(&ids(&["a", "b", "c"]), &sid, 1);
        assert_eq!(labels.len(), 3);
        assert!(labels.iter().all(|(_, _, l)| l.label == PairLabelKind::Neg));
    }
}
