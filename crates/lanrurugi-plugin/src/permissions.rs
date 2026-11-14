//! Permission-flag construction from a plugin's declared `plugin_info` (constitution Principle IV
//! / `contracts/plugin-protocol.md`): the host MUST NOT grant broader permissions than declared.

use crate::protocol::DeclaredPermissions;

/// Builds the `net`/`write` Deno CLI flags for a subprocess allowed to run exactly the declared
/// permission set and nothing else. `net` is granted as the declared host list: the dispatcher
/// keeps Deno's native `fetch` whenever the egress shim is loaded (`LD_PRELOAD`, see
/// [`crate::pool::Worker::spawn`]), and Deno itself is then what enforces "only these hosts" —
/// while the shim makes every socket leave through the chosen interface. When the shim is *not*
/// loaded the dispatcher falls back to the host relay and revokes this grant, so the list is
/// enforced either way. `read` is handled separately by
/// [`crate::pool::Worker::spawn`]: the dispatcher mechanically needs to *read* the plugin's own
/// `.ts` file to `import()` it regardless of what the plugin declares, so that baseline need is
/// computed there (scoped to just the dispatcher + plugin file paths) rather than here, and is
/// only widened to unscoped `--allow-read` when the plugin actually declares needing broad read
/// access for its own logic.
pub fn build_flags(permissions: &DeclaredPermissions) -> Vec<String> {
    let mut flags = Vec::new();
    if !permissions.net.is_empty() {
        // Deno accepts a comma-separated list; each entry is one declared host name.
        flags.push(format!("--allow-net={}", permissions.net.join(",")));
    }
    if permissions.write {
        flags.push("--allow-write".to_string());
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_permissions_declared_means_no_flags() {
        let perms = DeclaredPermissions {
            net: vec![],
            read: false,
            write: false,
        };
        assert!(build_flags(&perms).is_empty());
    }

    #[test]
    fn declared_hosts_become_the_net_grant() {
        let perms = DeclaredPermissions {
            net: vec!["api.example.com".to_string(), "cdn.example.com".to_string()],
            read: false,
            write: false,
        };
        // Native `fetch` (shim loaded) needs the grant; the relay path revokes it at startup, so
        // the list is never absent from enforcement.
        assert_eq!(
            build_flags(&perms),
            vec!["--allow-net=api.example.com,cdn.example.com".to_string()]
        );
    }

    #[test]
    fn write_flag_only_appears_when_declared() {
        let perms = DeclaredPermissions {
            net: vec![],
            read: false,
            write: true,
        };
        assert_eq!(build_flags(&perms), vec!["--allow-write".to_string()]);
    }

    #[test]
    fn read_is_not_produced_here_even_when_declared() {
        // See module docs: `read` is handled in `pool::Worker::spawn`, not here.
        let perms = DeclaredPermissions {
            net: vec![],
            read: true,
            write: false,
        };
        assert!(build_flags(&perms).is_empty());
    }
}
