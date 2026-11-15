//! Downloads served from a host name that is **not** behind the CDN.
//!
//! With the site itself behind a proxy (Cloudflare's orange cloud, a reverse proxy that buffers,
//! anything that terminates a request before it reaches us), a browser download has to leave that
//! proxy or it will be buffered, capped, or timed out halfway through a multi-gigabyte archive.
//! A download URL cannot simply be rewritten to another host: the archive route is session
//! authenticated, and a cookie does not follow a link to a different origin.
//!
//! So the frontend asks for a link (`GET /api/archives/{id}/download-link`) and this module signs
//! one: `/api/dl/archives/{id}?exp=…&sig=…`, aimed at the configured direct host. The signature
//! covers the resource and the expiry with the instance's own session secret, so the link needs
//! no cookie and cannot be edited into another archive's. The routes under `/api/dl` are the only
//! ones publicly reachable — they check that signature themselves.
//!
//! When no direct host is configured, `link` returns the ordinary relative path and nothing here
//! is involved.

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Response,
    routing::get,
    Router,
};
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use sha2::Sha256;

use crate::state::AppState;

/// Long enough for a slow start on a phone, short enough that a link pasted somewhere goes stale.
const LINK_TTL_SECS: i64 = 900;

#[derive(Debug, Deserialize)]
struct LinkQuery {
    exp: i64,
    sig: String,
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

fn sign(secret: &[u8], what: &str, expires: i64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(what.as_bytes());
    mac.update(b"\n");
    mac.update(expires.to_string().as_bytes());
    hex(&mac.finalize().into_bytes())
}

fn verify(secret: &[u8], what: &str, expires: i64, signature: &str) -> bool {
    let now = crate::shares::now_secs();
    // A link minted far in the future is not one of ours either: a clock jump must not turn a
    // captured link into a permanent one.
    if expires < now || expires > now + LINK_TTL_SECS * 2 {
        return false;
    }
    let Some(expected) = unhex(signature) else {
        return false;
    };
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(what.as_bytes());
    mac.update(b"\n");
    mac.update(expires.to_string().as_bytes());
    mac.verify_slice(&expected).is_ok()
}

/// The URL a browser should navigate to for a download: absolute on the direct host when one is
/// configured, the ordinary relative path otherwise.
pub async fn link(state: &AppState, what: &str, signed: &str, plain: &str) -> String {
    let Some(base) = crate::settings::direct_download_url(state).await else {
        return plain.to_string();
    };
    let Ok(auth) = crate::auth::load(state).await else {
        return plain.to_string();
    };
    let expires = crate::shares::now_secs() + LINK_TTL_SECS;
    let signature = sign(&auth.session_secret, what, expires);
    let separator = if signed.contains('?') { '&' } else { '?' };
    format!(
        "{}{signed}{separator}exp={expires}&sig={signature}",
        base.trim_end_matches('/')
    )
}

async fn signed(state: &AppState, what: &str, query: &LinkQuery) -> bool {
    let Ok(auth) = crate::auth::load(state).await else {
        return false;
    };
    verify(&auth.session_secret, what, query.exp, &query.sig)
}

/// `/api/dl/archives/{id}` — the same bytes as `/archives/{id}/download`, without a session.
async fn download_archive(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LinkQuery>,
    headers: HeaderMap,
) -> Response {
    if !signed(&state, &format!("archives:{id}"), &query).await {
        return crate::common::not_found("download_archive", "No archive ID specified.");
    }
    let archive_id = lanrurugi_core::ids::ArchiveId::from(id);
    let archive = match state.repos.archives.get(&archive_id).await {
        Ok(Some(archive)) => archive,
        Ok(None) => {
            return crate::common::not_found("download_archive", "No archive ID specified.")
        }
        Err(error) => {
            return crate::common::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "download_archive",
                error.to_string(),
            )
        }
    };
    crate::archives::serve_archive_download(
        &archive,
        crate::archives::byte_range(&headers),
        crate::archives::if_range(&headers),
    )
    .await
}

/// `/api/dl/tankoubons/{id}` — a whole tankoubon, without a session.
async fn download_tankoubon(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LinkQuery>,
    headers: HeaderMap,
) -> Response {
    if !signed(&state, &format!("tankoubons:{id}"), &query).await {
        return crate::common::not_found("download_tankoubon", "No tankoubon ID specified.");
    }
    let tank_id = lanrurugi_core::ids::TankId::from(id);
    match crate::tankoubons::tankoubon_zip_response(
        &state,
        &tank_id,
        crate::archives::byte_range(&headers),
        crate::archives::if_range(&headers),
    )
    .await
    {
        Some(response) => response,
        None => crate::common::not_found("download_tankoubon", "No tankoubon ID specified."),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/dl/archives/{id}", get(download_archive))
        .route("/dl/tankoubons/{id}", get(download_tankoubon))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_only_matches_its_own_resource_and_expiry() {
        let secret = b"secret";
        let expires = crate::shares::now_secs() + 60;
        let signature = sign(secret, "archives:a", expires);
        assert!(verify(secret, "archives:a", expires, &signature));
        assert!(!verify(secret, "archives:b", expires, &signature));
        assert!(!verify(secret, "archives:a", expires + 1, &signature));
        assert!(!verify(b"other", "archives:a", expires, &signature));
    }

    #[test]
    fn an_expired_or_far_future_link_is_refused() {
        let secret = b"secret";
        let now = crate::shares::now_secs();
        let past = now - 1;
        assert!(!verify(
            secret,
            "archives:a",
            past,
            &sign(secret, "archives:a", past)
        ));
        let future = now + LINK_TTL_SECS * 3;
        assert!(!verify(
            secret,
            "archives:a",
            future,
            &sign(secret, "archives:a", future)
        ));
    }

    #[test]
    fn a_malformed_signature_is_refused_without_panicking() {
        assert!(!verify(
            b"secret",
            "archives:a",
            crate::shares::now_secs() + 60,
            "zz"
        ));
        assert!(!verify(
            b"secret",
            "archives:a",
            crate::shares::now_secs() + 60,
            "abc"
        ));
    }
}
