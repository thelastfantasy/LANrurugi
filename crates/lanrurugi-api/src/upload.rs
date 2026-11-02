//! `PUT /archives/upload`, the resumable-session routes beside it, and `/tempfolder` — shapes
//! verified against
//! `~/LANraragi/tools/openapi.yaml`. Reuses `download_manager::ingest::ingest_downloaded_file`
//! (originally written for the download pipeline, per that module's own docs) rather than a
//! second, parallel stage→catalog→rename-into-`archive_dir`→`LRR_FILEMAP`-fixup implementation —
//! this is also what gives a local upload the exact same persisted download-queue state machine a
//! download gets: it survives a page refresh, offers the same overwrite/rename UI for a filename
//! collision instead of a flat 409 rejection, and shows a Remove button through the same
//! `DELETE /download_queue/{id}` endpoint.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path as AxumPath, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use futures_util::StreamExt;
use lanrurugi_storage::download_queue::{DownloadQueueState, NewQueueItem, QueueItemOrigin};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha1::{Digest, Sha1};

use crate::activity::record_manual;
use crate::auth_context::AuthContext;
use crate::common::error;
use crate::download_manager::ingest::ingest_downloaded_file;
use crate::download_manager::stream::DownloadedFile;
use crate::plugins::update_queue_item_state;
use crate::AppState;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};

/// Axum's `Multipart` extractor enforces `DefaultBodyLimit`'s built-in 2 MB default unless a layer
/// overrides it, so "no limit" has to be said explicitly: dropping the old `.max(2 GiB)` layer
/// would fall back to 2 MB (and fail with `multer`'s size-exceeded error), not to unlimited.
/// Archives over 2 GB are real — this instance's own library holds an 8.3 GB one — and the 2 GiB
/// that used to be the cap was a *request body* cap that happened to equal the handler's memory
/// footprint, because the handler buffered the whole file before writing it.
///
/// Unbounded is only safe because the field is now streamed straight to disk
/// ([`stream_field_to_file`]): nothing larger than one network chunk is ever held in memory, so the
/// only resource an upload can exhaust is disk space — which is the honest limit anyway. Scoped to
/// this route only, not applied globally, so every other endpoint keeps the small default.
/// The fixed `plugin_namespace` every local-upload queue item is stored under — never a real
/// installed plugin's namespace (validated nowhere against `PluginPool`, since nothing in this
/// path ever calls a plugin to move bytes the way a download does), just this item type's own
/// grouping key on the Upload page.
const LOCAL_UPLOAD_NAMESPACE: &str = "local_upload";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/archives/upload",
            put(upload_archive).layer(DefaultBodyLimit::disable()),
        )
        // The resumable path (see `UploadSession`): a session is created once, chunks are appended
        // by offset, and the existing ingest runs at `complete`. The one-shot route above stays for
        // API clients and for any frontend older than this — it is the same ingest either way.
        .route("/archives/upload/session", post(create_upload_session))
        .route(
            "/archives/upload/session/{id}",
            get(upload_session_status)
                .put(append_upload_chunk)
                .delete(delete_upload_session)
                // A chunk is *supposed* to be small, so unlike the one-shot route this is bounded
                // rather than unbounded — one chunk's worth of memory, not one archive's.
                .layer(DefaultBodyLimit::max(MAX_CHUNK_BYTES)),
        )
        .route(
            "/archives/upload/session/{id}/complete",
            post(complete_upload_session),
        )
        .route("/tempfolder", delete(clean_tempfolder))
}

// ─────────────────────────── resumable upload sessions ───────────────────────────

/// How long an untouched session's staged bytes are kept before a sweep reclaims them. Long enough
/// that a genuinely interrupted upload can be resumed the next day, short enough that an abandoned
/// multi-GB partial does not sit in `temp_dir` forever.
const SESSION_TTL_SECS: u64 = 24 * 60 * 60;

/// Largest single chunk accepted, matching the body limit on the append route. Deliberately far
/// below the one-shot route's unbounded limit: a chunk is a small, repeatable unit, and bounding it
/// is what keeps a resumable upload's memory use flat no matter how large the archive is.
pub(crate) const MAX_CHUNK_BYTES: usize = 64 * 1024 * 1024;

/// The sidecar describing a staged, resumable upload. Mirrors the download side's own
/// `{resume_path}.meta.json` — the same problem (a partial file plus the context needed to continue
/// it) solved the same way, rather than inventing a Redis schema for bytes that already have a
/// natural home on disk next to them.
#[derive(Debug, Serialize, Deserialize)]
struct UploadSession {
    version: u32,
    upload_id: String,
    /// The client's own filename, kept for the extension the staging path must carry (see
    /// [`staging_path_for`]) and for the queue item's display name.
    file_name: String,
    /// What the client announced as the total size, when it announced anything. Checked at
    /// `complete` only: a mismatch there means the client is uploading something other than what it
    /// said, which is worth refusing.
    #[serde(default)]
    file_size: Option<u64>,
    /// The client's stable key for this file (name + size + last-modified), so a refreshed page can
    /// re-attach to its own session without having kept the id. Never trusted as identity on its
    /// own — it only ever selects among *this* caller's staged files.
    #[serde(default)]
    client_key: Option<String>,
    created_at: u64,
    updated_at: u64,
}

/// Serializes appends per session id. Two tabs appending to one session must not interleave
/// (each would otherwise stat the file, both see the same length, and both write `>`) — and the
/// session is exactly the unit a client is expected to serialize itself, so a per-id lock costs
/// nothing in the normal case.
fn session_locks() -> &'static Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>
{
    static LOCKS: OnceLock<Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn session_lock(id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = session_locks().lock().expect("session lock map poisoned");
    locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    locks
        .entry(id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Same extension-carrying naming as [`staging_path_for`], with the session id in place of a random
/// uuid — `catalogue_new_archive` reads the format off this path's extension, so it must survive
/// here too.
fn session_data_path(temp_dir: &Path, id: &str, file_name: &str) -> PathBuf {
    match Path::new(file_name).extension().and_then(|e| e.to_str()) {
        Some(ext) => temp_dir.join(format!("upload-session-{id}.{ext}")),
        None => temp_dir.join(format!("upload-session-{id}")),
    }
}

fn session_meta_path(temp_dir: &Path, id: &str) -> PathBuf {
    temp_dir.join(format!("upload-session-{id}.meta.json"))
}

/// The session id a sidecar's filename denotes, or `None` for anything else in `temp_dir` — which
/// is most of it (resize caches, one-shot staging files, `.tmp` leftovers).
fn session_id_from_meta_name(name: &str) -> Option<&str> {
    name.strip_prefix("upload-session-")
        .and_then(|rest| rest.strip_suffix(".meta.json"))
        .filter(|id| !id.is_empty())
}

/// Whether a session that was last touched at `updated_at` should be swept at `now`. Split out so
/// the boundary is testable without building an `AppState` or waiting a day.
fn session_is_expired(updated_at: u64, now: u64) -> bool {
    updated_at.saturating_add(SESSION_TTL_SECS) < now
}

/// Writes the sidecar atomically (temp file + rename), so a sweep or a crash can never observe a
/// half-written JSON document and mistake a live session for a broken one.
async fn write_session(state: &AppState, session: &UploadSession) -> std::io::Result<()> {
    let path = session_meta_path(&state.library.temp_dir, &session.upload_id);
    let temporary = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(session).unwrap_or_default();
    tokio::fs::write(&temporary, body).await?;
    tokio::fs::rename(&temporary, &path).await
}

async fn read_session(state: &AppState, id: &str) -> Option<UploadSession> {
    let body = tokio::fs::read(session_meta_path(&state.library.temp_dir, id))
        .await
        .ok()?;
    let session: UploadSession = serde_json::from_slice(&body).ok()?;
    // A session whose bytes are gone is not a session: treat it exactly like an expired one so the
    // caller starts over instead of appending to nothing.
    let data = session_data_path(&state.library.temp_dir, id, &session.file_name);
    tokio::fs::metadata(&data).await.ok()?;
    Some(session)
}

/// Bytes actually on disk for a session — the authority for every offset decision, never a number
/// the client sent.
async fn staged_bytes(state: &AppState, session: &UploadSession) -> u64 {
    tokio::fs::metadata(session_data_path(
        &state.library.temp_dir,
        &session.upload_id,
        &session.file_name,
    ))
    .await
    .map(|m| m.len())
    .unwrap_or(0)
}

async fn drop_session(state: &AppState, id: &str) {
    if let Some(session) = read_session(state, id).await {
        let _ = tokio::fs::remove_file(session_data_path(
            &state.library.temp_dir,
            id,
            &session.file_name,
        ))
        .await;
    }
    // Also removes a sidecar whose data file is already gone (which `read_session` refuses).
    let _ = tokio::fs::remove_file(session_meta_path(&state.library.temp_dir, id)).await;
    let _ = tokio::fs::remove_file(
        session_meta_path(&state.library.temp_dir, id).with_extension("json.tmp"),
    )
    .await;
}

/// Reclaims sessions nobody has touched for [`SESSION_TTL_SECS`]. Run on the session endpoints
/// themselves rather than from a scheduler: this is the only feature touching these files, the
/// directory holds at most a handful of them, and a sweep on the operations that *create* the
/// garbage cannot drift out of sync with them.
async fn sweep_expired_sessions(state: &AppState) {
    let Ok(mut entries) = tokio::fs::read_dir(&state.library.temp_dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(id) = session_id_from_meta_name(&name) else {
            continue;
        };
        let stale = match read_session(state, id).await {
            Some(session) => session_is_expired(session.updated_at, now_secs()),
            // Unreadable or orphaned sidecar: nothing can resume it, so it is garbage by
            // definition rather than by age.
            None => true,
        };
        if stale {
            drop_session(state, id).await;
        }
    }
}

/// Filenames are taken from the multipart field only for their extension/base name — never used
/// as a path, so a client can't traverse outside `archive_dir` via `../../etc/passwd`-style names.
pub(crate) fn sanitize_filename(name: &str) -> String {
    Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("upload")
        .to_string()
}

/// Streams one multipart file field straight into `path`, hashing and counting as each chunk
/// arrives. Nothing larger than a single network chunk is ever held in memory — which is what makes
/// the route's `DefaultBodyLimit::disable()` safe rather than reckless (see that layer's own docs).
/// Returns `(bytes written, lowercase-hex SHA-1)`, removing the partial file if the transfer fails
/// part-way, so a dropped connection cannot leave a half-written archive in `temp_dir`.
async fn stream_field_to_file(
    field: &mut axum::extract::multipart::Field<'_>,
    path: &Path,
) -> Result<(u64, String), String> {
    use tokio::io::AsyncWriteExt;

    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| format!("could not stage the upload: {e}"))?;
    let mut hasher = Sha1::new();
    let mut written: u64 = 0;
    loop {
        let chunk = match field.chunk().await {
            Ok(chunk) => chunk,
            Err(e) => {
                let _ = tokio::fs::remove_file(path).await;
                return Err(e.to_string());
            }
        };
        let Some(chunk) = chunk else { break };
        hasher.update(&chunk);
        written += chunk.len() as u64;
        if let Err(e) = file.write_all(&chunk).await {
            let _ = tokio::fs::remove_file(path).await;
            return Err(format!("could not stage the upload: {e}"));
        }
    }
    if let Err(e) = file.flush().await {
        let _ = tokio::fs::remove_file(path).await;
        return Err(format!("could not stage the upload: {e}"));
    }
    Ok((written, hex_encode(&hasher.finalize())))
}

/// Staging path for an upload: `temp_dir` (never watched — `crate::watcher::watch` only recurses
/// `archive_dir`), carrying the uploaded file's own extension. That extension is load-bearing —
/// the identical fix exists in `download_manager::stream::download_one`, whose call site documents
/// the full story: `lanrurugi_scanner::pipeline::catalogue_new_archive` computes `pagecount` via
/// `archive_format::list_pages` against *this* staging path, a pure extension-based format check,
/// so a bare `upload-<uuid>` always fails it and the archive is silently and permanently stuck at
/// `pagecount: 0` however the final, renamed destination file looks.
fn staging_path_for(state: &AppState, file_name: Option<&str>) -> PathBuf {
    let unique = uuid::Uuid::new_v4().simple();
    match file_name.and_then(|name| Path::new(name).extension().and_then(|e| e.to_str())) {
        Some(ext) => state
            .library
            .temp_dir
            .join(format!("upload-{unique}.{ext}")),
        None => state.library.temp_dir.join(format!("upload-{unique}")),
    }
}

/// A fully staged upload, however it got there (one-shot multipart or a resumed session). This is
/// the hand-off point between "getting the bytes onto disk" and "cataloguing them", so both paths
/// hand off through the identical ingest.
pub(crate) struct StagedUpload {
    pub path: PathBuf,
    pub file_name: String,
    pub bytes: u64,
    pub sha1: String,
}

/// The user-supplied metadata a client may attach to an upload. Names match the one-shot
/// multipart field names, since they are the same feature reached two ways.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct UploadMetadata {
    #[serde(default)]
    pub catid: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub tags: Option<String>,
}

/// Streams a whole file through SHA-1 without holding it in memory. Only used by the session path's
/// `complete`: the one-shot path hashes as the bytes arrive, but a resumed upload's early chunks
/// were hashed by processes that are long gone, so the honest options are "persist hasher state and
/// trust it to stay in sync with the file" or "hash the finished file once". This is the second:
/// self-healing by construction (no state that can desync from the bytes), at the cost of one
/// sequential read of an already-local file.
async fn sha1_of_file(path: &Path) -> Result<String, String> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("could not read the staged upload: {e}"))?;
    let mut hasher = Sha1::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|e| format!("could not read the staged upload: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

/// The one-shot route: one multipart request carrying the whole file. Kept as-is for API clients
/// (and it is still what `PUT /archives/upload` documents), while the UI uses the resumable
/// session routes below so a dropped connection costs a chunk rather than the whole transfer.
async fn upload_archive(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    mut multipart: Multipart,
) -> Response {
    // `(staging path, bytes written, lowercase-hex SHA-1)` — the file streams to disk as it
    // arrives, so nothing larger than one network chunk is ever held in memory.
    let mut staged: Option<(PathBuf, u64, String)> = None;
    let mut file_name: Option<String> = None;
    let mut checksum: Option<String> = None;
    let mut metadata = UploadMetadata::default();

    loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return error(StatusCode::BAD_REQUEST, "upload", e.to_string()),
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" => {
                // More than one `file` part is not a shape any caller sends; the old buffer-based
                // code kept the last one and the earlier bytes simply became unreachable. Keep the
                // same "last wins" behaviour without leaking the earlier part's staged file.
                if let Some((previous, _, _)) = staged.take() {
                    let _ = tokio::fs::remove_file(&previous).await;
                }
                file_name = field.file_name().map(sanitize_filename);
                // Computed here, before the body is read, because the staging path has to carry the
                // upload's own extension — see `staging_path_for`'s docs.
                let path = staging_path_for(&state, file_name.as_deref());
                match stream_field_to_file(&mut field, &path).await {
                    Ok((written, sha1)) => staged = Some((path, written, sha1)),
                    Err(e) => return error(StatusCode::BAD_REQUEST, "upload", e),
                }
            }
            "file_checksum" => checksum = field.text().await.ok(),
            "catid" => metadata.catid = field.text().await.ok().filter(|s| !s.is_empty()),
            "title" => metadata.title = field.text().await.ok(),
            "summary" => metadata.summary = field.text().await.ok(),
            "tags" => metadata.tags = field.text().await.ok(),
            _ => {}
        }
    }

    let Some((path, bytes, sha1)) = staged else {
        return error(StatusCode::BAD_REQUEST, "upload", "No file provided.");
    };
    finalize_staged_upload(
        &state,
        auth.as_ref().map(|e| &e.0),
        StagedUpload {
            path,
            file_name: file_name.unwrap_or_else(|| "upload.zip".to_string()),
            bytes,
            sha1,
        },
        metadata,
        checksum,
    )
    .await
}

// ─────────────────────────── session endpoints ───────────────────────────

#[derive(Debug, Deserialize)]
struct CreateSessionBody {
    file_name: String,
    #[serde(default)]
    file_size: Option<u64>,
    #[serde(default)]
    client_key: Option<String>,
}

/// `POST /archives/upload/session` — start (or re-attach to) a resumable upload.
///
/// Re-attaching is what makes a page refresh survivable: the client sends the same `client_key` it
/// used before, and gets back the id *and* the offset its bytes are already at. The offset comes
/// from the file on disk, never from the sidecar, so a truncated or hand-edited sidecar cannot make
/// the server believe it has bytes it does not.
async fn create_upload_session(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<CreateSessionBody>,
) -> Response {
    sweep_expired_sessions(&state).await;

    let file_name = sanitize_filename(&body.file_name);
    if let Some(key) = body.client_key.as_deref().filter(|k| !k.is_empty()) {
        if let Ok(mut entries) = tokio::fs::read_dir(&state.library.temp_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let Some(id) = session_id_from_meta_name(&name) else {
                    continue;
                };
                let Some(session) = read_session(&state, id).await else {
                    continue;
                };
                // Same client key *and* same name+size: the client is asking about the file it
                // already started, not a different file that happens to hash its key the same way.
                if session.client_key.as_deref() == Some(key)
                    && session.file_name == file_name
                    && session.file_size == body.file_size
                {
                    return axum::Json(json!({
                        "operation": "upload",
                        "success": 1,
                        "upload_id": session.upload_id,
                        "offset": staged_bytes(&state, &session).await,
                        "file_name": session.file_name,
                        "file_size": session.file_size,
                        "resumed": true,
                    }))
                    .into_response();
                }
            }
        }
    }

    let upload_id = uuid::Uuid::new_v4().simple().to_string();
    let now = now_secs();
    let session = UploadSession {
        version: 1,
        upload_id: upload_id.clone(),
        file_name: file_name.clone(),
        file_size: body.file_size,
        client_key: body.client_key.filter(|k| !k.is_empty()),
        created_at: now,
        updated_at: now,
    };
    // Create the (empty) data file up front so the session's own existence is a fact on disk from
    // the first byte, exactly like a `.part` file on the download side.
    let data = session_data_path(&state.library.temp_dir, &upload_id, &file_name);
    if let Err(e) = tokio::fs::File::create(&data).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "upload", e.to_string());
    }
    if let Err(e) = write_session(&state, &session).await {
        let _ = tokio::fs::remove_file(&data).await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, "upload", e.to_string());
    }

    axum::Json(json!({
        "operation": "upload",
        "success": 1,
        "upload_id": upload_id,
        "offset": 0,
        "file_name": file_name,
        "file_size": session.file_size,
        "resumed": false,
    }))
    .into_response()
}

/// `GET /archives/upload/session/:id` — the handshake a resuming client starts from.
async fn upload_session_status(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    match read_session(&state, &id).await {
        Some(session) => {
            let offset = staged_bytes(&state, &session).await;
            axum::Json(json!({
                "operation": "upload",
                "success": 1,
                "upload_id": session.upload_id,
                "file_name": session.file_name,
                "file_size": session.file_size,
                "offset": offset,
                "created_at": session.created_at,
                "updated_at": session.updated_at,
            }))
            .into_response()
        }
        // Unknown, expired, or already-completed: the client's only correct move is to start over,
        // which a 404 says more precisely than an empty session would.
        None => error(
            StatusCode::NOT_FOUND,
            "upload",
            "Unknown upload session.".to_string(),
        ),
    }
}

/// `PUT /archives/upload/session/:id` with `Upload-Offset: N` — append one chunk.
///
/// The offset is verified against the file's real length and the response always reports the
/// authoritative position: `204` plus `Upload-Offset` on success, `409` plus `Upload-Offset` when
/// the client's idea of where it is disagrees with the disk. That turns a lost response, a retried
/// chunk, or two tabs racing into a resync rather than a corrupted archive.
async fn append_upload_chunk(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let Some(session) = read_session(&state, &id).await else {
        return error(
            StatusCode::NOT_FOUND,
            "upload",
            "Unknown upload session.".to_string(),
        );
    };
    let Some(claimed) = headers
        .get("upload-offset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "upload",
            "Missing or malformed Upload-Offset header.".to_string(),
        );
    };

    let lock = session_lock(&id);
    let _guard = lock.lock().await;
    let current = staged_bytes(&state, &session).await;
    if claimed != current {
        return offset_response(StatusCode::CONFLICT, current);
    }

    use tokio::io::AsyncWriteExt;
    let data = session_data_path(&state.library.temp_dir, &id, &session.file_name);
    let mut file = match tokio::fs::OpenOptions::new().append(true).open(&data).await {
        Ok(file) => file,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "upload", e.to_string()),
    };
    let mut written = 0u64;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            // The connection died mid-chunk. What already reached the disk stays (that is the whole
            // point of the append contract) and the reported offset tells the client where to
            // resume; nothing here is half-valid, because a partial chunk is just fewer bytes.
            Err(e) => {
                let _ = file.flush().await;
                let actual = staged_bytes(&state, &session).await;
                return error(StatusCode::BAD_REQUEST, "upload", e.to_string())
                    .into_response_with_offset(actual);
            }
        };
        if let Err(e) = file.write_all(&chunk).await {
            let _ = file.flush().await;
            return error(StatusCode::INTERNAL_SERVER_ERROR, "upload", e.to_string());
        }
        written += chunk.len() as u64;
    }
    if let Err(e) = file.flush().await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "upload", e.to_string());
    }
    drop(file);

    let mut updated = session;
    updated.updated_at = now_secs();
    let _ = write_session(&state, &updated).await;

    offset_response(StatusCode::NO_CONTENT, current + written)
}

#[derive(Debug, Default, Deserialize)]
struct CompleteSessionBody {
    #[serde(default)]
    file_checksum: Option<String>,
    #[serde(flatten)]
    metadata: UploadMetadata,
}

/// `POST /archives/upload/session/:id/complete` — verify what landed, then ingest it.
async fn complete_upload_session(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    AxumPath(id): AxumPath<String>,
    axum::Json(body): axum::Json<CompleteSessionBody>,
) -> Response {
    let Some(session) = read_session(&state, &id).await else {
        return error(
            StatusCode::NOT_FOUND,
            "upload",
            "Unknown upload session.".to_string(),
        );
    };

    let lock = session_lock(&id);
    let _guard = lock.lock().await;
    let bytes = staged_bytes(&state, &session).await;
    if bytes == 0 {
        return error(
            StatusCode::BAD_REQUEST,
            "upload",
            "No file provided.".to_string(),
        );
    }
    // The announced size is a claim about which file this is, so a mismatch is refused — but the
    // two directions mean different things. Fewer bytes than announced is simply "not finished
    // yet": the session is kept and the offset returned, so the client can send the rest (a client
    // that races its own final chunk gets a resync rather than losing the transfer). More bytes
    // than announced means client and server disagree about what is being uploaded at all, which
    // nothing can be salvaged from.
    if let Some(expected) = session.file_size {
        if bytes < expected {
            return offset_response(StatusCode::CONFLICT, bytes);
        }
        if bytes > expected {
            drop_session(&state, &id).await;
            return (
                StatusCode::EXPECTATION_FAILED,
                axum::Json(json!({
                    "operation": "upload",
                    "error": format!("Size mismatch: received {bytes} bytes, expected {expected}."),
                    "success": 0,
                })),
            )
                .into_response();
        }
    }

    let path = session_data_path(&state.library.temp_dir, &id, &session.file_name);
    let sha1 = match sha1_of_file(&path).await {
        Ok(sha1) => sha1,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "upload", e),
    };

    // The session's sidecar is removed first: from here on the bytes belong to the ingest path,
    // which owns (and on failure deletes) them. Leaving the sidecar would let a double `complete`
    // try to ingest the same file twice, and would leave a swept "session" pointing at a file that
    // has already been renamed into the library.
    let _ = tokio::fs::remove_file(session_meta_path(&state.library.temp_dir, &id)).await;

    finalize_staged_upload(
        &state,
        auth.as_ref().map(|e| &e.0),
        StagedUpload {
            path,
            file_name: session.file_name.clone(),
            bytes,
            sha1,
        },
        body.metadata,
        body.file_checksum,
    )
    .await
}

/// `DELETE /archives/upload/session/:id` — give up on a session and reclaim its bytes.
async fn delete_upload_session(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    drop_session(&state, &id).await;
    axum::Json(json!({ "operation": "upload", "success": 1 })).into_response()
}

/// A bodyless response carrying the authoritative `Upload-Offset` — the one piece of state the
/// resumable contract needs to return on both success and conflict.
fn offset_response(status: StatusCode, offset: u64) -> Response {
    (
        status,
        [(
            header::HeaderName::from_static("upload-offset"),
            header::HeaderValue::from_str(&offset.to_string())
                .unwrap_or(header::HeaderValue::from_static("0")),
        )],
    )
        .into_response()
}

/// Attaches `Upload-Offset` to an error response. Used only on the mid-chunk failure path, where
/// the client genuinely needs to know how much of its chunk made it to disk.
trait ResponseWithOffset {
    fn into_response_with_offset(self, offset: u64) -> Response;
}

impl ResponseWithOffset for Response {
    fn into_response_with_offset(self, offset: u64) -> Response {
        let (mut parts, body) = self.into_parts();
        if let Ok(value) = header::HeaderValue::from_str(&offset.to_string()) {
            parts
                .headers
                .insert(header::HeaderName::from_static("upload-offset"), value);
        }
        Response::from_parts(parts, body)
    }
}

async fn finalize_staged_upload(
    state: &AppState,
    auth: Option<&AuthContext>,
    staged: StagedUpload,
    metadata: UploadMetadata,
    expected_checksum: Option<String>,
) -> Response {
    let StagedUpload {
        path: staging_path,
        file_name,
        bytes: bytes_written,
        sha1: actual_sha1,
    } = staged;
    let UploadMetadata {
        catid: category,
        title,
        summary,
        tags,
    } = metadata;

    if let Some(expected) = expected_checksum.as_deref().filter(|s| !s.is_empty()) {
        // The one-shot path hashed while streaming, the session path hashed the finished file —
        // either way the comparison costs no extra read here. A rejected upload removes its own
        // staged bytes: nothing has been catalogued yet, so leaving them would only be litter in
        // `temp_dir`.
        if !actual_sha1.eq_ignore_ascii_case(expected) {
            let _ = tokio::fs::remove_file(&staging_path).await;
            return (
                StatusCode::EXPECTATION_FAILED,
                axum::Json(json!({
                    "operation": "upload",
                    "error": "Checksum mismatch.",
                    "success": 0,
                })),
            )
                .into_response();
        }
    }

    // The bytes are already staged on disk by this point (they were streamed there as they
    // arrived), so this item only ever exists for a fully-received upload — a transfer the client
    // abandons mid-flight leaves no row behind, unlike the previous buffer-everything-then-record
    // order. Still `Queued` first, same as a fresh download, and flipped to `Done`/`Error` a few
    // lines down once the real outcome is known; it never sits in `Queued` waiting for a user's own
    // Start click the way a download does, because a local upload has nothing left to start.
    let queue_item = match state
        .download_queue
        .add(NewQueueItem {
            // A local upload belongs to no subscription.
            subscription_id: None,
            origin: QueueItemOrigin::LocalUpload,
            url: file_name.clone(),
            plugin_namespace: LOCAL_UPLOAD_NAMESPACE.to_string(),
            file_size: Some(bytes_written),
            category: category.clone(),
            metadata_tags: Vec::new(),
            auto_fetch_metadata: false,
            overwrite_on_duplicate: false,
            // A local upload has no subscription behind it; its collisions are resolved by hand.
            conflict_policy: lanrurugi_storage::subscriptions::ConflictPolicy::Ask,
            state: DownloadQueueState::Queued,
        })
        .await
    {
        Ok(item) => {
            if let Some(tx) = state.download_queue_tx.as_ref() {
                let _ = tx.send(serde_json::json!({ "kind": "add", "item": item }));
            }
            item
        }
        Err(e) => {
            // Nothing will ever ingest these bytes now, so don't leave them staged.
            let _ = tokio::fs::remove_file(&staging_path).await;
            return error(StatusCode::INTERNAL_SERVER_ERROR, "upload", e.to_string());
        }
    };

    // Written to `temp_dir` (never watched — `crate::watcher::watch` only recurses
    // `archive_dir`) and ingested *there* first, mirroring legacy's own `handle_incoming_file`
    // (`~/LANraragi/lib/LANraragi/Model/Upload.pm`): legacy computes the ID and registers the
    // archive in Redis *before* moving the file into the watched content folder, specifically
    // "so Shinobu doesn't do it" (its own comment). Doing it the other way around — write
    // straight into `archive_dir`, ingest afterward — raced against this project's own
    // notify-based watcher picking up the same just-written file and cataloguing it first via
    // its own `ingest_file` call: the upload handler's *own* explicit call then saw the ID as
    // already tracked and returned a spurious 409 "already exists" for what was genuinely a
    // first-time upload. Reproduced directly against a real running backend during
    // 003-ui-test-automation's implementation (confirmed by disabling the watcher via
    // `--no-watch`, which made the race disappear) before this fix.
    let downloaded = DownloadedFile {
        path: staging_path,
        filename: file_name.clone(),
        bytes_downloaded: bytes_written,
    };

    // Same overwrite/reject semantics as a download's own `overwrite_on_duplicate` checkbox
    // (always `false` here — see `NewQueueItem` above; a same-name re-upload is offered the
    // same overwrite/rename choice a download would get instead of an unconditional overwrite),
    // and the exact same `ContentHash`-vs-`Filename` collision handling: a byte-identical
    // duplicate is unconditionally rejected, while a filename-only collision is staged to
    // `temp_dir` and offered to the user via `.../overwrite`/`.../rename` — no longer a flat 409
    // with the staged bytes simply deleted, unlike this handler's previous behavior.
    // `None` — a local upload has no real external source URL to stamp a `source:` tag with (see
    // `ingest_downloaded_file`'s own docs); `file_name` here is just the uploaded file's name, not
    // a URL, and was previously being written into `source:` verbatim as if it were one.
    // The unified `archive.ingest` record is written around the same call — see
    // `download_manager::ingest::record_ingest_result`'s own docs. The separate `archive.upload`
    // record below still captures the user-supplied title/tags and metadata-plugin summary this
    // path has that the download/scanner sources don't.
    let ingest_result = crate::download_manager::ingest::record_ingest_result(
        state,
        auth,
        "upload",
        &file_name,
        json!({ "category": category.clone() }),
        ingest_downloaded_file(state, &downloaded, false, None, Some(&queue_item.id)).await,
    )
    .await;
    match ingest_result {
        Ok(ingested) => {
            if let Some(catid) = &category {
                let _ =
                    crate::categories::add_archive_to_category(state, catid, &ingested.archive_id)
                        .await;
            }

            if title.is_some() || summary.is_some() || tags.is_some() {
                if let Ok(Some(mut archive)) = state
                    .repos
                    .archives
                    .get(&lanrurugi_core::ids::ArchiveId(ingested.archive_id.clone()))
                    .await
                {
                    if let Some(t) = title {
                        archive.title = t;
                    }
                    if let Some(s) = summary {
                        archive.summary = s;
                    }
                    if let Some(t) = tags {
                        archive.tags = t;
                    }
                    let _ = state.repos.archives.save(&archive).await;
                }
            }

            // Legacy's own real order (`Model::Upload.pm`): user-supplied title/tags land first,
            // *then* every enabled metadata plugin runs and appends on top
            // (`set_tags(..., append=1)`) — matched here by calling this after, not before, the
            // user-supplied-fields block above.
            let plugin_summary = crate::plugins::run_enabled_metadata_plugins_on_archive(
                state,
                &ingested.archive_id,
            )
            .await;

            record_manual(
                state,
                auth,
                action_types::ARCHIVE_UPLOAD,
                ActivityTarget {
                    id: Some(ingested.archive_id.clone()),
                    label: Some(file_name.clone()),
                    kind: Some("archive".to_string()),
                },
                Outcome::Success,
                None,
                Some(json!({
                    "metadata_plugins_ran": plugin_summary.successes,
                    "metadata_plugins_failed": plugin_summary.failures,
                    "tags_added": plugin_summary.added_tags,
                    "title_updated": plugin_summary.new_title,
                })),
            )
            .await;

            update_queue_item_state(
                &state.download_queue,
                state.download_queue_tx.as_ref(),
                &queue_item.id,
                DownloadQueueState::Done,
                None,
                None,
                Some(vec![ingested.archive_id.clone()]),
            )
            .await;

            axum::Json(json!({
                "operation": "upload",
                "success": 1,
                "id": ingested.archive_id,
            }))
            .into_response()
        }
        Err(e) => {
            // `PendingRename` is not a genuine failure the way the other variants are — the
            // bytes are safe, staged in `temp_dir`, and `stage_pending_rename` (inside
            // `ingest_downloaded_file`) already persisted the `PendingFilenameConflict` itself
            // directly onto the queue item; this only needs to additionally record the matching
            // `state`/`error`, mirroring `plugins.rs::start_download`'s own error-recording
            // convention for the identical situation on the download side. The HTTP response
            // below is still a plain error status either way — the frontend never reads this
            // response body for its own error UI (this handler doesn't return an id/`Location`
            // for the queue item), it always resolves via the queue item's next
            // `GET /download_queue` poll.
            let queue_error = lanrurugi_core::queue_error::QueueError::from(&e);
            update_queue_item_state(
                &state.download_queue,
                state.download_queue_tx.as_ref(),
                &queue_item.id,
                DownloadQueueState::Error,
                None,
                Some(queue_error.clone()),
                None,
            )
            .await;

            // Bytes were staged and cataloging was genuinely attempted (`PendingRename` is
            // excluded — see the doc comment above the `match` this arm belongs to — since that
            // case isn't a real failure, the bytes just await a user decision). Checked against
            // `e` itself, not the converted `queue_error` — `IngestDownloadError::PendingRename`
            // and a genuine filename collision both convert to the same
            // `QueueError::DuplicateFilename` shape, so only the pre-conversion error
            // distinguishes them.
            if !matches!(
                e,
                crate::download_manager::ingest::IngestDownloadError::PendingRename(_)
            ) {
                record_manual(
                    state,
                    auth,
                    action_types::ARCHIVE_UPLOAD,
                    ActivityTarget {
                        id: None,
                        label: Some(file_name.clone()),
                        kind: Some("archive".to_string()),
                    },
                    Outcome::Failure {
                        reason: e.to_string(),
                    },
                    None,
                    None,
                )
                .await;
            }

            use lanrurugi_core::queue_error::QueueError;
            let (status, message, existing_id) = match &queue_error {
                QueueError::DuplicateArchive { existing_id, .. } => (
                    StatusCode::CONFLICT,
                    "This file already exists in the Library.".to_string(),
                    Some(existing_id.clone()),
                ),
                QueueError::DuplicateFilename { existing_id, .. } => (
                    StatusCode::CONFLICT,
                    "A file with this name already exists in the Library.".to_string(),
                    Some(existing_id.clone()),
                ),
                other => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("{other:?}"),
                    None,
                ),
            };
            (
                status,
                axum::Json(json!({
                    "operation": "upload",
                    "error": message,
                    "success": 0,
                    "id": existing_id,
                })),
            )
                .into_response()
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").unwrap();
    }
    s
}

/// Scoped to `temp_dir/resize_page/` only (the reader's WebP resize cache — see
/// `archives.rs::resize_cache_path`) — a plain "delete everything under `temp_dir`" (legacy's own
/// behavior, which has no equivalent of this app's persisted download-queue staging) would also
/// wipe `temp_dir/temp_*` files backing an unresolved `PendingFilenameConflict`: bytes a user
/// hasn't yet chosen "overwrite" or "rename and catalog" for on the Upload page. Those have their
/// own age-based sweep (`ingest::sweep_stale_pending_renames`) and must survive an admin clicking
/// "Clear Cache" for unrelated reasons.
async fn clean_tempfolder(State(state): State<AppState>) -> Response {
    let mut newsize: u64 = 0;
    let mut error_msg: Option<String> = None;

    let resize_cache_dir = state.library.temp_dir.join("resize_page");
    match tokio::fs::read_dir(&resize_cache_dir).await {
        Ok(mut entries) => {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let result = if path.is_dir() {
                    tokio::fs::remove_dir_all(&path).await
                } else {
                    tokio::fs::remove_file(&path).await
                };
                if let Err(e) = result {
                    error_msg = Some(e.to_string());
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => error_msg = Some(e.to_string()),
    }

    if let Ok(mut remaining) = tokio::fs::read_dir(state.library.temp_dir).await {
        while let Ok(Some(entry)) = remaining.next_entry().await {
            if let Ok(meta) = entry.metadata().await {
                newsize += meta.len();
            }
        }
    }

    axum::Json(json!({
        "operation": "cleantemp",
        "success": 1,
        "error": error_msg,
        "newsize": newsize,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The archive filename itself (as opposed to filenames *inside* the archive, covered in
    // `lanrurugi-scanner::archive_format`'s tests) goes through `std::path::Path`/`OsString`
    // throughout this codebase, which is lossless for any filename the OS accepts — no
    // ASCII-only assumption anywhere. These tests just confirm `sanitize_filename` (the one place
    // that touches the raw multipart field name) doesn't mangle spaces, Japanese punctuation, or
    // CJK characters while still stripping directory-traversal components.
    #[test]
    fn sanitize_filename_preserves_spaces() {
        assert_eq!(
            sanitize_filename("my manga volume 1.zip"),
            "my manga volume 1.zip"
        );
    }

    #[test]
    fn sanitize_filename_preserves_japanese_punctuation_and_cjk() {
        assert_eq!(
            sanitize_filename("【サークル】第一巻「初版」.zip"),
            "【サークル】第一巻「初版」.zip"
        );
    }

    #[test]
    fn sanitize_filename_preserves_simplified_and_traditional_chinese() {
        assert_eq!(
            sanitize_filename("测试文件（第一卷）.zip"),
            "测试文件（第一卷）.zip"
        );
        assert_eq!(
            sanitize_filename("測試檔案（第一卷）.zip"),
            "測試檔案（第一卷）.zip"
        );
    }

    #[test]
    fn sanitize_filename_preserves_korean() {
        assert_eq!(
            sanitize_filename("테스트 파일 1권.zip"),
            "테스트 파일 1권.zip"
        );
    }

    #[test]
    fn sanitize_filename_strips_directory_traversal_but_keeps_special_chars() {
        assert_eq!(
            sanitize_filename("../../etc/passwd/日本語 テスト.zip"),
            "日本語 テスト.zip"
        );
        assert_eq!(sanitize_filename("../../../etc/passwd"), "passwd");
    }

    #[test]
    fn sanitize_filename_falls_back_when_empty_or_only_traversal() {
        assert_eq!(sanitize_filename(""), "upload");
        assert_eq!(sanitize_filename("../.."), "upload");
    }

    #[test]
    fn session_data_path_keeps_the_extension_the_scanner_reads_the_format_from() {
        let temp = Path::new("/tmp");
        assert_eq!(
            session_data_path(temp, "abc", "archive.zip"),
            Path::new("/tmp/upload-session-abc.zip"),
            "catalogue_new_archive reads the format off this extension, so it must survive"
        );
        // A dotted but extension-less name, and a name with no dot at all, both fall back rather
        // than producing a path whose extension is a lie.
        assert_eq!(
            session_data_path(temp, "abc", "no-extension"),
            Path::new("/tmp/upload-session-abc")
        );
        // The id is generated server-side, but the path builder itself must not be the thing that
        // lets a client-chosen name escape the temp directory.
        assert_eq!(
            session_data_path(temp, "abc", "../../etc/passwd"),
            Path::new("/tmp/upload-session-abc")
        );
    }

    #[test]
    fn session_meta_path_pairs_with_the_data_file_by_id_alone() {
        let temp = Path::new("/tmp");
        assert_eq!(
            session_meta_path(temp, "abc"),
            Path::new("/tmp/upload-session-abc.meta.json")
        );
        // The pairing is by id, not by the data path: the extension lives only on the data side,
        // so resolving a session from its sidecar never has to guess what was uploaded.
        assert_eq!(
            session_id_from_meta_name("upload-session-abc.meta.json"),
            Some("abc")
        );
    }

    #[test]
    fn session_id_from_meta_name_ignores_everything_else_in_temp_dir() {
        // The temp directory holds resize caches, one-shot staging files and half-written sidecars
        // as well; a sweep that mistook any of them for a session would delete live data.
        assert_eq!(session_id_from_meta_name("upload-1234abcd.zip"), None);
        assert_eq!(session_id_from_meta_name("resize_page"), None);
        assert_eq!(
            session_id_from_meta_name("upload-session-abc.meta.json.tmp"),
            None
        );
        assert_eq!(session_id_from_meta_name("upload-session-.meta.json"), None);
    }

    #[test]
    fn session_is_expired_uses_a_whole_ttl_of_slack() {
        let now = 1_800_000_000;
        assert!(!session_is_expired(now, now));
        assert!(!session_is_expired(now - SESSION_TTL_SECS, now));
        // One second past the TTL is the first eligible moment, and a timestamp from the future
        // (clock skew) must not underflow into "expired".
        assert!(session_is_expired(now - SESSION_TTL_SECS - 1, now));
        assert!(!session_is_expired(now + 10_000, now));
    }
}
