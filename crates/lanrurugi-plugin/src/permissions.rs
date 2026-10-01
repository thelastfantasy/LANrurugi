//! Permission-flag construction from a plugin's declared `plugin_info` (constitution Principle IV
//! / `contracts/plugin-protocol.md`): the host MUST NOT grant broader permissions than declared.

use crate::protocol::DeclaredPermissions;

/// Builds the `net`/`write` Deno CLI flags for a subprocess allowed to run exactly the declared
/// permission set and nothing else. `read` is handled separately by
/// [`crate::pool::Worker::spawn`]: the dispatcher mechanically needs to *read* the plugin's own
/// `.ts` file to `import()` it regardless of what the plugin declares, so that baseline need is
/// computed there (scoped to just the dispatcher + plugin file paths) rather than here, and is
/// only widened to unscoped `--allow-read` when the plugin actually declares needing broad read
/// access for its own logic.
pub fn build_flags(permissions: &DeclaredPermissions) -> Vec<String> {
    let mut flags = Vec::new();
    if !permissions.net.is_empty() {
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
    fn net_permission_lists_only_declared_hosts() {
        let perms = DeclaredPermissions {
            net: vec!["api.example.com".to_string(), "cdn.example.com".to_string()],
            read: false,
            write: false,
        };
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
