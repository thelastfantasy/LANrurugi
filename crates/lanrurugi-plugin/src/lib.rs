pub mod permissions;
pub mod pool;
pub mod protocol;

/// Embedded at compile time so the dispatcher script travels with the binary (no separate file to
/// ship/locate at runtime) — written out to a real file on disk at startup since Deno needs an
/// actual path to `run`, not stdin (stdin is reserved for the request/response protocol itself).
pub const DISPATCHER_SCRIPT: &str = include_str!("../dispatcher/dispatcher.ts");
