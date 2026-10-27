//! `/api/share/r/*`: what a share's recipient can do in a browser (017 US1, FR-012–FR-015,
//! contracts/http-api.md "Recipient"). The share credential is the only authority here: these
//! handlers never read or create a library session, and anything outside the share answers the
//! same 404 as having no credential at all (FR-013a).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::time::Instant;

use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use lanrurugi_core::ids::{ArchiveId, TankId};
use lanrurugi_share_proto::code::ShareRef;
use lanrurugi_share_proto::derive::access_hash;
use lanrurugi_share_proto::messages::{ManifestItem, ShareManifest};
use lanrurugi_storage::shares::{
    ClaimKind, ClaimOutcome, ClaimRefusal, RecipientTokenRepository, Share, ShareStatus,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::common::error;
use crate::share_attempts;
use crate::shares::{browse_manifest, hex, now_secs, recipient_address, repo};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/share/r/open", post(open))
        .route("/share/r/card", get(card))
        .route("/share/r/manifest", get(manifest))
        .route("/share/r/archives/{id}/thumbnail", get(thumbnail))
        .route("/share/r/archives/{id}/pages", get(pages))
        .route("/share/r/archives/{id}/page", get(page))
        .route("/share/r/archives/{id}/download", get(download_archive))
        .route("/share/r/tankoubons/{id}/download", get(download_tankoubon))
        .layer(axum::middleware::from_fn(no_referrer))
}

/// FR-013b: nothing on a share page may hand its address (and so the credential) to a third party.
async fn no_referrer(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

/// The one answer for an unknown credential, a revoked or expired share, a refused address and an
/// item outside the share: nothing about the share is revealed (FR-014).
fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "share", "not_found")
}

fn refused(status: StatusCode, code: &str) -> Response {
    error(status, "share", code)
}

fn rate_limited() -> Response {
    refused(StatusCode::TOO_MANY_REQUESTS, "rate_limited")
}

/// A recipient whose credential checked out for this request.
struct Recipient {
    token: String,
    share: Share,
    claim_id: Option<lanrurugi_core::ids::ClaimId>,
    address: IpAddr,
    manifest: ShareManifest,
}

impl Recipient {
    fn archive_ids(&self) -> HashSet<&str> {
        self.manifest
            .archives()
            .map(|a| a.archive_id.as_str())
            .collect()
    }

    fn has_tankoubon(&self, id: &str) -> bool {
        self.manifest
            .items
            .iter()
            .any(|i| matches!(i, ManifestItem::Tankoubon { id: t, .. } if t == id))
    }
}

/// Reads the token from `X-Share-Token` or `?rt=`.
fn token_from(headers: &HeaderMap, query: &HashMap<String, String>) -> Option<String> {
    headers
        .get("X-Share-Token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| query.get("rt").cloned())
}

/// Why a share can no longer be browsed: revoked or expired. A used-up share is **not** closed —
/// only downloads are limited, so browsing and reading continue (FR-015, US1-AS6); the response's
/// `status` field says `exhausted` and a download claim is refused instead.
fn closed_reason(share: &Share) -> Option<(StatusCode, &'static str)> {
    match share.status(now_secs()) {
        ShareStatus::Revoked => Some((StatusCode::NOT_FOUND, "revoked")),
        ShareStatus::Expired => Some((StatusCode::NOT_FOUND, "expired")),
        ShareStatus::Exhausted | ShareStatus::Active => None,
    }
}

/// Status, IP rules and first-N are re-checked on every request, so revocation, expiry, a used-up
/// limit and a changed allow-list apply immediately (SC-009).
async fn share_usable(state: &AppState, share: &Share, address: IpAddr) -> bool {
    closed_reason(share).is_none()
        && share.ip_allowed(address)
        && repo(state)
            .admit_address(share, address)
            .await
            .unwrap_or(false)
}

// Fails with a ready-made axum `Response`, this helper's error by design; boxing it would
// ripple through every `?` for no gain — the same call as `plugins::move_into_category`.
#[allow(clippy::result_large_err)]
async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
    peer: SocketAddr,
) -> Result<Recipient, Response> {
    let token = token_from(headers, query).ok_or_else(not_found)?;
    let record = RecipientTokenRepository::new(state.redis.config.clone())
        .resolve(&token)
        .await
        .ok()
        .flatten()
        .ok_or_else(not_found)?;
    let share = repo(state)
        .get(&record.share_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(not_found)?;
    let address = recipient_address(state, headers, peer).await;
    if !share_usable(state, &share, address).await {
        return Err(not_found());
    }
    let manifest = browse_manifest(state, &share).await;
    Ok(Recipient {
        token,
        claim_id: record.claim_id,
        share,
        address,
        manifest,
    })
}

fn manifest_json(recipient_share: &Share, manifest: &ShareManifest) -> Value {
    json!({
        "manifest": manifest,
        "permissions": recipient_share.permissions,
        "status": recipient_share.status(now_secs()),
    })
}

#[derive(Deserialize)]
struct OpenBody {
    code: String,
}

/// `POST /share/r/open`: exchanges a code for a recipient token (FR-011 limits failed tries).
async fn open(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<OpenBody>,
) -> Response {
    let address = recipient_address(&state, &headers, peer).await;
    let source = address.to_string();
    let limiter = share_attempts::shared();
    if !limiter.allowed(&source, Instant::now()) {
        return rate_limited();
    }
    let found = match ShareRef::parse(&body.code) {
        Ok(reference) => repo(&state)
            .by_code_hash(&hex(&access_hash(&reference.code)))
            .await
            .ok()
            .flatten(),
        Err(_) => None,
    };
    // A right code for a closed share says why, so the page can tell the recipient; it is not a
    // failed guess.
    if let Some((status, reason)) = found.as_ref().and_then(closed_reason) {
        return refused(status, reason);
    }
    let share = match found {
        Some(share) if share_usable(&state, &share, address).await => share,
        _ => {
            let penalty = limiter.record_failure(&source, Instant::now());
            limiter.apply(penalty).await;
            return not_found();
        }
    };
    let token = match RecipientTokenRepository::new(state.redis.config.clone())
        .issue(&share.id, address)
        .await
    {
        Ok(t) => t,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "share", e.to_string()),
    };
    let manifest = browse_manifest(&state, &share).await;
    let mut value = manifest_json(&share, &manifest);
    value["token"] = Value::String(token);
    Json(value).into_response()
}

#[derive(Deserialize)]
struct CardQuery {
    code: String,
}

/// `GET /share/r/card`: what an embedded card shows (FR-030–FR-031). Nothing at all unless the
/// share allows embedding; only "restricted" for a share with IP rules; only the state for a
/// revoked, expired or used-up one.
async fn card(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<CardQuery>,
) -> Response {
    let address = recipient_address(&state, &headers, peer).await;
    let source = address.to_string();
    let limiter = share_attempts::shared();
    if !limiter.allowed(&source, Instant::now()) {
        return rate_limited();
    }
    let found = match ShareRef::parse(&query.code) {
        Ok(reference) => repo(&state)
            .by_code_hash(&hex(&access_hash(&reference.code)))
            .await
            .ok()
            .flatten(),
        Err(_) => None,
    };
    let share = match found {
        Some(share) if share.embed => share,
        _ => {
            let penalty = limiter.record_failure(&source, Instant::now());
            limiter.apply(penalty).await;
            return not_found();
        }
    };
    let status = share.status(now_secs());
    let base = json!({ "status": status, "permissions": share.permissions });
    // A revoked or expired share's card shows its state and nothing of its content; a used-up one
    // keeps its content, since recipients can still read and import (FR-015).
    if matches!(
        share.status(now_secs()),
        ShareStatus::Revoked | ShareStatus::Expired
    ) {
        return Json(merge(base, json!({ "restricted": false }))).into_response();
    }
    if share.has_ip_restriction() {
        return Json(merge(base, json!({ "restricted": true }))).into_response();
    }
    let manifest = browse_manifest(&state, &share).await;
    // Each cover with its item's title, shown when the card enlarges a cover on hover. A share of
    // eight or more items fills two rows of four; the card uses seven of these plus its "+N" tile.
    let (covers, cover_titles): (Vec<String>, Vec<String>) = manifest
        .items
        .iter()
        .filter_map(|item| match item {
            ManifestItem::Archive(a) => Some((a.archive_id.clone(), a.title.clone())),
            ManifestItem::Tankoubon { name, members, .. } => members
                .first()
                .map(|m| (m.archive_id.clone(), name.clone())),
            ManifestItem::Unavailable { .. } => None,
        })
        .take(CARD_COVERS_LIMIT)
        .unzip();
    // Every item, for the card's "+N" list: its title, a cover and, for a tankoubon, its volumes.
    let items: Vec<Value> = manifest
        .items
        .iter()
        .take(CARD_ITEMS_LIMIT)
        .map(|item| match item {
            ManifestItem::Archive(a) => json!({ "title": a.title, "cover": a.archive_id }),
            ManifestItem::Tankoubon { name, members, .. } => json!({
                "title": name,
                "cover": members.first().map(|m| m.archive_id.clone()),
                "volumes": members.len(),
            }),
            ManifestItem::Unavailable { title, .. } => json!({ "title": title, "cover": null }),
        })
        .collect();
    // The card loads its covers through the recipient routes, like the share page does.
    let token = RecipientTokenRepository::new(state.redis.config.clone())
        .issue(&share.id, address)
        .await
        .ok();
    Json(merge(
        base,
        json!({
            "restricted": false,
            "title": manifest.title,
            "item_count": manifest.items.len(),
            "covers": covers,
            "cover_titles": cover_titles,
            "items": items,
            "token": token,
        }),
    ))
    .into_response()
}

/// Items a card lists behind its "+N" tile; a share rarely holds more, and the list scrolls.
const CARD_ITEMS_LIMIT: usize = 500;
/// Covers a card may lay out: a single row of four, or two rows when there are eight items or more.
const CARD_COVERS_LIMIT: usize = 8;

fn merge(mut base: Value, extra: Value) -> Value {
    if let (Value::Object(b), Value::Object(e)) = (&mut base, extra) {
        b.extend(e);
    }
    base
}

async fn manifest(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    match authorize(&state, &headers, &query, peer).await {
        Ok(r) => Json(manifest_json(&r.share, &r.manifest)).into_response(),
        Err(response) => response,
    }
}

/// Resolves a recipient whose share contains `archive_id`.
// Fails with a ready-made axum `Response`, this helper's error by design; boxing it would
// ripple through every `?` for no gain — the same call as `plugins::move_into_category`.
#[allow(clippy::result_large_err)]
async fn authorize_archive(
    state: &AppState,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
    peer: SocketAddr,
    archive_id: &str,
) -> Result<Recipient, Response> {
    let recipient = authorize(state, headers, query, peer).await?;
    if !recipient.archive_ids().contains(archive_id) {
        return Err(not_found());
    }
    Ok(recipient)
}

async fn thumbnail(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<ArchiveId>,
    Query(query): Query<HashMap<String, String>>,
    Query(params): Query<crate::archives::GetThumbnailParams>,
) -> Response {
    if let Err(r) = authorize_archive(&state, &headers, &query, peer, id.as_str()).await {
        return r;
    }
    crate::archives::get_archive_thumbnail(State(state), None, Path(id), Query(params), headers)
        .await
}

async fn pages(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<ArchiveId>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let recipient = match authorize_archive(&state, &headers, &query, peer, id.as_str()).await {
        Ok(r) => r,
        Err(r) => return r,
    };
    if !recipient.share.permissions.read {
        return refused(StatusCode::FORBIDDEN, "permission_denied");
    }
    let response = crate::archives::get_files(State(state), None, Path(id.clone())).await;
    if !response.status().is_success() {
        return response;
    }
    // Page URLs point at the library route; recipients reach the same pages through their own.
    let body = match axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return not_found(),
    };
    let library = format!("/api/archives/{id}/page?");
    let recipient_route = format!("/api/share/r/archives/{id}/page?");
    let text = String::from_utf8_lossy(&body).replace(&library, &recipient_route);
    ([(header::CONTENT_TYPE, "application/json")], text).into_response()
}

async fn page(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: axum::http::Method,
    headers: HeaderMap,
    Path(id): Path<ArchiveId>,
    Query(query): Query<HashMap<String, String>>,
    Query(params): Query<crate::archives::PageParams>,
) -> Response {
    let recipient = match authorize_archive(&state, &headers, &query, peer, id.as_str()).await {
        Ok(r) => r,
        Err(r) => return r,
    };
    if !recipient.share.permissions.read {
        return refused(StatusCode::FORBIDDEN, "permission_denied");
    }
    crate::archives::serve_page(
        &state,
        None,
        &id,
        params,
        &headers,
        method == axum::http::Method::HEAD,
    )
    .await
}

/// A paused or full own-share upload group answers 503 with `Retry-After` (FR-042, FR-043).
fn held(reason: crate::share_net::limits::Held) -> Response {
    let retry = match reason {
        crate::share_net::limits::Held::Paused { until: Some(until) } => {
            (until - crate::shares::now_secs()).clamp(60, 24 * 3600)
        }
        crate::share_net::limits::Held::Paused { until: None } => 300,
        crate::share_net::limits::Held::Busy => 30,
    };
    let mut response = refused(StatusCode::SERVICE_UNAVAILABLE, "paused");
    if let Ok(value) = HeaderValue::from_str(&retry.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// Paces a download body under the own-share rate cap.
fn throttled(response: Response, caps: crate::share_net::limits::Caps) -> Response {
    if caps.rate.is_none() {
        return response;
    }
    use futures_util::StreamExt;
    let (parts, body) = response.into_parts();
    let stream = body.into_data_stream().then(move |chunk| async move {
        if let Ok(bytes) = &chunk {
            crate::share_net::limits::throttle(
                crate::share_net::limits::Group::Own,
                caps,
                bytes.len() as u64,
            )
            .await;
        }
        chunk
    });
    Response::from_parts(parts, axum::body::Body::from_stream(stream))
}

/// Creates the token's claim on its first download, and reuses it afterwards (FR-015).
// Fails with a ready-made axum `Response`, this helper's error by design; boxing it would
// ripple through every `?` for no gain — the same call as `plugins::move_into_category`.
#[allow(clippy::result_large_err)]
async fn claim_download(
    state: &AppState,
    recipient: &Recipient,
) -> Result<crate::share_net::limits::Caps, Response> {
    if !recipient.share.permissions.download {
        return Err(refused(StatusCode::FORBIDDEN, "permission_denied"));
    }
    let caps = crate::share_net::limits::admit(
        state,
        crate::share_net::limits::Group::Own,
        &recipient.address.to_string(),
    )
    .await
    .map_err(held)?;
    let outcome = repo(state)
        .claim(
            &recipient.share,
            ClaimKind::BrowserDownload,
            recipient.address,
            recipient.claim_id.clone(),
            None,
            now_secs(),
        )
        .await
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, "share", e.to_string()))?;
    match outcome {
        ClaimOutcome::Created(claim_id) => {
            let _ = RecipientTokenRepository::new(state.redis.config.clone())
                .attach_claim(&recipient.token, &claim_id)
                .await;
            Ok(caps)
        }
        ClaimOutcome::Existing(_) => Ok(caps),
        ClaimOutcome::Refused(ClaimRefusal::Exhausted) => {
            Err(refused(StatusCode::CONFLICT, "exhausted"))
        }
        ClaimOutcome::Refused(ClaimRefusal::PermissionDenied) => {
            Err(refused(StatusCode::FORBIDDEN, "permission_denied"))
        }
        ClaimOutcome::Refused(_) => Err(not_found()),
    }
}

async fn download_archive(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<ArchiveId>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let recipient = match authorize_archive(&state, &headers, &query, peer, id.as_str()).await {
        Ok(r) => r,
        Err(r) => return r,
    };
    let caps = match claim_download(&state, &recipient).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    match state.repos.archives.get(&id).await {
        Ok(Some(archive)) => throttled(
            crate::archives::serve_archive_download(&archive).await,
            caps,
        ),
        _ => not_found(),
    }
}

async fn download_tankoubon(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<TankId>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let recipient = match authorize(&state, &headers, &query, peer).await {
        Ok(r) if r.has_tankoubon(id.as_str()) => r,
        Ok(_) => return not_found(),
        Err(r) => return r,
    };
    let caps = match claim_download(&state, &recipient).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    crate::tankoubons::tankoubon_zip_response(&state, &id)
        .await
        .map(|r| throttled(r, caps))
        .unwrap_or_else(not_found)
}
