pub mod permissions;
pub mod pool;
pub mod protocol;

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
