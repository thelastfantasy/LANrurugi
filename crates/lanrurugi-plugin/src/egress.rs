//! Locating the `LD_PRELOAD` shim that pins the dispatcher's sockets to one interface.
//!
//! The shim ([`lanrurugi_egress`](../../lanrurugi-egress)) ships as a `cdylib` next to the server
//! binary. Everything here is best-effort by design: when the library is absent the dispatcher
//! falls back to the host relay, which still enforces the declared hosts but cannot honour the
//! interface — so a missing file degrades behaviour rather than silently ignoring the setting.

use std::path::PathBuf;

/// Name the build produces (`cargo build -p lanrurugi-egress`).
pub const LIBRARY_NAME: &str = "liblanrurugi_egress.so";

/// The shim to preload, if this build/platform has one.
///
/// `LANRURUGI_EGRESS_LIB` overrides the search so a package that stores it elsewhere (or a test)
/// can point at it explicitly; otherwise the server's own directory is the only place looked at,
/// which is where the image and the native packages put it.
pub fn library() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("LANRURUGI_EGRESS_LIB") {
        let path = PathBuf::from(raw);
        return path.is_file().then_some(path);
    }
    if !cfg!(target_os = "linux") {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join(LIBRARY_NAME);
    path.is_file().then_some(path)
}

/// Prepends `library` to the `LD_PRELOAD` list the child would otherwise inherit.
pub fn preload_value(library: &std::path::Path, inherited: Option<&std::ffi::OsStr>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(existing) = inherited {
        parts.extend(
            existing
                .to_string_lossy()
                .split(':')
                .filter(|part| !part.is_empty())
                .map(str::to_string),
        );
    }
    let ours = library.display().to_string();
    if !parts.iter().any(|part| part == &ours) {
        parts.insert(0, ours);
    }
    parts.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preload_keeps_what_was_already_there() {
        let value = preload_value(
            std::path::Path::new("/usr/local/bin/liblanrurugi_egress.so"),
            Some(std::ffi::OsStr::new("/opt/other.so")),
        );
        assert_eq!(value, "/usr/local/bin/liblanrurugi_egress.so:/opt/other.so");
    }

    #[test]
    fn preload_does_not_repeat_itself() {
        let value = preload_value(
            std::path::Path::new("/shim.so"),
            Some(std::ffi::OsStr::new("/shim.so:/opt/a.so")),
        );
        assert_eq!(value, "/shim.so:/opt/a.so");
    }
}
