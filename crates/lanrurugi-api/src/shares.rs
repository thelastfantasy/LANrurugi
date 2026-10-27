//! `/api/shares*`: creating and managing shares (specs/017-archive-sharing US1, US2,
//! contracts/http-api.md). Administrators only; the route policy refuses everyone else.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use deadpool_redis::redis::AsyncCommands;
use hkdf::Hkdf;
use lanrurugi_core::ids::ShareId;
use lanrurugi_share_proto::code::{ShareCode, ShareRef};
use lanrurugi_share_proto::derive::{access_hash, DerivedKeys};
use lanrurugi_share_proto::messages::{ArchiveEntry, ManifestItem, Permissions, ShareManifest};
use lanrurugi_share_proto::record::{open_bytes, seal_bytes};
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
use lanrurugi_storage::keys;
use lanrurugi_storage::shares::{
    Share, ShareItem, ShareRepository, ShareStatus, SHORT_CODE_MAX_LIFETIME_SECS,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::Sha256;

use crate::activity::record_manual;
use crate::auth_context::AuthContext;
use crate::common::{error, not_found};
use crate::share_settings::{self, ShareSettings};
use crate::AppState;

const CODE_ENC_AAD: &[u8] = b"lrrg-code-enc-v1";
/// Settings that the running endpoint is built from.
const ENDPOINT_FIELDS: &[&str] = &[
    "share_public_services",
    "share_relay_urls",
    "share_nodes",
    "share_udp_port",
    "share_portmap",
    "share_external_addrs",
    "share_self_relay_url",
    "share_node_relay",
    "share_node_relay_tls",
    "share_node_relay_port",
    "share_node_relay_cert",
    "share_node_relay_key",
    "share_node_relay_rate",
    "share_node_relay_allow",
];
/// Claim history shown in the management panel.
const CLAIM_HISTORY_LIMIT: isize = 500;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/shares", get(list_shares).post(create_share))
        .route(
            "/shares/settings",
            get(get_share_settings).put(put_share_settings),
        )
        .route("/shares/by-item/{item_id}", get(shares_by_item))
        .route("/shares/{id}", get(get_share).patch(update_share))
        .route(
            "/shares/{id}/items",
            post(add_items).put(reorder_items).delete(remove_items),
        )
        .route("/shares/{id}/revoke", post(revoke_share))
        .route("/shares/{id}/code", get(get_code))
}

pub(crate) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn repo(state: &AppState) -> ShareRepository {
    ShareRepository::new(state.redis.config.clone())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// This instance's long-term secret (also its iroh key), created on first use and included in
/// backups so the instance keeps its identity across restores.
pub(crate) async fn node_secret(state: &AppState) -> Result<[u8; 32], String> {
    let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
    let existing: Option<String> = conn
        .get(keys::SHARE_NODE_SECRET_KEY)
        .await
        .map_err(|e| e.to_string())?;
    let stored = match existing {
        Some(s) => s,
        None => {
            let fresh: [u8; 32] = rand::random();
            let _: bool = conn
                .set_nx(keys::SHARE_NODE_SECRET_KEY, hex(&fresh))
                .await
                .map_err(|e| e.to_string())?;
            conn.get(keys::SHARE_NODE_SECRET_KEY)
                .await
                .map_err(|e| e.to_string())?
        }
    };
    unhex(&stored)
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or_else(|| "the stored node secret is malformed".to_string())
}

async fn code_key(state: &AppState) -> Result<[u8; 32], String> {
    let secret = node_secret(state).await?;
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(None, &secret)
        .expand(b"code-enc", &mut key)
        .expect("32 bytes is a valid HKDF output length");
    Ok(key)
}

/// The share's code, re-derived from `code_enc` (FR-006).
pub(crate) async fn share_code(state: &AppState, share: &Share) -> Option<ShareCode> {
    let key = code_key(state).await.ok()?;
    share_code_with(&key, share)
}

/// `share_code` for a caller that already fetched the key (a list decrypting several shares).
fn share_code_with(key: &[u8; 32], share: &Share) -> Option<ShareCode> {
    let plain = open_bytes(key, &unhex(&share.code_enc)?, CODE_ENC_AAD)?;
    ShareCode::parse(std::str::from_utf8(&plain).ok()?).ok()
}

/// Parsed `share_trusted_proxies`, cached because every recipient request asks for the address.
static TRUSTED_PROXIES: Mutex<Option<(Instant, Vec<ipnet::IpNet>)>> = Mutex::new(None);
const TRUSTED_PROXIES_TTL: Duration = Duration::from_secs(60);

fn parse_trusted_proxies(text: &str) -> Vec<ipnet::IpNet> {
    text.split(',')
        .filter_map(|rule| rule.trim().parse().ok())
        .collect()
}

/// The peers whose `X-Forwarded-For` is believed, from `share_trusted_proxies` (loopback by
/// default). Never a private range by implication: any LAN client can write that header.
pub(crate) async fn trusted_proxies(state: &AppState) -> Vec<ipnet::IpNet> {
    if let Some((at, list)) = TRUSTED_PROXIES
        .lock()
        .expect("trusted proxies lock")
        .as_ref()
    {
        if at.elapsed() < TRUSTED_PROXIES_TTL {
            return list.clone();
        }
    }
    let settings = crate::share_settings::load(state).await;
    let list = parse_trusted_proxies(settings.text("share_trusted_proxies"));
    *TRUSTED_PROXIES.lock().expect("trusted proxies lock") = Some((Instant::now(), list.clone()));
    list
}

/// Drops the cache after the setting changes, so the next request reads the new list.
pub(crate) fn forget_trusted_proxies() {
    *TRUSTED_PROXIES.lock().expect("trusted proxies lock") = None;
}

/// The address a share's IP rules are checked against. A request that arrives from one of the
/// trusted proxies uses the nearest `X-Forwarded-For` hop — the one that proxy appended; every
/// other peer's own address stands, whatever header it carries.
pub(crate) async fn recipient_address(
    state: &AppState,
    headers: &HeaderMap,
    peer: SocketAddr,
) -> IpAddr {
    forwarded_address(headers, peer.ip(), &trusted_proxies(state).await)
}

/// The pure half of `recipient_address`, so the trust rule is unit-testable.
fn forwarded_address(headers: &HeaderMap, peer: IpAddr, trusted: &[ipnet::IpNet]) -> IpAddr {
    let peer_ip = lanrurugi_storage::shares::canonical_ip(peer);
    if !trusted.iter().any(|net| net.contains(&peer_ip)) {
        return peer_ip;
    }
    headers
        .get_all("X-Forwarded-For")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|hop| hop.trim().parse::<IpAddr>().ok())
        .next_back()
        .map(lanrurugi_storage::shares::canonical_ip)
        .unwrap_or(peer_ip)
}

// ---------------------------------------------------------------------------------------------
// Manifest

fn archive_entry(a: &lanrurugi_core::entities::Archive) -> ArchiveEntry {
    let filename = std::path::Path::new(&a.file)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    ArchiveEntry {
        archive_id: a.id.to_string(),
        title: if a.title.trim().is_empty() {
            a.name.clone()
        } else {
            a.title.clone()
        },
        tags: a.tags.clone(),
        summary: a.summary.clone(),
        pagecount: a.pagecount,
        filename,
        size: a.arcsize,
        blake3: None,
        categories: Vec::new(),
        toc: a
            .toc
            .iter()
            .map(|c| lanrurugi_share_proto::messages::Chapter {
                page: c.page,
                name: c.name.clone(),
            })
            .collect(),
    }
}

/// The share's items as they are now in the library (browse form: no fingerprints). Tankoubons
/// list their current members in order; deleted items become `Unavailable` (US2-4).
pub(crate) async fn browse_manifest(state: &AppState, share: &Share) -> ShareManifest {
    let mut items = Vec::with_capacity(share.items.len());
    for item in &share.items {
        match item {
            ShareItem::Archive { id } => match state.repos.archives.get(id).await {
                Ok(Some(a)) => items.push(ManifestItem::Archive(archive_entry(&a))),
                _ => items.push(ManifestItem::Unavailable { title: None }),
            },
            ShareItem::Tankoubon { id } => match state.repos.groupings.get(id).await {
                Ok(Some(tank)) => {
                    let records = futures_util::future::join_all(
                        tank.archives.iter().map(|a| state.repos.archives.get(a)),
                    )
                    .await;
                    let members = records
                        .into_iter()
                        .filter_map(|r| r.ok().flatten())
                        .map(|a| archive_entry(&a))
                        .collect();
                    items.push(ManifestItem::Tankoubon {
                        id: id.to_string(),
                        name: tank.name,
                        tags: tank.tags,
                        summary: tank.summary,
                        members,
                    });
                }
                _ => items.push(ManifestItem::Unavailable { title: None }),
            },
        }
    }
    ShareManifest {
        share_id: share.id.to_string(),
        title: share.title.clone().unwrap_or_default(),
        permissions: share.permissions,
        items,
    }
}

/// Default title: the first item's own name.
async fn first_item_title(state: &AppState, items: &[ShareItem]) -> Option<String> {
    match items.first()? {
        ShareItem::Archive { id } => state
            .repos
            .archives
            .get(id)
            .await
            .ok()
            .flatten()
            .map(|a| archive_entry(&a).title),
        ShareItem::Tankoubon { id } => state
            .repos
            .groupings
            .get(id)
            .await
            .ok()
            .flatten()
            .map(|t| t.name),
    }
}

async fn item_exists(state: &AppState, item: &ShareItem) -> bool {
    match item {
        ShareItem::Archive { id } => matches!(state.repos.archives.get(id).await, Ok(Some(_))),
        ShareItem::Tankoubon { id } => matches!(state.repos.groupings.get(id).await, Ok(Some(_))),
    }
}

/// Queues the archives of `items` for background fingerprinting (research §13).
async fn enqueue_prepare(state: &AppState, items: &[ShareItem]) {
    let mut archive_ids: Vec<String> = Vec::new();
    for item in items {
        match item {
            ShareItem::Archive { id } => archive_ids.push(id.to_string()),
            ShareItem::Tankoubon { id } => {
                if let Ok(Some(tank)) = state.repos.groupings.get(id).await {
                    archive_ids.extend(tank.archives.iter().map(|a| a.to_string()));
                }
            }
        }
    }
    if archive_ids.is_empty() {
        return;
    }
    let now = now_secs();
    if let Ok(mut conn) = state.redis.config.get().await {
        let members: Vec<(i64, String)> = archive_ids.into_iter().map(|id| (now, id)).collect();
        let _: Result<(), _> = deadpool_redis::redis::cmd("ZADD")
            .arg(keys::SHARE_PREPARE_KEY)
            .arg("NX")
            .arg(&members)
            .query_async(&mut conn)
            .await;
    }
}

// ---------------------------------------------------------------------------------------------
// Response shapes

#[derive(Serialize)]
pub(crate) struct ShareSummary {
    id: String,
    title: String,
    status: ShareStatus,
    permissions: Permissions,
    downloads_used: u32,
    download_limit: Option<u32>,
    item_count: usize,
    created_at: i64,
    expires_at: Option<i64>,
    embed: bool,
    allow_helpers: bool,
    short_code: bool,
    restricted: bool,
    /// The share's code, so the list can show and copy it without one request per row. Named
    /// `share_code` because `ShareDetail`'s own `code` is the richer `ShareCodeInfo` object.
    #[serde(skip_serializing_if = "Option::is_none")]
    share_code: Option<String>,
}

fn summary(share: &Share, now: i64) -> ShareSummary {
    ShareSummary {
        id: share.id.to_string(),
        title: share.title.clone().unwrap_or_default(),
        status: share.status(now),
        permissions: share.permissions,
        downloads_used: share.downloads_used,
        download_limit: share.download_limit,
        item_count: share.items.len(),
        created_at: share.created_at,
        expires_at: share.expires_at,
        embed: share.embed,
        allow_helpers: share.allow_helpers,
        short_code: share.short_code,
        restricted: share.has_ip_restriction(),
        share_code: None,
    }
}

#[derive(Serialize)]
struct ResolvedItem {
    kind: &'static str,
    id: String,
    title: Option<String>,
    available: bool,
    member_count: Option<usize>,
}

async fn detail_json(state: &AppState, share: &Share) -> Value {
    let now = now_secs();
    let manifest = browse_manifest(state, share).await;
    let items: Vec<ResolvedItem> = share
        .items
        .iter()
        .zip(&manifest.items)
        .map(|(item, resolved)| {
            let kind = match item {
                ShareItem::Archive { .. } => "archive",
                ShareItem::Tankoubon { .. } => "tankoubon",
            };
            let (title, member_count) = match resolved {
                ManifestItem::Archive(a) => (Some(a.title.clone()), None),
                ManifestItem::Tankoubon { name, members, .. } => {
                    (Some(name.clone()), Some(members.len()))
                }
                ManifestItem::Unavailable { title } => (title.clone(), None),
            };
            ResolvedItem {
                kind,
                id: item.id_str().to_string(),
                available: !matches!(resolved, ManifestItem::Unavailable { .. }),
                title,
                member_count,
            }
        })
        .collect();
    let claims = repo(state)
        .claim_history(&share.id, CLAIM_HISTORY_LIMIT)
        .await
        .unwrap_or_default();
    // Every helper that ever registered, with when it was last heard from (T093).
    let helpers: Vec<Value> = repo(state)
        .helpers(&share.id, 0)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|h| json!({ "endpoint_id": h.endpoint_id, "last_seen": h.last_seen }))
        .collect();
    let mut value = serde_json::to_value(summary(share, now)).expect("summary serialises");
    let extra = json!({
        "items": items,
        "ip_allow": share.ip_allow,
        "ip_first_n": share.ip_first_n,
        "claims": claims,
        "helpers": helpers,
    });
    merge(&mut value, extra);
    value
}

fn merge(target: &mut Value, extra: Value) {
    if let (Value::Object(t), Value::Object(e)) = (target, extra) {
        t.extend(e);
    }
}

/// Code, text form, link and embed snippets for the share dialog (FR-009, FR-010a).
async fn code_json(state: &AppState, share: &Share, settings: &ShareSettings) -> Option<Value> {
    let code = share_code(state, share).await?;
    let mut reference = ShareRef::new(code.clone());
    // The code carries its own routes (addresses, relays, nodes), so it works with no rendezvous
    // or relay reachable, as long as one of them still leads to this instance.
    if let Some(net) = crate::share_net::handle().await {
        reference.via = crate::share_net::publish::via_hint(&net, settings).map(|mut via| {
            // An own relay (a tunnel, or the relay role) reaches importers whose UDP cannot
            // reach any address.
            if let Some(own) = own_relay_hint(settings).filter(|r| !via.relays.contains(r)) {
                via.relays.push(own);
            }
            via
        });
    }
    let nodes = crate::share_net::nodes::configured_nodes(settings);
    reference.nodes = nodes
        .iter()
        .map(|n| format!("{}@{}", hex(&n.endpoint_id), n.addr))
        .collect();
    let text_form = reference.text_form();
    let link = settings
        .public_url()
        .map(|url| format!("{url}/s/{}", code.display()));
    let embed = match (&link, share.embed) {
        (Some(_), true) => {
            let url = settings.public_url().expect("link implies a public URL");
            let code = code.display();
            Some(json!({
                "iframe": format!(
                    "<iframe src=\"{url}/embed/s/{code}\" width=\"360\" height=\"420\" style=\"border:0;max-width:100%\" loading=\"lazy\" referrerpolicy=\"no-referrer\"></iframe>"
                ),
                "element": format!(
                    "<script src=\"{url}/embed/share.js\" defer></script>\n<lrrg-share code=\"{code}\" src=\"{url}\"></lrrg-share>"
                ),
            }))
        }
        _ => None,
    };
    let needs_direct_route = !settings.flag("share_public_services") && nodes.is_empty();
    Some(json!({
        "code": code.display(),
        "text_form": text_form,
        "needs_direct_route": needs_direct_route,
        "link": link,
        "embed": embed,
    }))
}

// ---------------------------------------------------------------------------------------------
// Request shapes and editing

#[derive(Deserialize)]
struct NewShare {
    items: Vec<ShareItem>,
    #[serde(default)]
    short_code: bool,
    /// Every other field (title, permissions, limits, IP rules, expiry, embed, helpers) is applied
    /// through the same path as an edit, over the instance defaults.
    #[serde(flatten)]
    fields: Map<String, Value>,
}

/// Applies an edit body to a share. `null` clears an optional field.
fn apply_update(share: &mut Share, body: &Map<String, Value>, now: i64) -> Result<(), String> {
    for (key, value) in body {
        match key.as_str() {
            "title" => {
                share.title = value
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            }
            "permissions" => {
                share.permissions = serde_json::from_value(value.clone())
                    .map_err(|_| "\"permissions\" must be {import, read, download}".to_string())?
            }
            "download_limit" => share.download_limit = optional_count(key, value)?,
            "ip_first_n" => share.ip_first_n = optional_count(key, value)?,
            "ip_allow" => {
                share.ip_allow = match value {
                    Value::Null => Vec::new(),
                    Value::String(s) => share_settings::split_list(s),
                    other => serde_json::from_value(other.clone())
                        .map_err(|_| "\"ip_allow\" must be a list of addresses".to_string())?,
                }
            }
            "expires_at" => {
                share.expires_at = match value {
                    Value::Null => None,
                    v => Some(v.as_i64().ok_or("\"expires_at\" must be unix seconds")?),
                }
            }
            "embed" => share.embed = value.as_bool().ok_or("\"embed\" must be a boolean")?,
            "allow_helpers" => {
                let on = value
                    .as_bool()
                    .ok_or("\"allow_helpers\" must be a boolean")?;
                if share.allow_helpers && !on {
                    share.helpers_disabled_at = Some(now);
                }
                share.allow_helpers = on;
            }
            other => return Err(format!("Unknown share field: \"{other}\".")),
        }
    }
    share.validate().map_err(|e| e.to_string())
}

fn optional_count(key: &str, value: &Value) -> Result<Option<u32>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) if s.trim().is_empty() => Ok(None),
        v => v
            .as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("\"{key}\" must be a whole number")),
    }
}

fn defaults_from(settings: &ShareSettings, now: i64) -> Map<String, Value> {
    let mut defaults = Map::new();
    defaults.insert(
        "permissions".into(),
        serde_json::to_value(settings.default_permissions()).expect("permissions serialise"),
    );
    if let Some(n) = settings.number("share_default_download_limit") {
        defaults.insert("download_limit".into(), json!(n));
    }
    let ip_allow = settings.list("share_default_ip_allow");
    if !ip_allow.is_empty() {
        defaults.insert("ip_allow".into(), json!(ip_allow));
    }
    if let Some(n) = settings.number("share_default_ip_first_n") {
        defaults.insert("ip_first_n".into(), json!(n));
    }
    if let Some(days) = settings.number("share_default_expiry_days") {
        defaults.insert("expires_at".into(), json!(now + days as i64 * 86_400));
    }
    defaults.insert("embed".into(), json!(settings.flag("share_default_embed")));
    defaults.insert(
        "allow_helpers".into(),
        json!(settings.flag("share_default_allow_helpers")),
    );
    defaults
}

fn bad_request(operation: &str, message: impl Into<String>) -> Response {
    error(StatusCode::BAD_REQUEST, operation, message)
}

fn internal(operation: &str, message: impl ToString) -> Response {
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        operation,
        message.to_string(),
    )
}

fn target(share: &Share) -> ActivityTarget {
    ActivityTarget {
        id: Some(share.id.to_string()),
        label: share.title.clone(),
        kind: Some("share".to_string()),
    }
}

// Fails with a ready-made axum `Response`, this helper's error by design; boxing it would
// ripple through every `?` for no gain — the same call as `plugins::move_into_category`.
#[allow(clippy::result_large_err)]
async fn load_share(state: &AppState, id: &str, operation: &str) -> Result<Share, Response> {
    match repo(state).get(&ShareId::from(id)).await {
        Ok(Some(share)) => Ok(share),
        Ok(None) => Err(not_found(operation, "No such share.")),
        Err(e) => Err(internal(operation, e)),
    }
}

// ---------------------------------------------------------------------------------------------
// Handlers

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
}

async fn list_shares(State(state): State<AppState>, Query(q): Query<ListQuery>) -> Response {
    let now = now_secs();
    match repo(&state).list().await {
        Ok(shares) => {
            let only_active = q.status.as_deref() == Some("active");
            let key = code_key(&state).await.ok();
            let mut list: Vec<ShareSummary> = shares
                .iter()
                .filter(|s| !only_active || s.status(now) == ShareStatus::Active)
                .map(|s| summary(s, now))
                .collect();
            // One key fetch for the whole page; each code is a small decrypt.
            for item in &mut list {
                let Some(share) = shares.iter().find(|s| s.id.as_str() == item.id) else {
                    continue;
                };
                item.share_code = key
                    .as_ref()
                    .and_then(|k| share_code_with(k, share))
                    .map(|c| c.display());
            }
            Json(list).into_response()
        }
        Err(e) => internal("list_shares", e),
    }
}

async fn create_share(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Json(body): Json<NewShare>,
) -> Response {
    const OP: &str = "create_share";
    let now = now_secs();
    if body.items.is_empty() {
        return bad_request(OP, "A share needs at least one item.");
    }
    for item in &body.items {
        if !item_exists(&state, item).await {
            return bad_request(OP, format!("No such item: {}", item.id_str()));
        }
    }
    let settings = share_settings::load(&state).await;
    let mut fields = defaults_from(&settings, now);
    fields.extend(body.fields);
    if body.short_code
        && !fields
            .get("expires_at")
            .and_then(Value::as_i64)
            .is_some_and(|t| t <= now + SHORT_CODE_MAX_LIFETIME_SECS)
    {
        return bad_request(OP, "A short code needs an expiry within 7 days.");
    }

    let key = match code_key(&state).await {
        Ok(k) => k,
        Err(e) => return internal(OP, e),
    };
    let repo = repo(&state);
    // A collision in a 50-bit space is astronomically unlikely, but costs nothing to rule out.
    let mut code = ShareCode::generate(body.short_code);
    while matches!(
        repo.by_code_hash(&hex(&access_hash(&code))).await,
        Ok(Some(_))
    ) {
        code = ShareCode::generate(body.short_code);
    }
    let derived = {
        let code = code.clone();
        match tokio::task::spawn_blocking(move || DerivedKeys::derive(&code)).await {
            Ok(d) => d,
            Err(e) => return internal(OP, e),
        }
    };
    let mut share = Share {
        id: ShareId::generate(),
        title: None,
        code_hash: hex(&access_hash(&code)),
        short_code: body.short_code,
        code_enc: hex(&seal_bytes(&key, code.as_str().as_bytes(), CODE_ENC_AAD)),
        lookup_pubkey: hex(&derived.lookup_public()),
        record_key: hex(&derived.record_key),
        items: dedup(body.items),
        permissions: Permissions::ALL,
        download_limit: None,
        downloads_used: 0,
        ip_allow: Vec::new(),
        ip_first_n: None,
        expires_at: None,
        embed: false,
        allow_helpers: false,
        helpers_disabled_at: None,
        revoked_at: None,
        created_at: now,
        created_by: auth
            .as_ref()
            .map(|Extension(a)| a.trace_label())
            .unwrap_or_default(),
    };
    if let Err(message) = apply_update(&mut share, &fields, now) {
        return bad_request(OP, message);
    }
    if share.title.is_none() {
        share.title = first_item_title(&state, &share.items).await;
    }
    if let Err(e) = repo.create(&share).await {
        return internal(OP, e);
    }
    enqueue_prepare(&state, &share.items).await;
    publish_in_background(&state, &share);
    record_manual(
        &state,
        auth.as_ref().map(|Extension(a)| a),
        action_types::SHARE_CREATE,
        target(&share),
        Outcome::Success,
        None,
        Some(json!({ "items": share.items.len(), "permissions": share.permissions })),
    )
    .await;

    let mut value = detail_json(&state, &share).await;
    if let Some(code) = code_json(&state, &share, &settings).await {
        merge(&mut value, json!({ "code": code }));
    }
    (StatusCode::CREATED, Json(value)).into_response()
}

/// Publishes a new share's knock record so other instances can resolve its code (research §2).
fn publish_in_background(state: &AppState, share: &Share) {
    if !share.permissions.import {
        return;
    }
    let state = state.clone();
    let share = share.clone();
    tokio::spawn(async move {
        if let Some(net) = crate::share_net::handle().await {
            crate::share_net::publish::publish_share(&state, &net, &share).await;
        }
    });
}

fn dedup(items: Vec<ShareItem>) -> Vec<ShareItem> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|i| seen.insert(i.clone()))
        .collect()
}

async fn get_share(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match load_share(&state, &id, "get_share").await {
        Ok(share) => Json(detail_json(&state, &share).await).into_response(),
        Err(r) => r,
    }
}

async fn update_share(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
    Json(body): Json<Map<String, Value>>,
) -> Response {
    const OP: &str = "update_share";
    let mut share = match load_share(&state, &id, OP).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if share.revoked_at.is_some() {
        return error(
            StatusCode::CONFLICT,
            OP,
            "A revoked share cannot be changed.",
        );
    }
    let before = serde_json::to_value(summary(&share, now_secs())).ok();
    let previous_items = share.items.clone();
    if let Err(message) = apply_update(&mut share, &body, now_secs()) {
        return bad_request(OP, message);
    }
    if let Err(e) = repo(&state).save(&share, &previous_items).await {
        return internal(OP, e);
    }
    record_manual(
        &state,
        auth.as_ref().map(|Extension(a)| a),
        action_types::SHARE_UPDATE,
        target(&share),
        Outcome::Success,
        before,
        serde_json::to_value(summary(&share, now_secs())).ok(),
    )
    .await;
    Json(detail_json(&state, &share).await).into_response()
}

#[derive(Deserialize)]
struct ItemsBody {
    items: Vec<ShareItem>,
    #[serde(default)]
    update: Option<Map<String, Value>>,
}

async fn add_items(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
    Json(body): Json<ItemsBody>,
) -> Response {
    change_items(state, auth, id, body, true).await
}

async fn remove_items(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
    Json(body): Json<ItemsBody>,
) -> Response {
    change_items(state, auth, id, body, false).await
}

async fn change_items(
    state: AppState,
    auth: Option<Extension<AuthContext>>,
    id: String,
    body: ItemsBody,
    adding: bool,
) -> Response {
    const OP: &str = "share_items";
    let mut share = match load_share(&state, &id, OP).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if share.revoked_at.is_some() {
        return error(
            StatusCode::CONFLICT,
            OP,
            "A revoked share cannot be changed.",
        );
    }
    let previous_items = share.items.clone();
    if adding {
        for item in &body.items {
            if !item_exists(&state, item).await {
                return bad_request(OP, format!("No such item: {}", item.id_str()));
            }
        }
        let mut items = share.items.clone();
        items.extend(body.items.iter().cloned());
        share.items = dedup(items);
    } else {
        share.items.retain(|i| !body.items.contains(i));
    }
    if let Some(update) = &body.update {
        if let Err(message) = apply_update(&mut share, update, now_secs()) {
            return bad_request(OP, message);
        }
    } else if let Err(e) = share.validate() {
        return bad_request(OP, e.to_string());
    }
    if let Err(e) = repo(&state).save(&share, &previous_items).await {
        return internal(OP, e);
    }
    if adding {
        enqueue_prepare(&state, &body.items).await;
    }
    record_manual(
        &state,
        auth.as_ref().map(|Extension(a)| a),
        action_types::SHARE_ITEMS,
        target(&share),
        Outcome::Success,
        None,
        Some(json!({ "added": adding, "items": body.items })),
    )
    .await;
    Json(detail_json(&state, &share).await).into_response()
}

#[derive(Deserialize)]
struct ReorderBody {
    items: Vec<ShareItem>,
}

/// Replaces the share's item order (FR-002a): the recipient page, the import list and the
/// tankoubon member order shown there all follow `items`. The code, its claims and the limits are
/// untouched — this writes the list, nothing else. The set must be the share's current one, so a
/// stale client can reorder but never drop an item by accident.
async fn reorder_items(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
    Json(body): Json<ReorderBody>,
) -> Response {
    const OP: &str = "share_reorder";
    let mut share = match load_share(&state, &id, OP).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if share.revoked_at.is_some() {
        return error(
            StatusCode::CONFLICT,
            OP,
            "A revoked share cannot be changed.",
        );
    }
    let previous_items = share.items.clone();
    let ordered = dedup(body.items);
    let wanted: HashSet<&ShareItem> = ordered.iter().collect();
    let current: HashSet<&ShareItem> = previous_items.iter().collect();
    if wanted != current {
        return bad_request(
            OP,
            "Reordering must list exactly the share's current items.",
        );
    }
    share.items = ordered;
    if let Err(e) = share.validate() {
        return bad_request(OP, e.to_string());
    }
    if let Err(e) = repo(&state).save(&share, &previous_items).await {
        return internal(OP, e);
    }
    record_manual(
        &state,
        auth.as_ref().map(|Extension(a)| a),
        action_types::SHARE_ITEMS,
        target(&share),
        Outcome::Success,
        None,
        Some(json!({ "reordered": share.items })),
    )
    .await;
    Json(detail_json(&state, &share).await).into_response()
}

async fn revoke_share(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
) -> Response {
    const OP: &str = "revoke_share";
    let mut share = match load_share(&state, &id, OP).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if share.revoked_at.is_none() {
        let now = now_secs();
        if let Err(e) = repo(&state).revoke(&share.id, now).await {
            return internal(OP, e);
        }
        share.revoked_at = Some(now);
        record_manual(
            &state,
            auth.as_ref().map(|Extension(a)| a),
            action_types::SHARE_REVOKE,
            target(&share),
            Outcome::Success,
            None,
            None,
        )
        .await;
    }
    Json(detail_json(&state, &share).await).into_response()
}

async fn get_code(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    const OP: &str = "share_code";
    let share = match load_share(&state, &id, OP).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let settings = share_settings::load(&state).await;
    match code_json(&state, &share, &settings).await {
        Some(v) => Json(v).into_response(),
        None => internal(OP, "The share code could not be recovered."),
    }
}

async fn shares_by_item(State(state): State<AppState>, Path(item_id): Path<String>) -> Response {
    let repo = repo(&state);
    let now = now_secs();
    let ids = match repo.shares_referencing(&item_id).await {
        Ok(ids) => ids,
        Err(e) => return internal("shares_by_item", e),
    };
    let mut list = Vec::with_capacity(ids.len());
    for id in ids {
        if let Ok(Some(share)) = repo.get(&id).await {
            list.push(summary(&share, now));
        }
    }
    list.sort_by_key(|s| std::cmp::Reverse(s.created_at));
    Json(list).into_response()
}

async fn get_share_settings(State(state): State<AppState>) -> Response {
    Json(settings_with_status(&state).await).into_response()
}

/// The settings plus what is in effect now: upload mode (FR-042), this endpoint's routes (T054)
/// and the outside services in use (FR-049).
async fn settings_with_status(state: &AppState) -> Value {
    let settings = share_settings::load(state).await;
    let mut value = Value::Object(settings.to_json());
    let (mode, _, _) = crate::share_net::limits::current(state).await;
    merge(
        &mut value,
        serde_json::to_value(mode).unwrap_or(Value::Null),
    );
    let endpoint = match crate::share_net::handle().await {
        Some(net) => {
            let addr = net.endpoint().addr();
            json!({
                "running": true,
                "endpoint_id": crate::share_net::endpoint_hex(&net.id()),
                "relays": addr.relay_urls().map(|u| u.to_string()).collect::<Vec<_>>(),
                "direct_addresses": addr.ip_addrs().map(|a| a.to_string()).collect::<Vec<_>>(),
                "proxy": crate::share_net::followed_proxy(),
                "node_string": crate::share_net::nodes::node_string(&net.id(), &settings),
            })
        }
        None => json!({ "running": false }),
    };
    // Relays: the endpoint keeps a connection to its home relay only, so that one is known
    // reachable and the others are unknown rather than failed (FR-049).
    let connected: Vec<String> = match crate::share_net::handle().await {
        Some(net) => net
            .endpoint()
            .addr()
            .relay_urls()
            .map(|u| u.to_string())
            .collect(),
        None => Vec::new(),
    };
    let mut services = crate::share_net::status::snapshot();
    services.extend(
        crate::share_net::relay_urls(&settings)
            .into_iter()
            .map(|url| {
                let url = url.to_string();
                crate::share_net::status::ServiceStatus {
                    reachable: connected.contains(&url).then_some(true),
                    kind: "relay",
                    last_ok_at: None,
                    url,
                }
            }),
    );
    merge(
        &mut value,
        json!({
            "endpoint": endpoint,
            "services": services,
            "helpers_supported": true,
            // What an admin compares between two instances before an import works: the engine
            // object (protocol epoch + transport), not a single number.
            "share_engine": lanrurugi_share_proto::messages::ShareEngine::current(),
        }),
    );
    value
}

async fn put_share_settings(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Json(body): Json<Map<String, Value>>,
) -> Response {
    const OP: &str = "put_share_settings";
    match share_settings::save(&state, &body).await {
        Ok(settings) => {
            if body.contains_key("share_trusted_proxies") {
                forget_trusted_proxies();
            }
            let changed: Vec<&String> = body.keys().collect();
            // Connectivity changes rebuild the endpoint, then republish every record with the
            // new relays.
            if changed
                .iter()
                .any(|k| ENDPOINT_FIELDS.contains(&k.as_str()))
            {
                let state = state.clone();
                tokio::spawn(async move {
                    match crate::share_net::start(&state).await {
                        Ok(net) => crate::share_net::publish::publish_all(&state, &net).await,
                        Err(e) => tracing::warn!(error = %e, "sharing endpoint could not restart"),
                    }
                });
            }
            record_manual(
                &state,
                auth.as_ref().map(|Extension(a)| a),
                action_types::SHARE_SETTINGS_UPDATE,
                ActivityTarget {
                    id: None,
                    label: None,
                    kind: Some("settings".to_string()),
                },
                Outcome::Success,
                None,
                Some(json!({ "fields": changed })),
            )
            .await;
            let _ = settings;
            Json(settings_with_status(&state).await).into_response()
        }
        Err(message) => bad_request(OP, message),
    }
}

/// This instance's own relay for a `via` hint: the tunnel address, or the relay role's.
fn own_relay_hint(settings: &ShareSettings) -> Option<String> {
    let explicit = settings.text("share_self_relay_url");
    let own = if explicit.is_empty() {
        settings
            .flag("share_node_relay")
            .then(|| crate::share_net::nodes::own_relay_url(settings))
            .flatten()?
    } else {
        explicit.to_string()
    };
    share_settings::canonical_https_url(&own)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_core::ids::TankId;

    fn share() -> Share {
        Share {
            id: ShareId::generate(),
            title: None,
            code_hash: String::new(),
            short_code: false,
            code_enc: String::new(),
            lookup_pubkey: String::new(),
            record_key: String::new(),
            items: vec![ShareItem::Tankoubon {
                id: TankId::from("TANK_1"),
            }],
            permissions: Permissions::ALL,
            download_limit: None,
            downloads_used: 0,
            ip_allow: vec![],
            ip_first_n: None,
            expires_at: None,
            embed: false,
            allow_helpers: true,
            helpers_disabled_at: None,
            revoked_at: None,
            created_at: 100,
            created_by: String::new(),
        }
    }

    fn body(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn edits_apply_and_null_clears() {
        let mut s = share();
        apply_update(
            &mut s,
            &body(
                json!({"download_limit": 3, "ip_allow": "10.0.0.0/8, 192.168.1.2", "title": " x "}),
            ),
            200,
        )
        .unwrap();
        assert_eq!(s.download_limit, Some(3));
        assert_eq!(s.ip_allow, ["10.0.0.0/8", "192.168.1.2"]);
        assert_eq!(s.title.as_deref(), Some("x"));
        apply_update(
            &mut s,
            &body(json!({"download_limit": null, "title": ""})),
            200,
        )
        .unwrap();
        assert_eq!(s.download_limit, None);
        assert_eq!(s.title, None);
    }

    #[test]
    fn turning_helping_off_records_when() {
        let mut s = share();
        apply_update(&mut s, &body(json!({"allow_helpers": false})), 250).unwrap();
        assert_eq!(s.helpers_disabled_at, Some(250));
    }

    #[test]
    fn invalid_edits_are_rejected() {
        let mut s = share();
        assert!(apply_update(
            &mut s,
            &body(json!({"permissions": {"import": false, "read": false, "download": false}})),
            1
        )
        .is_err());
        let mut s = share();
        assert!(apply_update(&mut s, &body(json!({"nonsense": 1})), 1).is_err());
        let mut s = share();
        assert!(apply_update(&mut s, &body(json!({"download_limit": 0})), 1).is_err());
    }

    #[test]
    fn forwarded_headers_are_only_believed_from_listed_proxies() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Forwarded-For", "1.1.1.1, 203.0.113.9".parse().unwrap());
        let loopback_only = parse_trusted_proxies("127.0.0.1/32, ::1/128");
        let public: IpAddr = "198.51.100.4".parse().unwrap();
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        let private: IpAddr = "172.17.0.1".parse().unwrap();

        assert_eq!(
            forwarded_address(&headers, public, &loopback_only).to_string(),
            "198.51.100.4"
        );
        assert_eq!(
            forwarded_address(&headers, loopback, &loopback_only).to_string(),
            "203.0.113.9"
        );
        assert_eq!(
            forwarded_address(&HeaderMap::new(), loopback, &loopback_only).to_string(),
            "127.0.0.1"
        );
        // A private-range peer is not a proxy merely by being private: anyone on the LAN can
        // write that header, so its own address stands unless the operator lists it.
        assert_eq!(
            forwarded_address(&headers, private, &loopback_only).to_string(),
            "172.17.0.1"
        );
        let listed = parse_trusted_proxies("127.0.0.1/32, 172.17.0.0/16");
        assert_eq!(
            forwarded_address(&headers, private, &listed).to_string(),
            "203.0.113.9"
        );
        // An emptied setting trusts nobody.
        assert_eq!(
            forwarded_address(&headers, loopback, &[]).to_string(),
            "127.0.0.1"
        );
    }

    #[test]
    fn the_list_summary_carries_the_share_code() {
        let summary = ShareSummary {
            id: "s".into(),
            title: String::new(),
            status: ShareStatus::Active,
            permissions: lanrurugi_share_proto::messages::Permissions::ALL,
            downloads_used: 0,
            download_limit: None,
            item_count: 1,
            created_at: 0,
            expires_at: None,
            embed: false,
            allow_helpers: false,
            short_code: false,
            restricted: false,
            share_code: Some("K7QM9-XRT2H".into()),
        };
        let value = serde_json::to_value(&summary).unwrap();
        assert_eq!(value["share_code"], "K7QM9-XRT2H");
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(unhex(&hex(&[0, 15, 255])).unwrap(), vec![0, 15, 255]);
        assert!(unhex("abc").is_none());
    }
}
