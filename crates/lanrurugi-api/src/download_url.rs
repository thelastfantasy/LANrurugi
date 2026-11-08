//! Validation for the operator-configured download host (`direct_download_url`).
//!
//! The download host is the second, non-proxied host browser downloads are sent to (see
//! [`crate::direct_download`] and [`crate::download_host`]). A value that cannot work — an
//! `http://` host while the site is served over HTTPS, credentials in the URL — must be refused at
//! save time: the only symptom later is a download button the browser blocks, which reads as a
//! broken feature rather than a misconfiguration.

use url::Url;

/// The settings key this module validates.
pub const FIELD: &str = "direct_download_url";

/// Why a download address was refused. The reasons are distinct so the settings UI — and its
/// tests — can tell "the shape is wrong" from "the shape is fine but the deployment makes it
/// unusable".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadUrlError {
    /// Not an absolute `http`/`https` URL with a host.
    Shape,
    /// Carries a username or password — a credential in a URL is both a leak and a syntax the app
    /// never uses.
    HasCredentials,
    /// Carries a query or fragment, which links built from it would have to strip or carry along.
    HasQueryOrFragment,
    /// `http://` while the page origin is HTTPS: browsers treat the download as mixed content.
    SchemeMismatch,
}

impl DownloadUrlError {
    /// The message shown to the administrator.
    pub fn message(self) -> &'static str {
        match self {
            Self::Shape => {
                "the download address must be an absolute http:// or https:// URL with a host"
            }
            Self::HasCredentials => "the download address must not contain a username or password",
            Self::HasQueryOrFragment => "the download address must not contain a query or fragment",
            Self::SchemeMismatch => {
                "the download address must use https when the site itself is served over https"
            }
        }
    }
}

/// Normalizes `text` and checks it against the page's own scheme.
///
/// `page_origin` is the configured public URL of the *page* host (`share_public_url`), not the
/// request's scheme: the app usually sits behind a proxy that terminates TLS, so the request
/// arrives as plain HTTP even when the browser sees HTTPS. `None` means the page origin is not
/// configured yet, in which case only the shape checks apply.
///
/// An empty value is valid (the feature is off) and normalizes to an empty string; a trailing slash
/// is removed so links built from the base never contain a double slash.
pub fn validate(text: &str, page_origin: Option<&str>) -> Result<String, DownloadUrlError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(String::new());
    }
    let url = Url::parse(text).map_err(|_| DownloadUrlError::Shape)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return Err(DownloadUrlError::Shape);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(DownloadUrlError::HasCredentials);
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(DownloadUrlError::HasQueryOrFragment);
    }
    if url.scheme() == "http"
        && page_origin
            .is_some_and(|origin| origin.trim().to_ascii_lowercase().starts_with("https://"))
    {
        return Err(DownloadUrlError::SchemeMismatch);
    }
    Ok(text.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(text: &str, origin: Option<&str>) -> String {
        validate(text, origin).expect("expected a valid download address")
    }

    #[test]
    fn empty_means_the_feature_is_off() {
        assert_eq!(ok("", Some("https://lib.example.org")), "");
        assert_eq!(ok("   ", None), "");
    }

    #[test]
    fn https_is_accepted_and_trailing_slashes_are_removed() {
        assert_eq!(
            ok("https://fs.example.org/", Some("https://lib.example.org")),
            "https://fs.example.org"
        );
    }

    #[test]
    fn http_is_accepted_when_the_page_is_http_or_unknown() {
        assert_eq!(
            ok("http://192.168.1.10:8080", Some("http://192.168.1.10:8080")),
            "http://192.168.1.10:8080"
        );
        assert_eq!(ok("http://fs.lan", None), "http://fs.lan");
    }

    #[test]
    fn http_is_refused_when_the_page_is_https() {
        assert_eq!(
            validate("http://fs.example.org", Some("https://lib.example.org")),
            Err(DownloadUrlError::SchemeMismatch)
        );
        // Case and surrounding whitespace of the configured origin must not decide the check.
        assert_eq!(
            validate("http://fs.example.org", Some(" HTTPS://lib.example.org ")),
            Err(DownloadUrlError::SchemeMismatch)
        );
    }

    #[test]
    fn credentials_and_query_fragments_are_refused() {
        assert_eq!(
            validate("https://user:pass@fs.example.org", None),
            Err(DownloadUrlError::HasCredentials)
        );
        assert_eq!(
            validate("https://user@fs.example.org", None),
            Err(DownloadUrlError::HasCredentials)
        );
        assert_eq!(
            validate("https://fs.example.org/?a=1", None),
            Err(DownloadUrlError::HasQueryOrFragment)
        );
        assert_eq!(
            validate("https://fs.example.org/#x", None),
            Err(DownloadUrlError::HasQueryOrFragment)
        );
    }

    #[test]
    fn malformed_values_are_refused() {
        for text in [
            "fs.example.org",
            "ftp://fs.example.org",
            "//fs.example.org",
            "https://",
        ] {
            assert_eq!(validate(text, None), Err(DownloadUrlError::Shape), "{text}");
        }
    }
}
