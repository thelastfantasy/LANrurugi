//! The plugin egress interface: one switch for *all* plugin traffic.
//!
//! A site that refuses the host's own address refuses the metadata lookup and the login too, not
//! just the file — so the interface choice lives in [`lanrurugi_core::http_client`] (which the
//! plugin relay and this crate's download client both build from) rather than being applied only
//! to the last request of a plugin run. What is left here is the download client cache and the
//! settings-facing names.

use std::sync::Mutex;
use std::sync::OnceLock;

use super::stream::DownloadError;

/// The client built for the current interface. Rebound whenever the interface changes: reqwest
/// bakes the binding into each socket it creates, so a cached client would keep the old one.
static CLIENT: OnceLock<Mutex<Option<(String, reqwest::Client)>>> = OnceLock::new();

fn client_slot() -> &'static Mutex<Option<(String, reqwest::Client)>> {
    CLIENT.get_or_init(|| Mutex::new(None))
}

/// Applies an interface choice made in the settings. An empty name means "no binding".
pub fn set_outbound_interface(name: Option<String>) {
    let name = name.filter(|n| !n.trim().is_empty());
    tracing::info!(
        interface = name.as_deref().unwrap_or("-"),
        "plugin egress interface"
    );
    lanrurugi_core::http_client::set_outbound_interface(name);
    // Drop the cached client so the next download rebuilds against the new interface.
    *client_slot().lock().expect("outbound client lock") = None;
}

/// The interface currently in force, for the settings page and for logging.
pub fn outbound_interface() -> Option<String> {
    lanrurugi_core::http_client::outbound_interface()
}

/// The client downloads should use. Bound to `SO_BINDTODEVICE`-equivalent behaviour via reqwest's
/// `interface`, which is what makes "download through the VPN" real rather than advisory.
pub fn outbound_client() -> Result<reqwest::Client, DownloadError> {
    let Some(name) = outbound_interface() else {
        return Ok(lanrurugi_core::http_client::default_client());
    };
    let mut slot = client_slot().lock().expect("outbound client lock");
    if let Some((cached, client)) = slot.as_ref() {
        if cached == &name {
            return Ok(client.clone());
        }
    }
    let built = lanrurugi_core::http_client::build(lanrurugi_core::http_client::bind_outbound(
        reqwest::Client::builder()
            // A download is a long-lived stream: a dead peer otherwise keeps the socket half-open
            // indefinitely (the idle timeout in `stream.rs` covers the body, this covers the
            // handshake and the OS-level keepalive).
            .connect_timeout(std::time::Duration::from_secs(30))
            // Time spent waiting on the peer between reads, including the wait for response
            // headers: `connect_timeout` only covers the TCP/TLS handshake, so a server that
            // accepts and then says nothing would otherwise hold the transfer open forever. A
            // slow-but-alive download never trips this, because each read resets it.
            .read_timeout(std::time::Duration::from_secs(120))
            .tcp_keepalive(Some(std::time::Duration::from_secs(60))),
    ));
    // 022 US4: pinning is a promise, not a preference. Falling back to the unbound client here
    // would send a "download through the VPN" through the physical link — silently, and exactly
    // when the interface is misconfigured. Fail the download instead; the message names the field.
    let client = built.map_err(|e| DownloadError::Egress(name.clone(), e.to_string()))?;
    *slot = Some((name, client.clone()));
    Ok(client)
}
