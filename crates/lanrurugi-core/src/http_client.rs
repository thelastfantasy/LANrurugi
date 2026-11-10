//! The one place outbound HTTPS clients are built. A host without system CA certificates (a
//! minimal container, VM or BSD install without `ca-certificates`) would otherwise make every
//! client fail to build — and the server panic at startup — over a dependency only network
//! features need. The system store is used whenever it has certificates; otherwise Mozilla's
//! root certificates bundled with the binary (`webpki-root-certs`) stand in.

use std::sync::OnceLock;

/// Where clients get their trusted root certificates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootSource {
    /// The operating system's certificate store.
    System,
    /// The bundled Mozilla roots: the system store had none.
    Bundled,
}

impl RootSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Bundled => "bundled",
        }
    }
}

/// Decided once per process by building a plain client against the system store.
pub fn root_source() -> RootSource {
    static SOURCE: OnceLock<RootSource> = OnceLock::new();
    *SOURCE.get_or_init(|| match reqwest::Client::builder().build() {
        Ok(_) => RootSource::System,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "no usable system CA certificates; HTTPS uses the bundled Mozilla roots \
                 (install ca-certificates to use the system's)"
            );
            RootSource::Bundled
        }
    })
}

fn bundled_roots() -> Vec<reqwest::Certificate> {
    webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .filter_map(|der| reqwest::Certificate::from_der(der.as_ref()).ok())
        .collect()
}

/// `builder` with this process's root certificates.
pub fn with_roots(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    match root_source() {
        RootSource::System => builder,
        RootSource::Bundled => builder.tls_certs_only(bundled_roots()),
    }
}

/// The network interface outbound plugin traffic must leave through, if one was chosen.
///
/// Process-wide rather than threaded through every call site: plugin requests are relayed from a
/// Deno subprocess through several layers, and a request has to leave the same interface whether it
/// is fetching a gallery page, logging in, or downloading the file itself — a site that refuses the
/// host's own address refuses the login too, not just the final download.
static OUTBOUND_INTERFACE: OnceLock<std::sync::Mutex<Option<String>>> = OnceLock::new();

fn outbound_slot() -> &'static std::sync::Mutex<Option<String>> {
    OUTBOUND_INTERFACE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Sets (or clears, with `None`/an empty name) the interface outbound plugin traffic is bound to.
/// Clients already built keep the interface they were built with; the call sites that care rebuild
/// against [`outbound_interface`].
pub fn set_outbound_interface(name: Option<String>) {
    let name = name.filter(|n| !n.trim().is_empty());
    *outbound_slot().lock().expect("outbound interface lock") = name;
}

pub fn outbound_interface() -> Option<String> {
    outbound_slot()
        .lock()
        .expect("outbound interface lock")
        .clone()
}

/// How much of the interface pinning this build can actually enforce.
///
/// The settings that ask for one are rejected up front where the answer is `None`, rather than
/// accepted and then silently ignored — a pinned interface that quietly does nothing is worse
/// than a refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BindingScope {
    /// Every outbound path is pinned, including plugin subprocesses.
    Full,
    /// Only the requests this process makes itself are pinned; a plugin subprocess's own `fetch`
    /// still takes the system route.
    #[serde(rename = "rust-only")]
    RustOnly,
    /// Nothing is pinned; the settings that ask for an interface are refused.
    None,
}

/// The OS this build runs on, spelled the way [`plugin_binding_scope_for`] expects.
pub fn binding_platform() -> &'static str {
    std::env::consts::OS
}

/// What the plugin-egress setting can enforce here.
pub fn plugin_binding_scope() -> BindingScope {
    plugin_binding_scope_for(std::env::consts::OS)
}

/// What the sharing-engine setting can enforce here.
pub fn share_binding_scope() -> BindingScope {
    share_binding_scope_for(std::env::consts::OS)
}

/// Plugin egress is pinned twice: reqwest's `.interface()` for our own requests, and the
/// `lanrurugi-egress` shim (`LD_PRELOAD`, `SO_BINDTODEVICE`) for the Deno subprocess. Only Linux has
/// both; macOS has the first, so a plugin's own `fetch` is unbounded there.
pub fn plugin_binding_scope_for(os: &str) -> BindingScope {
    match os {
        "linux" => BindingScope::Full,
        "android" | "fuchsia" | "illumos" | "ios" | "macos" | "solaris" | "tvos" | "visionos"
        | "watchos" => BindingScope::RustOnly,
        _ => BindingScope::None,
    }
}

/// Sharing traffic is pinned inside the two vendored crates (iroh's UDP sockets, the relay's TCP
/// connections), and that fork implements `SO_BINDTODEVICE` only: everywhere else the endpoint
/// always takes the system default route, so the setting is refused rather than silently ignored.
pub fn share_binding_scope_for(os: &str) -> BindingScope {
    match os {
        "linux" => BindingScope::Full,
        _ => BindingScope::None,
    }
}

/// Points `builder` at the interface the user pinned for outbound traffic (`share`/plugin egress),
/// when this platform can express that on a single socket. Only the requests this process makes
/// itself go through it; a plugin subprocess's own `fetch` cannot be steered this way on every
/// platform (see `plugin_binding_scope`).
pub fn bind_outbound(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    #[cfg(any(
        target_os = "android",
        target_os = "fuchsia",
        target_os = "illumos",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos",
        target_os = "solaris",
        target_os = "tvos",
        target_os = "visionos",
        target_os = "watchos",
    ))]
    {
        if let Some(name) = outbound_interface() {
            return builder.interface(&name);
        }
    }
    builder
}

/// Builds `builder` with this process's root certificates.
pub fn build(builder: reqwest::ClientBuilder) -> reqwest::Result<reqwest::Client> {
    with_roots(builder).build()
}

/// A client with no options, for call sites that only need defaults. Never panics over the
/// certificate store: if even the chosen roots fail, the bundled ones are used.
pub fn default_client() -> reqwest::Client {
    build(reqwest::Client::builder()).unwrap_or_else(|e| {
        tracing::error!(error = %e, "HTTPS client failed to build; retrying with the bundled roots");
        reqwest::Client::builder()
            .tls_certs_only(bundled_roots())
            .build()
            .expect("a client trusting only the bundled roots needs nothing from the system")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_roots_are_usable_certificates() {
        let roots = bundled_roots();
        assert!(roots.len() > 100, "got {}", roots.len());
        // A client trusting only them builds without touching the system store.
        assert!(reqwest::Client::builder()
            .tls_certs_only(roots)
            .build()
            .is_ok());
    }
}

#[cfg(test)]
mod scope_tests {
    use super::{plugin_binding_scope_for, share_binding_scope_for, BindingScope};

    #[test]
    fn only_linux_pins_every_plugin_request() {
        assert_eq!(plugin_binding_scope_for("linux"), BindingScope::Full);
        assert_eq!(plugin_binding_scope_for("macos"), BindingScope::RustOnly);
        assert_eq!(plugin_binding_scope_for("freebsd"), BindingScope::None);
        assert_eq!(plugin_binding_scope_for("windows"), BindingScope::None);
    }

    #[test]
    fn only_linux_pins_the_sharing_engine() {
        assert_eq!(share_binding_scope_for("linux"), BindingScope::Full);
        assert_eq!(share_binding_scope_for("macos"), BindingScope::None);
        assert_eq!(share_binding_scope_for("freebsd"), BindingScope::None);
        assert_eq!(share_binding_scope_for("windows"), BindingScope::None);
    }
}
