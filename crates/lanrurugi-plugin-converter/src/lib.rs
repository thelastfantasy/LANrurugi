//! Converts a legacy LANraragi Perl plugin (`.pm`) into a best-effort TypeScript plugin matching
//! `contracts/plugin-protocol.md`'s shape. Two independent passes:
//!
//! - `plugin_info` (metadata): a genuine recursive-descent parser over the hash-literal syntax
//!   (`perl_value.rs`/`metadata.rs`) — high confidence, since that block's grammar is small and
//!   completely static across the whole legacy plugin corpus.
//! - everything else (`ppi.rs`/`render.rs`): parsed by PPI (Perl's own AST library, invoked via a
//!   `perl` subprocess — see `Dockerfile.build`) and walked into TS. Structural correctness comes
//!   from PPI; semantic/vocabulary mapping (sigils, `->`, `eq`/`ne`, `push`/`join`/etc.) is this
//!   module's job — this is an assist tool, not a compiler, and its output (particularly any
//!   `// TODO(perl-convert)` marker) is never meant to be trusted unread.

pub mod metadata;
pub mod perl_value;
pub mod permissions;
pub mod ppi;
pub mod render;
pub mod strings;
pub mod web;

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("failed to read {0}: {1}")]
    Read(std::path::PathBuf, #[source] std::io::Error),
    #[error("no `sub plugin_info {{ ... }}` found — is this a real LANraragi plugin file?")]
    NoPluginInfo,
    #[error(transparent)]
    Ppi(#[from] ppi::PpiError),
}

pub struct ConversionOutput {
    pub ts: String,
    pub warnings: Vec<String>,
}

pub fn convert_file(path: &Path) -> Result<ConversionOutput, ConvertError> {
    let source =
        std::fs::read_to_string(path).map_err(|e| ConvertError::Read(path.to_path_buf(), e))?;
    convert_source_with_path(&source, path)
}

/// Converts Perl source text that isn't (necessarily) backed by a file on disk — e.g. pasted
/// into the `serve` mode's web form. PPI still needs a real file to read (it's a `perl`
/// subprocess, not a library linked into this binary), so this writes `source` to a temp file
/// first; callers with an existing on-disk file should use [`convert_file`] instead to avoid
/// that redundant copy.
///
/// The temp filename mixes the process ID with a per-process atomic counter (not just the
/// process ID alone, which every thread in this process shares) — real, confirmed-live bug this
/// fixes: two concurrent `convert_source` calls on different threads of the same process (e.g.
/// `cargo test`'s default multi-threaded test runner, which is exactly how this was caught) would
/// otherwise race on the exact same temp path, so one call's `remove_file` could delete the file
/// out from under the other call's still-in-flight `perl` subprocess read of it, or one call's
/// `write` could silently replace the source another concurrently-running call was mid-read on.
pub fn convert_source(source: &str) -> Result<ConversionOutput, ConvertError> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp_path = std::env::temp_dir().join(format!(
        "lanrurugi-plugin-converter-input-{}-{unique}.pm",
        std::process::id()
    ));
    std::fs::write(&temp_path, source).map_err(|e| ConvertError::Read(temp_path.clone(), e))?;
    let result = convert_source_with_path(source, &temp_path);
    let _ = std::fs::remove_file(&temp_path);
    result
}

fn convert_source_with_path(source: &str, path: &Path) -> Result<ConversionOutput, ConvertError> {
    let plugin_info_fields =
        perl_value::parse_plugin_info(source).ok_or(ConvertError::NoPluginInfo)?;
    let converted_metadata = metadata::convert(&plugin_info_fields, source);
    let metadata_ts = metadata::render(&converted_metadata);

    let top_level = ppi::parse_pm_file(path)?;
    let subs = render::find_subs(&top_level, &["plugin_info"]);

    let entry_point = render::entry_point_names(&converted_metadata.kind);
    let has_info_hash = render::entry_point_has_info_hash(&converted_metadata.kind);
    let first_param_name = converted_metadata
        .parameters
        .first()
        .map(|p| p.name.as_str());

    // Fixed-point iteration over which helper subs end up `async` (see `Renderer::known_async_subs`'s
    // own docs for why this can't be known in one pass): render every sub, note which ones used
    // `await` internally, then re-render everything with that knowledge so a caller of a newly-
    // async sub gets its own call site `await`-ed (which may in turn make *that* caller newly
    // async too, for callers further up the chain). Stops once a full pass adds no new async
    // *helper* sub — guaranteed to terminate since there are finitely many subs and the set only
    // ever grows.
    //
    // The entry point itself is never added to `known_async_subs` (it's the host-facing export,
    // never called by another sub in this same file — see `render_all_subs`'s own comment), so it
    // never triggers another loop iteration on its own account. But it's still a *caller* of
    // whatever helper subs the very last iteration just newly discovered are async, and that
    // last iteration rendered the entry point against the *previous* (smaller) known-async set —
    // so one final render past the fixed point, with the now-fully-settled set, is required to
    // get the entry point's own call sites `await`-ed correctly. Without this extra pass, a
    // helper sub that only becomes async on the very last iteration (e.g. because it calls
    // another helper that itself only became async that same iteration) would compile correctly
    // itself, but the entry point's call *to* it would still be missing its `await`.
    let imported_external_modules = render::collect_external_module_names(&top_level);
    let mut known_async_subs: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        let (_, _, async_this_pass) = render_all_subs(
            &subs,
            entry_point,
            first_param_name,
            has_info_hash,
            &converted_metadata.name,
            known_async_subs.clone(),
            imported_external_modules.clone(),
        );
        if async_this_pass.is_subset(&known_async_subs) {
            break;
        }
        known_async_subs.extend(async_this_pass);
    }
    let (mut renderer, sub_ts, _) = render_all_subs(
        &subs,
        entry_point,
        first_param_name,
        has_info_hash,
        &converted_metadata.name,
        known_async_subs,
        imported_external_modules,
    );

    // Package-scope `my $x = ...;`/`my %x = (...);` declarations — sitting directly in the file,
    // outside any `sub { ... }` block (real corpus cases: `Metadata/Chaika.pm`'s `my $chaika_url
    // = "https://panda.chaika.moe";`, `Metadata/EHDLInfo.pm`'s three `my $S_*` constants,
    // `Metadata/RegexParse.pm`'s `my $PLUGIN_TAG_NS`/`my %COMMON_EXTRANEOUS_VALUES`). `find_subs`
    // above only ever looks for `PPI::Statement::Sub` nodes, so these were previously dropped
    // entirely — every sub referencing one compiled to a reference to a name that was never
    // declared anywhere in the output (`Cannot find name '...'`, one of the more common causes of
    // a converted plugin failing `deno check`, not an inherent unsupported-Perl-construct
    // limitation the way an external module reference is). Rendered with the same
    // `render_variable_statement` a within-a-sub declaration uses, and placed before every sub in
    // the output so they're initialized at module-load time, before the host ever calls the
    // exported entry point.
    let top_level_vars: String = top_level
        .iter()
        .filter(|n| n.class == "PPI::Statement::Variable")
        .map(|n| renderer.render_variable_statement(n))
        .collect::<Vec<_>>()
        .join("\n");
    let sub_ts = if top_level_vars.is_empty() {
        sub_ts
    } else {
        format!("{top_level_vars}\n{sub_ts}")
    };

    // The host today passes exactly one generic `arg` string per call (see `render.rs`'s
    // `EntryContext` docs) — a plugin declaring more than one custom parameter can't be mapped
    // faithfully onto that: every parameter past the first ends up bound to the *same*
    // `hostArgs.arg` value in the generated code, which silently produces wrong behavior (all
    // those settings collapse together) rather than a loud error, so this needs calling out
    // explicitly rather than left for someone to discover at runtime.
    if converted_metadata.parameters.len() > 1 {
        renderer.warnings.push(format!(
            "this plugin declares {} custom parameters, but the host currently passes only one \
             generic `arg` value per call — only `{}` was mapped to it; the rest were also bound \
             to that same value and need manual attention before this plugin can work correctly",
            converted_metadata.parameters.len(),
            first_param_name.unwrap_or("?")
        ));
    }

    match entry_point {
        Some((legacy_name, _)) if !subs.iter().any(|f| f.name == legacy_name) => {
            renderer.warnings.push(format!(
                "expected mandatory sub `{legacy_name}` for plugin type `{}` but it wasn't found — \
                 this plugin has no exported entry point the host can actually call",
                converted_metadata.kind
            ));
        }
        None => {
            renderer.warnings.push(format!(
                "unrecognized plugin type `{}` — could not determine which sub to export as the \
                 host-facing entry point",
                converted_metadata.kind
            ));
        }
        _ => {}
    }

    let ts = format!("{metadata_ts}\n{sub_ts}");
    Ok(ConversionOutput {
        ts,
        warnings: renderer.warnings,
    })
}

/// One full rendering pass over every sub, given a `known_async_subs` set from a previous pass
/// (empty on the very first call). Returns the renderer (for its accumulated warnings), the
/// concatenated rendered TS, and the set of sub names *this pass*
/// discovered need `async` (checked via `Renderer::used_await` immediately after each individual
/// `render_sub`/`render_entry_sub` call, since that flag is reset and re-set per call) — the
/// caller (`convert_source_with_path`) feeds this back in as the next pass's `known_async_subs`
/// until the set stops growing. See `Renderer::known_async_subs`'s own docs for why this
/// fixed-point iteration is needed at all.
fn render_all_subs(
    subs: &[render::FoundSub],
    entry_point: Option<(&'static str, &'static str)>,
    first_param_name: Option<&str>,
    has_info_hash: bool,
    plugin_name: &str,
    known_async_subs: std::collections::HashSet<String>,
    imported_external_modules: std::collections::HashSet<String>,
) -> (render::Renderer, String, std::collections::HashSet<String>) {
    let mut renderer = render::Renderer::new();
    renderer.plugin_name = Some(plugin_name.to_string());
    renderer.known_async_subs = known_async_subs;
    renderer.imported_external_modules = imported_external_modules;
    let mut sub_ts = String::new();
    let mut async_this_pass = std::collections::HashSet::new();
    for found in subs {
        sub_ts.push('\n');
        match entry_point {
            Some((legacy_name, export_name)) if found.name == legacy_name => {
                sub_ts.push_str(&renderer.render_entry_sub(
                    found,
                    export_name,
                    first_param_name,
                    has_info_hash,
                ));
                // The entry point is always `async` regardless (see `render_entry_sub`'s own
                // docs) — not added to `async_this_pass` since it's never itself a callee another
                // sub in this file could call (it's the host-facing export, called only from
                // `dispatcher.ts`), so there's no `await`-the-call-site propagation to do for it.
            }
            _ => {
                sub_ts.push_str(&renderer.render_sub(found));
                if renderer.used_await {
                    async_this_pass.insert(found.name.clone());
                }
            }
        }
    }
    (renderer, sub_ts, async_this_pass)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression guard for issue #86: a freshly-converted plugin must never again carry an
    /// inline `declare global` block, bare `Perl*`/`perlCompat.*` boilerplate, or even a
    /// `/// <reference>` line — the shared ambient types now live once in
    /// `plugins/legacy-globals.d.ts`, a global (no-`import`/`export`) script that
    /// `plugins/tsconfig.json` makes ambiently visible to every `.ts` file under `plugins/` with
    /// zero per-file marker of any kind, so a future accidental reversion to the old inline-stub
    /// (or even the intermediate one-line-reference) approach fails this test immediately instead
    /// of silently reintroducing per-file boilerplate.
    #[test]
    fn a_converted_plugin_using_legacy_compat_gets_zero_boilerplate() {
        let src = r#"
            sub plugin_info {
                return ( name => "X", type => "metadata", namespace => "x" );
            }
            sub get_tags {
                my $logger = get_logger( "X", "plugins" );
                $logger->debug("hello");
                return ();
            }
        "#;
        let output = convert_source(src).expect("a minimal but real plugin must convert cleanly");

        assert!(
            !output.ts.contains("declare global"),
            "must not inline a declare global block anymore — got:\n{}",
            output.ts
        );
        assert!(
            !output.ts.contains("/// <reference"),
            "a plugin file must carry zero reference directives — ambient visibility comes from \
             plugins/tsconfig.json alone — got:\n{}",
            output.ts
        );
        assert!(
            !output.ts.contains("PerlUserAgent")
                && !output.ts.contains("PerlLogger")
                && !output.ts.contains("PerlDomNode")
                && !output.ts.contains("PerlTransaction")
                && !output.ts.contains("PerlHttpResult"),
            "must not reference any Perl*-prefixed type name anymore — got:\n{}",
            output.ts
        );
        assert!(
            !output.ts.contains("perlCompat."),
            "must call legacyCompat.*, not perlCompat.* — got:\n{}",
            output.ts
        );
        assert!(
            output.ts.contains("legacyCompat.getLogger("),
            "the generated call site itself must use the renamed global — got:\n{}",
            output.ts
        );
    }

    /// Same zero-boilerplate guarantee holds even for a plugin kind (`login`) whose entry point
    /// (`do_login`) receives no info hash at all (`entry_point_has_info_hash`), so never triggers
    /// `render_user_agent_hydration_preamble`'s own `legacyCompat.userAgent()` call — there's no
    /// per-plugin "does this one need the reference line" branch left to get wrong, since no
    /// plugin file gets one at all anymore (ambient visibility is a `plugins/tsconfig.json`-level
    /// property, not a per-file one).
    #[test]
    fn a_converted_login_plugin_also_gets_zero_boilerplate() {
        let src = r#"
            sub plugin_info {
                return ( name => "X", type => "login", namespace => "x" );
            }
            sub do_login {
                return ( cookies => [] );
            }
        "#;
        let output = convert_source(src).expect("a minimal but real plugin must convert cleanly");
        assert!(
            !output.ts.contains("declare global") && !output.ts.contains("/// <reference"),
            "got:\n{}",
            output.ts
        );
    }
}
