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
