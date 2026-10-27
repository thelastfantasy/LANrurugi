//! Helping deliver other instances' shares (017 US7, FR-033–FR-035a, research §6).
//!
//! An instance that imported a share with "help deliver" on registers with the sharer, then sends
//! a heartbeat every five minutes and stores each answer in the import record. While the sharer
//! reports `active`, it serves `Range` requests for the archives it holds to importers carrying a
//! ticket signed by the sharer. It never serves outboards: importers verify every piece against
//! the root from the sharer's own manifest.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use iroh::{EndpointAddr, EndpointId, RelayUrl};
use lanrurugi_core::ids::{ArchiveId, ShareImportId};
use lanrurugi_share_proto::code::ShareRef;
use lanrurugi_share_proto::messages::{
    HelpReason, HelpState as WireState, HelperAddr, HelperStatus, RefusalReason, Request, Response,
};
use lanrurugi_storage::shares::{HelpState, ImportArchiveState, ImportState, ShareImport};
use tokio::sync::Notify;

use super::{call, endpoint_hex, publish, ShareNet, ShareNetError};
use crate::AppState;

pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(5 * 60);

/// Serialises read-modify-write of import records between the helper loop, range serving and
/// the import API, which all store whole documents.
pub fn record_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn wake_signal() -> &'static Notify {
    static WAKE: OnceLock<Notify> = OnceLock::new();
    WAKE.get_or_init(Notify::new)
}

/// Imports registered with their sharer during this process's life.
fn registered() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(Default::default)
}

/// Bytes served per import since the last flush into its record.
fn served() -> &'static Mutex<HashMap<String, u64>> {
    static MAP: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    MAP.get_or_init(Default::default)
}

/// Runs the next heartbeat round now: an import just finished or helping was switched on.
pub fn wake() {
    wake_signal().notify_one();
}

pub fn count_served(import_id: &ShareImportId, bytes: u64) {
    *served()
        .lock()
        .expect("served lock")
        .entry(import_id.to_string())
        .or_default() += bytes;
}

/// The helper loop, for as long as the process lives.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        loop {
            round(&state).await;
            tokio::select! {
                _ = tokio::time::sleep(HEARTBEAT_EVERY) => {}
                _ = wake_signal().notified() => {}
            }
        }
    });
}

fn helps(record: &ShareImport) -> bool {
    record.help_deliver
        && !record.help_hidden
        && record.state == ImportState::Done
        && record.help_state != HelpState::Ended
}

async fn round(state: &AppState) {
    flush_served(state).await;
    let Some(net) = super::handle().await else {
        return;
    };
    let imports = super::import::repo(state).list().await.unwrap_or_default();
    for record in imports.into_iter().filter(helps) {
        contact(state, &net, &record).await;
    }
}

/// Adds the bytes served since the last round to each record.
async fn flush_served(state: &AppState) {
    let pending: Vec<(String, u64)> = served().lock().expect("served lock").drain().collect();
    if pending.is_empty() {
        return;
    }
    let _guard = record_lock().lock().await;
    let repo = super::import::repo(state);
    for (id, bytes) in pending {
        if let Ok(Some(mut record)) = repo.get(&ShareImportId(id)).await {
            record.help_bytes_served += bytes;
            let _ = repo.save(&record).await;
        }
    }
}

/// Where importers can dial this instance.
fn own_addr(net: &ShareNet) -> (Vec<String>, Vec<String>) {
    let addr = net.endpoint().addr();
    (
        addr.relay_urls().map(|u| u.to_string()).collect(),
        addr.ip_addrs().map(|a| a.to_string()).collect(),
    )
}

/// One register-or-heartbeat exchange with the sharer, its answer stored in the record.
async fn contact(state: &AppState, net: &ShareNet, record: &ShareImport) {
    let Some(code) = record.code.as_deref() else {
        return;
    };
    let Ok(reference) = ShareRef::parse(code) else {
        return;
    };
    let conn = match publish::dial(state, net, &reference).await {
        Ok((id, conn)) if endpoint_hex(&id) == record.sharer_endpoint_id => Ok(conn),
        Ok(_) => Err(ShareNetError::Unreachable(
            "the code now points elsewhere".into(),
        )),
        Err(e) => Err(e),
    };
    let Ok(conn) = conn else {
        // Unreachable: the record's last contact ages into `sharer_offline`.
        return;
    };
    let key = record.id.to_string();
    let local: Vec<ArchiveId> = record
        .archives
        .iter()
        .filter_map(|a| a.local_archive_id.clone().map(ArchiveId))
        .collect();
    let register = || {
        let (relay_urls, addrs) = own_addr(net);
        Request::HelperRegister {
            share_id: record.share_id.clone(),
            archives: record
                .archives
                .iter()
                .filter(|a| {
                    matches!(
                        a.state,
                        ImportArchiveState::Done | ImportArchiveState::Reused
                    )
                })
                .map(|a| a.sharer_archive_id.clone())
                .collect(),
            relay_urls,
            addrs,
        }
    };
    let first = !registered().lock().expect("registered lock").contains(&key);
    let request = if first {
        register()
    } else {
        Request::HelperHeartbeat {
            share_id: record.share_id.clone(),
        }
    };
    let mut answer = call(&conn, request).await;
    if !first && matches!(answer, Err(ShareNetError::Refused(RefusalReason::NotFound))) {
        // The sharer lost the registration (it restarted or the record expired): register again.
        registered().lock().expect("registered lock").remove(&key);
        answer = call(&conn, register()).await;
    }
    let status = match answer {
        Ok((Response::HelperStatus(status), _)) => status,
        // Helpers are not (or no longer) allowed, or the share is gone: helping ends.
        Ok(_)
        | Err(ShareNetError::Refused(RefusalReason::PermissionDenied | RefusalReason::NotFound)) => {
            HelperStatus {
                state: WireState::Ended,
                reason: Some(HelpReason::HelpingDisabled),
                at: crate::shares::now_secs() as u64,
                removed_archives: Vec::new(),
            }
        }
        Err(_) => return,
    };
    registered().lock().expect("registered lock").insert(key);
    if status.state == WireState::Active && first {
        // Outboards for the local copies, so the first importer's ranges need no hashing pass.
        let state = state.clone();
        tokio::spawn(async move {
            super::blobs::prepare(&state, &local).await;
        });
    }
    apply_status(state, &record.id, &status).await;
}

pub fn reason_code(reason: HelpReason) -> &'static str {
    match reason {
        HelpReason::Exhausted => "exhausted",
        HelpReason::Revoked => "revoked",
        HelpReason::Expired => "expired",
        HelpReason::HelpingDisabled => "helping_disabled",
    }
}

/// Stores a `HelperStatus` answer (data-model.md "Help state transitions"). `ended` is final: the
/// code is dropped, so nothing reconnects for this share again.
pub async fn apply_status(state: &AppState, id: &ShareImportId, status: &HelperStatus) {
    let _guard = record_lock().lock().await;
    let repo = super::import::repo(state);
    let Ok(Some(mut record)) = repo.get(id).await else {
        return;
    };
    if !record.help_deliver || record.help_state == HelpState::Ended {
        return;
    }
    record.help_state = match status.state {
        WireState::Active => HelpState::Active,
        WireState::Paused => HelpState::Paused,
        WireState::Ended => HelpState::Ended,
    };
    record.help_reason = status.reason.map(|r| reason_code(r).to_string());
    record.help_state_at = Some(status.at as i64);
    record.help_last_contact_at = Some(crate::shares::now_secs());
    record.help_removed_archives = status.removed_archives.clone();
    let _ = repo.save(&record).await;
}

/// The import through which this instance helps deliver `share_id`, when it serves it now.
pub async fn serving_import(state: &AppState, share_id: &str) -> Option<ShareImport> {
    super::import::repo(state)
        .list()
        .await
        .ok()?
        .into_iter()
        .find(|r| r.share_id == share_id && r.serves_help())
}

/// The local archive holding `blake3` in a helped import, unless the sharer removed it.
pub fn local_archive(record: &ShareImport, blake3_hex: &str) -> Option<ArchiveId> {
    record
        .archives
        .iter()
        .filter(|a| !record.help_removed_archives.contains(&a.sharer_archive_id))
        .find(|a| a.blake3.as_deref() == Some(blake3_hex))
        .and_then(|a| a.local_archive_id.clone())
        .map(ArchiveId)
}

/// A helper's dialable address from the sharer's `Claimed`/`Renewed` list.
pub fn helper_endpoint_addr(helper: &HelperAddr) -> Option<EndpointAddr> {
    let id = EndpointId::from_bytes(&helper.endpoint_id).ok()?;
    let mut addr = EndpointAddr::new(id);
    for relay in &helper.relay_urls {
        if let Ok(url) = relay.parse::<RelayUrl>() {
            addr = addr.with_relay_url(url);
        }
    }
    for ip in &helper.addrs {
        if let Ok(ip) = ip.parse::<SocketAddr>() {
            addr = addr.with_ip_addr(ip);
        }
    }
    Some(addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_storage::shares::{ImportArchive, HELP_OFFLINE_AFTER_SECS};

    fn record() -> ShareImport {
        let archive = |id: &str, hash: &str| ImportArchive {
            sharer_archive_id: id.into(),
            title: String::new(),
            blake3: Some(hash.into()),
            size: 1,
            state: ImportArchiveState::Done,
            bytes_done: 1,
            error: None,
            local_archive_id: Some(format!("local-{id}")),
        };
        ShareImport {
            id: ShareImportId("i".into()),
            code: Some("x".into()),
            sharer_endpoint_id: String::new(),
            share_id: "s".into(),
            claim_id: String::new(),
            title: String::new(),
            manifest: serde_json::Value::Null,
            category: None,
            queue_item_id: String::new(),
            archives: vec![archive("a", "aa"), archive("b", "bb")],
            concurrency: 1,
            help_deliver: true,
            help_hidden: false,
            help_state: HelpState::Active,
            help_reason: None,
            help_state_at: None,
            help_last_contact_at: Some(1_000),
            help_removed_archives: vec!["b".into()],
            help_bytes_served: 0,
            inherit_categories: false,
            state: ImportState::Done,
            error: None,
            paused_until: None,
            created_at: 0,
        }
    }

    #[test]
    fn removed_archives_are_never_served() {
        let r = record();
        assert_eq!(local_archive(&r, "aa"), Some(ArchiveId("local-a".into())));
        assert_eq!(local_archive(&r, "bb"), None);
        assert_eq!(local_archive(&r, "cc"), None);
    }

    #[test]
    fn serving_needs_the_opt_in_a_finished_import_and_an_active_state() {
        let mut r = record();
        assert!(r.serves_help());
        r.help_state = HelpState::Paused;
        assert!(!r.serves_help());
        r.help_state = HelpState::Active;
        r.help_deliver = false;
        assert!(!r.serves_help());
        r.help_deliver = true;
        r.state = ImportState::Transferring;
        assert!(!r.serves_help());
    }

    #[test]
    fn a_silent_sharer_shows_as_offline_but_ended_stays_ended() {
        let mut r = record();
        assert_eq!(
            r.shown_help_state(1_000 + 60, HELP_OFFLINE_AFTER_SECS),
            "active"
        );
        assert_eq!(
            r.shown_help_state(1_000 + HELP_OFFLINE_AFTER_SECS + 1, HELP_OFFLINE_AFTER_SECS),
            "sharer_offline"
        );
        r.help_state = HelpState::Ended;
        assert_eq!(
            r.shown_help_state(1_000_000, HELP_OFFLINE_AFTER_SECS),
            "ended"
        );
    }
}
