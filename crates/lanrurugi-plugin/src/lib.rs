pub mod egress;
pub mod http;
pub mod permissions;
pub mod pool;
pub mod protocol;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where the library lives, set once at startup (`lanrurugi-server`'s serve path) so a plugin's
/// write grant can be scoped to the by-name tree instead of the whole filesystem. Absent in tools
/// and tests that never spawn a plugin: a plugin then simply gets no write grant.
static LIBRARY_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_library_dir(dir: impl Into<PathBuf>) {
    let _ = LIBRARY_DIR.set(dir.into());
}

pub fn library_dir() -> Option<&'static Path> {
    LIBRARY_DIR.get().map(PathBuf::as_path)
}

/// The instance's temp directory, registered alongside the library: a converted plugin that writes
/// a scratch file (a downloaded page on its way into a zip) belongs there, not in the library.
static SCRATCH_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_scratch_dir(dir: impl Into<PathBuf>) {
    let _ = SCRATCH_DIR.set(dir.into());
}

pub fn scratch_dir() -> Option<&'static Path> {
    SCRATCH_DIR.get().map(PathBuf::as_path)
}

/// Embedded at compile time so the dispatcher script travels with the binary (no separate file to
/// ship/locate at runtime) — written out to a real file on disk at startup since Deno needs an
/// actual path to `run`, not stdin (stdin is reserved for the request/response protocol itself).
pub const DISPATCHER_SCRIPT: &str = include_str!("../dispatcher/dispatcher.ts");

/// Same embed-and-write-out treatment as [`DISPATCHER_SCRIPT`], written to the same directory
/// (see every `DISPATCHER_SCRIPT` write site's own sibling write of this constant) — this is what
/// lets `dispatcher.ts` and every plugin file `import { PluginErrorException } from
/// "file://<temp_dir>/plugin-sdk.ts"` (an absolute `file://` URL, since a plugin's own directory
/// under `plugins_dir` has no fixed relative path to `temp_dir`) as a real, single-source class
/// instead of each plugin file hand-copying its own definition (issue #86).
pub const PLUGIN_SDK_SCRIPT: &str = include_str!("../dispatcher/plugin-sdk.ts");
