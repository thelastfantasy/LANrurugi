//! Permission-flag construction from a plugin's declared `plugin_info` (constitution Principle IV
//! / `contracts/plugin-protocol.md`): the host MUST NOT grant broader permissions than declared.
//!
//! `write` is scoped rather than absolute. A hash-layout archive lives at
//! `<library>/<bb>/<id>.zip` (020): delete or rename one file there and the catalogue record that
//! points at it is wrong with no way back — the id *is* the path. So the grant is
//! `--allow-write=<library>/_byname`, the tree where a filename is the user's own and a move is
//! still recoverable by the scanner. `script`-type plugins get no write grant at all: their job is
//! to return `updates`/`categories_to_create`, and the host applies them under its own locks.

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
pub fn build_flags(permissions: &DeclaredPermissions, may_write: bool) -> Vec<String> {
    let mut flags = Vec::new();
    if !permissions.net.is_empty() {
        // Deno accepts a comma-separated list; each entry is one declared host name.
        flags.push(format!("--allow-net={}", permissions.net.join(",")));
    }
    if permissions.write
        && may_write
        && let Some(library) = crate::library_dir()
    {
        let mut grant = library
            .join(lanrurugi_storage::paths::BYNAME_DIR)
            .display()
            .to_string();
        // A converted plugin may also write a scratch file on its way to building an archive.
        if let Some(scratch) = crate::scratch_dir() {
            grant.push(',');
            grant.push_str(&scratch.display().to_string());
        }
        flags.push(format!("--allow-write={grant}"));
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
        assert!(build_flags(&perms, true).is_empty());
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
            build_flags(&perms, true),
            vec!["--allow-net=api.example.com,cdn.example.com".to_string()]
        );
    }

    #[test]
    fn write_is_scoped_to_the_byname_tree_when_declared() {
        crate::set_library_dir("/library");
        let perms = DeclaredPermissions {
            net: vec![],
            read: false,
            write: true,
        };
        assert_eq!(
            build_flags(&perms, true),
            vec!["--allow-write=/library/_byname".to_string()]
        );
        // A `script`-type plugin computes and returns; the host does the writing.
        assert!(build_flags(&perms, false).is_empty());
    }

    #[test]
    fn read_is_not_produced_here_even_when_declared() {
        // See module docs: `read` is handled in `pool::Worker::spawn`, not here.
        let perms = DeclaredPermissions {
            net: vec![],
            read: true,
            write: false,
        };
        assert!(build_flags(&perms, true).is_empty());
    }
}
