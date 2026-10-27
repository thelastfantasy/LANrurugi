//! Reachability of the outside services in use, for Settings → Sharing (FR-049).

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct ServiceStatus {
    pub url: String,
    pub kind: &'static str,
    pub reachable: Option<bool>,
    pub last_ok_at: Option<i64>,
}

static SERVICES: Mutex<Option<HashMap<String, ServiceStatus>>> = Mutex::new(None);

fn record(kind: &'static str, url: &str, ok: bool) {
    let mut guard = SERVICES.lock().expect("service status lock");
    let entry = guard
        .get_or_insert_with(HashMap::new)
        .entry(url.to_string())
        .or_insert_with(|| ServiceStatus {
            url: url.to_string(),
            kind,
            ..Default::default()
        });
    entry.reachable = Some(ok);
    if ok {
        entry.last_ok_at = Some(crate::shares::now_secs());
    }
}

pub fn record_rendezvous(url: &str, ok: bool) {
    record("rendezvous", url, ok);
}

/// Every service seen so far.
pub fn snapshot() -> Vec<ServiceStatus> {
    let guard = SERVICES.lock().expect("service status lock");
    let mut list: Vec<ServiceStatus> = guard
        .as_ref()
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default();
    list.sort_by(|a, b| a.url.cmp(&b.url));
    list
}
