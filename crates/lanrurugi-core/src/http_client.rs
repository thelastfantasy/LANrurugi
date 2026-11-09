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

/// `builder` bound to the configured interface, when there is one. Binding is what makes an
/// interface choice a hard constraint rather than a preference: `SO_BINDTODEVICE` makes the kernel
/// pick this device's route (and fail when there is none) instead of silently falling back.
/// Whether this build can actually pin a socket to a named interface. The settings that ask for
/// one are rejected up front on platforms where the answer is no, rather than accepted and then
/// silently ignored — a pinned interface that quietly does nothing is worse than a refusal.
pub const fn interface_binding_supported() -> bool {
    cfg!(any(
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
    ))
}

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
