//! Walks a real PPI parse tree (`ppi.rs`) and renders it to TypeScript. Structural correctness
//! (matching parens/braces, telling a `/regex/` apart from division, knowing exactly where one
//! statement ends and the next begins) comes for free from PPI having already resolved it — the
//! job left here is purely the Perl→JS *vocabulary* mapping (sigils, `->` vs `.`, `eq`/`ne`,
//! `push`/`join`/`split`, etc.), which is a much smaller and safer problem than re-deriving
//! Perl's grammar from text (see this crate's earlier regex-based `body.rs`, now superseded).
//!
//! The original source's own comments survive at their original positions (PPI keeps them as
//! ordinary sibling tokens, rendered as `//` lines right where they were) — nothing here
//! duplicates the whole sub as one extra comment block on top of that. Anything this renderer
//! doesn't specifically recognize is emitted inline as `/* TODO(perl-convert): <reconstructed
//! source> */` (via [`crate::ppi::PpiNode::source_text`]) rather than guessed at or dropped —
//! this is still fundamentally a best-effort assist tool, just one with a real parser under it
//! instead of text patterns.

use regex::Regex;

use crate::ppi::PpiNode;
use crate::strings::convert_string_content;
use crate::strings::FoundString;

/// One parameter from a modern Perl signature (`sub foo ($a, $b = 10, @rest) { ... }`,
/// `use feature 'signatures'` — ubiquitous in current legacy plugin source, e.g. every helper sub
/// in `~/LANraragi/lib/LANraragi/Plugin/Metadata/EHentai.pm`). Distinct from the older
/// `my ($a, $b) = @_;` destructuring idiom (`render_variable_statement` handles that one) — a
/// signature sub's parameter names are bound *before* the body ever runs, so they never appear as
/// a `my (...) = @_` statement inside the block at all; a converter that only looks for the
/// latter (as this one used to) sees these names referenced in the body but never declared
/// anywhere, silently emitting broken TS.
#[derive(Debug, Clone)]
pub struct SigParam {
    pub name: String,
    /// `@`/`%`-sigil'd trailing slurpy parameter (`@rest`/`%opts`) — becomes a TS rest parameter
    /// (`...rest: any[]`) rather than a plain named one. Perl only allows at most one, always last.
    pub slurpy: bool,
}

pub struct FoundSub<'a> {
    pub name: String,
    pub block: &'a PpiNode,
    /// Empty for a sub with no signature (an old-style `sub foo { my ($a, $b) = @_; ... }`, or a
    /// signature-less `sub foo { ... }` that reads `@_`/`shift` directly) — `render_sub` falls
    /// back to the pre-existing `(...args: any[])` shape in that case, unchanged from before.
    pub params: Vec<SigParam>,
}

/// Scans the document's top-level children for `sub NAME { ... }` statements, excluding any
/// named in `exclude` (namely `plugin_info`, handled separately by `metadata.rs`).
pub fn find_subs<'a>(top_level: &'a [PpiNode], exclude: &[&str]) -> Vec<FoundSub<'a>> {
    let mut found = Vec::new();
    for node in top_level {
        if node.class != "PPI::Statement::Sub" {
            continue;
        }
        let mut name = None;
        let mut block = None;
        let mut signature = None;
        let mut prototype = None;
        let mut seen_sub_kw = false;
        for child in node.children() {
            match child.class.as_str() {
                "PPI::Token::Word" if !seen_sub_kw && child.content.as_deref() == Some("sub") => {
                    seen_sub_kw = true;
                }
                "PPI::Token::Word" if seen_sub_kw && name.is_none() => {
                    name = child.content.clone();
                }
                "PPI::Structure::Signature" => signature = Some(child),
                // PPI's own quirk (verified against real `perl -MPPI::Dumper` output on actual
                // legacy plugin `.pm` files, not just hand-written test snippets): a modern
                // `sub foo ($a, $b) {...}` signature is *not reliably* tokenized as the
                // structured `PPI::Structure::Signature` above — PPI often instead emits it as a
                // single opaque `PPI::Token::Prototype` string token (the same class it uses for
                // old-style `sub foo ($$) {...}` prototypes, which really are just an opaque
                // type-arity string with no variable names at all). Every real sub signature
                // across the legacy plugin corpus came back this way, not as a `Structure`, so
                // both must be handled or every signature-using sub silently gets the
                // `(...args: any[])` fallback with its body still referencing names that were
                // never declared (the exact bug this whole `params` mechanism exists to fix).
                "PPI::Token::Prototype" => prototype = Some(child),
                "PPI::Structure::Block" => block = Some(child),
                _ => {}
            }
        }
        let (Some(name), Some(block)) = (name, block) else {
            continue;
        };
        if exclude.contains(&name.as_str()) {
            continue;
        }
        let params = signature
            .map(parse_signature_params)
            .or_else(|| prototype.map(parse_prototype_params))
            .unwrap_or_default();
        found.push(FoundSub {
            name,
            block,
            params,
        });
    }
    found
}

/// Scans the document's top-level `use Foo;` / `use Foo qw(...);` statements (`PPI::Statement::
/// Include`) and returns every externally-referenced module name — i.e. excluding Perl's own
/// pragmas (`strict`/`warnings`/`utf8`/`feature`/a bare version number like `v5.36`, none of
/// which name a real module a plugin body could reference as a bareword) and this project's own
/// `LANraragi::*` internal namespace (already routed through the pre-existing `::`-in-name
/// detection in `render_token`/`try_match_call`, which — unlike this scan — also catches a
/// package-qualified reference that was never `use`d by its short name at all, e.g. a fully
/// spelled-out `LANraragi::Utils::Database::get_tags(...)` call with no corresponding `use`
/// line). Feeds `Renderer::imported_external_modules`, which lets `render_token` warn about a
/// single-segment external module name (`URI`, no `::` in it at all) the same way it already
/// does for a `Foo::Bar`-shaped one — see that field's own docs for the real corpus bug
/// (`Download/EHentai.pm`'s `use URI;` + `URI->new(...)`) this exists to catch.
pub fn collect_external_module_names(top_level: &[PpiNode]) -> std::collections::HashSet<String> {
    const PRAGMAS: &[&str] = &[
        "strict", "warnings", "utf8", "feature", "parent", "base", "lib",
    ];
    let mut names = std::collections::HashSet::new();
    for node in top_level {
        if node.class != "PPI::Statement::Include" {
            continue;
        }
        let module_name = node
            .children()
            .iter()
            .find(|c| c.class == "PPI::Token::Word" && c.content.as_deref() != Some("use"))
            .and_then(|c| c.content.as_deref());
        let Some(module_name) = module_name else {
            continue; // e.g. `use v5.36;` — a `PPI::Token::Number::Version`, not a module name.
        };
        if PRAGMAS.contains(&module_name) || module_name.starts_with("LANraragi::") {
            continue;
        }
        names.insert(module_name.to_string());
    }
    names
}

/// Scans an entry sub's body for every static-key access on its info-hash parameter (`$name->
/// {url}`, `$name->{"user_agent"}`, ...) and returns the set of keys found — feeds
/// `render_entry_sub`'s decision to emit a real, narrow interface for that parameter instead of
/// the blanket `Record<string, any>` escape hatch (see that call site's own docs for why `any`
/// was the pre-existing default: Perl hashes are freely-extensible and the converter can't
/// otherwise know what shape a given hash has). Returns `None` — meaning "give up, fall back to
/// `any`" — the moment a *dynamic* key access is found (`$name->{$some_var}`): a real key wasn't
/// spelled out in the source at all in that case, so there is no complete, safe key list this
/// scan could ever produce, and emitting a narrow interface anyway would silently reject a
/// legitimate access the source actually performs.
///
/// Recurses into every child unconditionally (`if`/`while`/nested blocks, string-interpolated
/// subscripts, etc. — a real corpus access is not guaranteed to sit at the sub's top level) rather
/// than only scanning direct statement children, since a hash access can appear arbitrarily deep.
fn collect_static_hash_keys(block: &PpiNode, var_name: &str) -> Option<Vec<String>> {
    let mut keys = Vec::new();
    if !collect_static_hash_keys_into(block, var_name, &mut keys) {
        return None;
    }
    // Stable, de-duplicated order (first-seen) rather than whatever `HashSet` iteration would
    // give — keeps the generated interface's field order deterministic across runs, and matches
    // the order a reader scanning the source top-to-bottom would expect.
    let mut seen = std::collections::HashSet::new();
    keys.retain(|k: &String| seen.insert(k.clone()));
    Some(keys)
}

/// `true` on success (every subscript found on `var_name` was a static key, collected into `out`);
/// `false` the moment a dynamic one is found (caller must then discard everything and fall back
/// to `any` — see `collect_static_hash_keys`'s own docs).
fn collect_static_hash_keys_into(node: &PpiNode, var_name: &str, out: &mut Vec<String>) -> bool {
    let children = node.children();
    let mut i = 0;
    while i < children.len() {
        let child = &children[i];
        let is_target_symbol = child.class == "PPI::Token::Symbol"
            && child
                .content
                .as_deref()
                .is_some_and(|c| strip_sigil(c) == var_name);
        if is_target_symbol {
            // `$name->{key}` (explicit arrow) or `$name{key}` (arrow-less sugar, PPI still emits
            // a `Structure::Subscript` immediately after the symbol either way) — look past an
            // optional `->` for the subscript.
            let mut j = i + 1;
            if children.get(j).is_some_and(|t| {
                t.class == "PPI::Token::Operator" && t.content.as_deref() == Some("->")
            }) {
                j += 1;
            }
            if let Some(subscript) = children.get(j) {
                if subscript.class == "PPI::Structure::Subscript" {
                    let inner: Vec<&PpiNode> = subscript
                        .children()
                        .iter()
                        .flat_map(|c| {
                            if c.class == "PPI::Statement"
                                || c.class == "PPI::Statement::Expression"
                            {
                                c.children().iter().collect::<Vec<_>>()
                            } else {
                                vec![c]
                            }
                        })
                        .filter(|c| c.class != "PPI::Token::Whitespace")
                        .collect();
                    match inner.as_slice() {
                        [tok] if tok.class == "PPI::Token::Word" => {
                            out.push(tok.content.clone().unwrap_or_default());
                        }
                        [tok]
                            if tok.class == "PPI::Token::Quote::Single"
                                || tok.class == "PPI::Token::Quote::Double" =>
                        {
                            let quote = if tok.class == "PPI::Token::Quote::Single" {
                                '\''
                            } else {
                                '"'
                            };
                            out.push(strip_quotes(tok.content.as_deref().unwrap_or(""), quote));
                        }
                        _ => return false, // dynamic key ($var, an expression, ...) — bail out entirely.
                    }
                }
            }
        }
        if !collect_static_hash_keys_into(child, var_name, out) {
            return false;
        }
        i += 1;
    }
    true
}

/// Finds the entry sub's own info-hash variable name. Two real corpus shapes bind it (matching
/// the two branches `render_variable_statement`/`render_entry_destructure` already handle):
///
/// - `my $lrr_info = shift;` — a lone symbol shifted off `@_` one at a time.
/// - `my ($lrr_info, %params) = @_;` — destructured directly, always with the info hash first
///   (real corpus case: `Download/EHentai.pm`'s `provide_url`, preceded by a discarded `shift;`
///   for the invocant). The name itself is never assumed in either shape — only its position.
///
/// Only looks at the block's direct top-level statements (not nested inside an `if`/loop/etc.)
/// since this binding is always the very first thing an entry sub does in every real corpus file
/// — a matching statement found deeper in the body would be a different variable entirely, not
/// this one.
fn find_info_hash_var_name(block: &PpiNode) -> Option<String> {
    for child in block.children() {
        if child.class != "PPI::Statement::Variable" {
            continue;
        }
        // Excludes both whitespace and the statement's own trailing `;` (a `PPI::Token::
        // Structure`) — without this, every real statement here has one token more than the
        // patterns below expect (verified via real `perl -MPPI::Dumper` output: `my $lrr_info =
        // shift;` is 5 tokens including the semicolon, not the 4 an earlier version of this
        // function assumed, which meant neither pattern below ever matched anything at all).
        let tokens: Vec<&PpiNode> = child
            .children()
            .iter()
            .filter(|t| t.class != "PPI::Token::Whitespace" && t.class != "PPI::Token::Structure")
            .collect();
        match tokens.as_slice() {
            [my_kw, symbol, op, shift_kw]
                if is_word(my_kw, "my")
                    && symbol.class == "PPI::Token::Symbol"
                    && is_operator(op, "=")
                    && is_word(shift_kw, "shift") =>
            {
                return Some(strip_sigil(symbol.content.as_deref().unwrap_or("")));
            }
            [my_kw, list, op, magic]
                if is_word(my_kw, "my")
                    && list.class == "PPI::Structure::List"
                    && is_operator(op, "=")
                    && magic.class == "PPI::Token::Magic"
                    && magic.content.as_deref() == Some("@_") =>
            {
                let first_symbol = list
                    .children()
                    .iter()
                    .flat_map(|c| {
                        if c.class == "PPI::Statement" || c.class == "PPI::Statement::Expression" {
                            c.children().iter().collect::<Vec<_>>()
                        } else {
                            vec![c]
                        }
                    })
                    .find(|t| t.class == "PPI::Token::Symbol");
                if let Some(symbol) = first_symbol {
                    return Some(strip_sigil(symbol.content.as_deref().unwrap_or("")));
                }
            }
            _ => {}
        }
    }
    None
}

/// Deterministic, collision-free interface name for a given entry sub's info-hash parameter —
/// derived from the export name (`execDownload` → `ExecDownloadInfo`) rather than a fixed
/// constant, since a file could in principle (not seen in the real corpus, but not ruled out
/// either) end up needing more than one of these if this scanning were ever extended to helper
/// subs too.
fn info_hash_interface_name(export_name: &str) -> String {
    let mut chars = export_name.chars();
    let capitalized = match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    };
    format!("{capitalized}Info")
}

/// Maps an entry point's export name to the authoritative `plugin-sdk.ts` interface describing
/// its real `hostArgs` shape — `execMetadata` → `MetadataHostArgs`, `execDownload` →
/// `DownloadHostArgs`, `runScript` → `ScriptHostArgs`. `execLogin`/`LoginHostArgs` never reaches
/// [`render_info_hash_interface`] at all (`render_entry_sub` is always called with
/// `has_info_hash: false` for it — a login plugin's own hash has no per-archive fields worth
/// generating a narrowed interface for), so this only needs to cover the three that do.
fn authoritative_host_args_interface(export_name: &str) -> &'static str {
    match export_name {
        "execDownload" => "DownloadHostArgs",
        "runScript" => "ScriptHostArgs",
        _ => "MetadataHostArgs", // execMetadata, and any future entry point sharing its shape.
    }
}

/// Renders the `interface NAME { ... }` declaration for a generated info-hash parameter type —
/// always includes `user_agent`/`user_agent_cookies` (see `render_user_agent_hydration_preamble`'s
/// own docs: the host unconditionally injects both into the info hash before the plugin's own
/// code ever runs, regardless of whether the plugin's own source happens to read them back), plus
/// every key `collect_static_hash_keys` found actually being read.
///
/// Field types are `Required<Pick<AuthoritativeInterface, "key1" | "key2" | ...>>` against the
/// real `plugin-sdk.ts`/`plugins/legacy-globals.d.ts` interface (`MetadataHostArgs`/
/// `DownloadHostArgs`/`ScriptHostArgs` — see [`authoritative_host_args_interface`]) rather than
/// each field independently guessed as `string` — the old approach was a real, live-verified
/// source of drift: `customargs` is actually `string[]`, not `string`, and every converted plugin
/// that reads it needed a hand-fix to correct the guess (`plugins/metadata/ehentai.ts`'s own
/// `customargs: string[]` is exactly that manual correction, made necessary by this exact bug —
/// issue #86). `Pick` also means a key `collect_static_hash_keys` found that *isn't* a real field
/// on the authoritative interface (a typo, or a name that doesn't actually exist in the host
/// contract) fails `deno check` instead of silently type-checking as `string` — a real, useful
/// signal this generator didn't have before.
///
/// `Required<...>` (not bare `Pick<...>`) matters because most authoritative fields are
/// genuinely optional (`arg?: string`, `existing_tags?: string`, ...) — the host may not always
/// supply them. But every converted plugin's own generated body reads these fields unconditionally
/// (`lrr_info["existing_tags"].match(...)`, no `?.`/undefined check — a faithful translation of
/// Perl's own `$lrr_info->{existing_tags}`, which is simply `undef` rather than a type error if
/// absent), so a bare `Pick` here would make `deno check` correctly flag every one of those reads
/// as "possibly undefined" — a real regression this exact live-verified case caught (confirmed via
/// a real re-conversion of `EHentai.pm`: `TS2532` on `lrr_info["oneshot_param"].match(...)`).
/// `Required` matches what the *old*, independently-guessed-`string`-per-field approach always
/// implicitly assumed (every field non-optional), while still deriving the actual value type from
/// the authoritative interface instead of a blind `string` guess.
fn render_info_hash_interface(iface_name: &str, keys: &[String], export_name: &str) -> String {
    let authoritative = authoritative_host_args_interface(export_name);
    let picked: Vec<&String> = keys
        .iter()
        .filter(|key| key.as_str() != "user_agent" && key.as_str() != "user_agent_cookies")
        .collect();
    let mut out = if picked.is_empty() {
        format!("  interface {iface_name} {{\n")
    } else {
        let pick_keys = picked
            .iter()
            .map(|key| format!("\"{key}\""))
            .collect::<Vec<_>>()
            .join(" | ");
        format!(
            "  interface {iface_name} extends Required<Pick<{authoritative}, {pick_keys}>> {{\n"
        )
    };
    out.push_str("    user_agent: LegacyUserAgent;\n");
    out.push_str("    user_agent_cookies?: LegacyCookie[];\n");
    out.push_str("  }\n");
    out
}

/// Extracts parameter names from a `PPI::Structure::Signature` node — its children are a single
/// `PPI::Statement::Expression` wrapping a comma-separated run of `PPI::Token::Symbol`s, each
/// optionally followed by `= <default expr>` (verified against real `perl -MPPI::Dumper` output
/// on `sub f ($a, $b = 10, @rest) {...}`). Default-value expressions are intentionally dropped —
/// TS parameter defaults would need them re-rendered as TS expressions, and every current
/// call site host-side already supplies every argument, so a default is never actually exercised.
fn parse_signature_params(signature: &PpiNode) -> Vec<SigParam> {
    let inner: Vec<&PpiNode> = signature
        .children()
        .iter()
        .flat_map(|c| {
            if c.class == "PPI::Statement::Expression" {
                c.children().iter().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .filter(|c| c.class != "PPI::Token::Whitespace")
        .collect();

    let mut params = Vec::new();
    for group in split_on_commas(&inner) {
        let Some(symbol) = group.iter().find(|n| n.class == "PPI::Token::Symbol") else {
            continue;
        };
        let raw = symbol.content.as_deref().unwrap_or("");
        let sigil = raw.chars().next();
        params.push(SigParam {
            name: strip_sigil(raw),
            slurpy: matches!(sigil, Some('@') | Some('%')),
        });
    }
    params
}

/// Extracts parameter names from a `PPI::Token::Prototype`'s raw content string (e.g.
/// `"( $title, $tags, @rest )"`) — PPI hands this whole thing over as one opaque token, no
/// sub-structure at all, so this is plain string scanning rather than tree-walking. Distinguishes
/// a genuine modern signature (has `$name`/`@name`/`%name` — real identifiers) from an old-style
/// prototype (`($$;@)` — bare sigils/punctuation, no names) by requiring every comma-separated
/// entry to actually contain an identifier; a real prototype's entries never do, so it falls
/// through to an empty `Vec` (the pre-existing `(...args: any[])` fallback in `render_sub`) rather
/// than misreading punctuation as parameter names.
fn parse_prototype_params(prototype: &PpiNode) -> Vec<SigParam> {
    let raw = prototype.content.as_deref().unwrap_or("");
    let inner = raw.trim().trim_start_matches('(').trim_end_matches(')');
    if inner.trim().is_empty() {
        return Vec::new();
    }
    let mut params = Vec::new();
    for entry in inner.split(',') {
        let entry = entry.trim();
        // Take the entry up to (not including) any `= default` — only the sigil+name matters.
        let name_part = entry.split('=').next().unwrap_or(entry).trim();
        let Some(sigil) = name_part.chars().next() else {
            return Vec::new();
        };
        if !matches!(sigil, '$' | '@' | '%') {
            return Vec::new();
        }
        let name: String = name_part
            .chars()
            .skip(1)
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        // A real prototype entry is bare sigils/punctuation (`$$`, `;@`, `\%`) with no identifier
        // following — an empty name here means this wasn't a real signature after all.
        if name.is_empty() {
            return Vec::new();
        }
        params.push(SigParam {
            name,
            slurpy: matches!(sigil, '@' | '%'),
        });
    }
    params
}

/// The one legacy sub name every plugin of a given `plugin_info` `type` is required to define
/// (`~/LANraragi/lib/LANraragi/Utils/Plugins.pm`), and the export name `dispatcher.ts` calls it
/// under instead (`exec_metadata`/`exec_login`/`exec_download`/`exec_script` → `mod.execMetadata`/
/// `mod.execLogin`/`mod.execDownload`/`mod.runScript`). Returns `None` for a `type` this converter
/// doesn't recognize, in which case the caller can't tell which found sub (if any) is the entry
/// point.
pub fn entry_point_names(kind: &str) -> Option<(&'static str, &'static str)> {
    match kind {
        "metadata" => Some(("get_tags", "execMetadata")),
        "login" => Some(("do_login", "execLogin")),
        "download" => Some(("provide_url", "execDownload")),
        "script" => Some(("run_script", "runScript")),
        _ => None,
    }
}

/// Whether a plugin `type`'s mandatory entry-point sub receives an info-hash (legacy's
/// `$lrr_info`, `\%infohash`) as its first argument at all. Verified against every call site in
/// `~/LANraragi/lib/LANraragi/Model/Plugins.pm`: `get_tags`/`provide_url`/`run_script` are all
/// called as `$plugin->METHOD( \%infohash, @{ $args{customargs} } )` (or the `\%args` hashref
/// variant) — info hash first, then custom args — but `do_login` is the one exception, called as
/// `$loginplugin->do_login( @{ $loginargs{customargs} } )` with **no** info hash at all, just the
/// plugin's own custom parameters. Feeds [`EntryContext::info_hash_bound`]'s starting value so
/// `do_login`'s first destructured/shifted name doesn't get wrongly bound to the whole `hostArgs`
/// object the way `get_tags`'s genuinely does.
pub fn entry_point_has_info_hash(kind: &str) -> bool {
    kind != "login"
}

/// Tracks the mandatory entry-point sub's own parameter binding while it's being rendered — see
/// [`Renderer::render_entry_sub`]. Every *other* sub in the file is an internal helper only ever
/// called from already-converted TS, so it keeps the existing `...args: any[]` (mimicking Perl's
/// untyped `@_`) treatment; only the one sub the host itself calls receives the host's real single-
/// object calling convention (`crates/lanrurugi-api/src/plugins.rs::use_plugin_sync`'s
/// `{archive_id, arg, file_path}`, not the richer `parameters: {...}` shape
/// `contracts/plugin-protocol.md` describes but the host doesn't actually send yet).
struct EntryContext {
    /// The plugin's own first declared custom parameter name (`plugin_info`'s `parameters`), used
    /// to give the one generic `hostArgs.arg` value a meaningful field name when the legacy sub
    /// destructures it into a hash (`my ($lrr_info, %params) = @_;`). `None` if the plugin
    /// declares no custom parameters, or declares more than the host can actually carry today.
    first_param_name: Option<String>,
    /// Whether the info-hash slot (legacy's `$lrr_info`) has already been bound to `hostArgs` —
    /// every subsequent shift/@_-derived binding is instead the plugin's one custom value
    /// (`hostArgs.arg`).
    info_hash_bound: bool,
    /// The name + field list of a real interface generated for this sub's info-hash parameter
    /// (see `render_entry_sub`'s own docs on why/how this gets computed), used in place of the
    /// blanket `Record<string, any>` when every key access on that parameter was statically
    /// known. `None` when no such interface could be generated (a dynamic key access was found,
    /// or the sub doesn't bind an info hash at all), in which case the pre-existing `Record<string,
    /// any>` fallback is used unchanged.
    info_hash_interface: Option<(String, Vec<String>)>,
}

pub struct Renderer {
    shift_cursor: usize,
    entry: Option<EntryContext>,
    /// Set whenever the `legacyCompat.userAgent()`-returned object's `get`/`post` (the only
    /// genuinely async operations this converter recognizes) get awaited (see
    /// `render_expr_sequence`'s `-> result` lookahead) — a helper sub that ends up needing
    /// `await` must be declared `async function`, unlike the fire-and-forget default every other
    /// helper sub keeps. Reset at the start of each [`Self::render_sub`]/
    /// [`Self::render_entry_sub`] call; the entry point is always `async` regardless (network
    /// calls are the norm there), so this only actually changes anything for helper subs.
    pub used_await: bool,
    /// Set whenever a Perl `=~ /pattern/` regex match gets rendered (see
    /// `render_expr_sequence`'s `=~` handling) — the rendered code assigns into a `match`
    /// variable (so later `$1`/`$2` references, mapped by `render_magic` to `match[1]`/
    /// `match[2]`, resolve correctly), which must be declared once at the top of the enclosing
    /// function rather than at every assignment site (JS doesn't auto-vivify a bare `match = ...`
    /// the way Perl's `my`-less regex match binds implicitly). Reset at the start of each
    /// `render_sub`/`render_entry_sub` call, same lifecycle as `used_await`.
    uses_regex_match: bool,
    /// The plugin's own declared display name (`plugin_info`'s `name`) — needed to resolve
    /// `get_plugin_logger()`'s zero-argument legacy call, which at runtime uses Perl's `caller`
    /// reflection plus the plugin's own `plugin_info` to fill in what `get_logger($name,
    /// "plugins")` needs; there's no equivalent reflection in the converted TS, so this is
    /// resolved at *conversion* time instead, using the same metadata this converter already
    /// parsed. Set once by `lib.rs::convert_source_with_path` before rendering any subs.
    pub plugin_name: Option<String>,
    pub warnings: Vec<String>,
    /// Names of helper subs (in *this same file*) already known to compile to an `async function`
    /// — used by `render_named_call` to `await` a call to one of them. Perl has no `async`/`await`
    /// distinction at all (every call, however it resolves, just blocks), so nothing in the
    /// source itself marks a call site this way; whether a given sub needs to be `async` in JS
    /// only becomes knowable *after* rendering it once (does its body call `await`-needing things
    /// like `->get()`/`->post()`/`sleep()`?) — and that "needs async" status can itself cascade
    /// through further callers (`lib.rs::convert_source_with_path` re-renders every sub against a
    /// growing `known_async_subs` set until it stops changing, a fixed-point iteration, since
    /// e.g. sub A calling newly-async sub B might make A itself newly async for sub C's next
    /// pass). Empty on the first pass, by construction — nothing can be "already known" yet.
    pub known_async_subs: std::collections::HashSet<String>,
    /// Module names collected from this file's own `use Foo;` statements (`lib.rs::
    /// convert_source_with_path` populates this before rendering any subs) — excludes Perl
    /// pragmas (`strict`/`warnings`/`utf8`/`feature`/bare version numbers) and this project's own
    /// `LANraragi::*` internal namespace (routed through the existing `::`-in-name detection in
    /// `render_token`/`try_match_call` instead, which also catches package-qualified references
    /// to those modules that were never `use`d by name in the first place).
    ///
    /// Exists because the pre-existing "external module, no JS equivalent" detection only ever
    /// triggered for a name containing `::` (`Mojo::UserAgent`, `Data::Dumper::Dumper`, ...) — a
    /// single-segment module name like `URI` (real corpus case: `Download/EHentai.pm`'s `use
    /// URI;` + `URI->new(...)`) has no `::` in it at all, so it fell through every existing check
    /// silently and rendered as a bare, undeclared `URI.new()` reference — a real `deno check`
    /// failure (`Cannot find name 'URI'`) with zero warning ever surfaced about it, unlike every
    /// other external-module case in the corpus.
    pub imported_external_modules: std::collections::HashSet<String>,
    /// Names of variables assigned a `URI->new(...)`/`URI->new()` result (see
    /// `try_match_legacy_http_constructor`'s own docs on the `URI`→`URL` mapping) — real corpus
    /// values are `URL | undefined`, not `string`, so a later comparison against `""` (Perl's
    /// `$var eq ""`, verified real corpus case: `Download/EHentai.pm`'s `if ($@ || $finalURL eq
    /// "")`) needs its `""` rewritten to `undefined` or `deno check` rejects the comparison
    /// outright (`URL | undefined` and `string` have no overlap at all, unlike Perl, which is
    /// perfectly happy comparing an object reference against a string — always `false`, same as
    /// the rewritten `undefined` comparison, so this preserves that pre-existing behavior rather
    /// than changing it). Populated by `render_variable_statement`/`render_expr_sequence`'s
    /// assignment-rendering paths whenever their RHS rendering used the `URI`→`URL` mapping;
    /// consulted by `render_expr_sequence`'s own `EXPR eq ""` window match.
    uri_typed_vars: std::collections::HashSet<String>,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            shift_cursor: 0,
            entry: None,
            used_await: false,
            uses_regex_match: false,
            plugin_name: None,
            warnings: Vec::new(),
            known_async_subs: std::collections::HashSet::new(),
            imported_external_modules: std::collections::HashSet::new(),
            uri_typed_vars: std::collections::HashSet::new(),
        }
    }

    pub fn render_sub(&mut self, found: &FoundSub) -> String {
        // Deliberately no "here's the original Perl in a comment block" dump above the
        // function — the original source's own comments already survive at their original
        // positions (see `render_statement_list`'s `PPI::Token::Comment` handling below), so a
        // full second copy of the whole sub as one more comment block on top of that was pure
        // duplication, not a safety net.
        self.shift_cursor = 0;
        self.used_await = false;
        self.uses_regex_match = false;
        let body = self.render_statement_list(found.block.children());
        let body = self.with_match_decl(body);
        let keyword = if self.used_await {
            "async function"
        } else {
            "function"
        };

        // A modern Perl signature (`sub foo ($a, $b) { ... }`) binds its parameter names before
        // the body runs — the names are never destructured from `@_` inside the block at all, so
        // they must become real named TS parameters here, or every reference to them in the
        // (already-rendered, unchanged) body above is left pointing at a variable that was never
        // declared. `any`, not `unknown`, for the same reason as the `@_`-less fallback below:
        // there was never a real static type on these values upstream, and `unknown` would force
        // a cast at every one of the many string/property-access sites they routinely flow into.
        if !found.params.is_empty() {
            let params_list = self.render_signature_params(&found.params);
            return format!("{keyword} {}({params_list}) {{\n{body}}}\n", found.name);
        }

        // `any[]`, not `unknown[]`: Perl's `@_` carries no type information at all, and values
        // pulled out of it routinely flow into string/property contexts a few lines later (e.g.
        // a value destructured here gets handed straight to a `Mojo::Cookie::Response`-shaped
        // object literal elsewhere) — `unknown` would force a cast at every one of those sites for
        // no real safety benefit, since there was never a real type to check against upstream.
        format!("{keyword} {}(...args: any[]) {{\n{body}}}\n", found.name)
    }

    /// Prepends a `let match: RegExpMatchArray | null;` declaration to `body` if this sub's
    /// rendering used one (see `uses_regex_match`'s docs) — a bare `match = expr.match(...)` with
    /// no prior declaration is a JS `ReferenceError` (unlike Perl, where a regex match's captures
    /// bind implicitly with no declaration needed at all).
    fn with_match_decl(&self, body: String) -> String {
        if self.uses_regex_match {
            format!("  let match: RegExpMatchArray | null;\n{body}")
        } else {
            body
        }
    }

    /// Renders a signature's parameters as a TS parameter list — a trailing slurpy (`@rest`/
    /// `%opts`) becomes a rest parameter (`...rest: any[]`), everything before it a plain named
    /// `name: any`. Perl only allows one slurpy parameter and it's always last, so no special
    /// splicing is needed beyond checking the final entry.
    fn render_signature_params(&self, params: &[SigParam]) -> String {
        params
            .iter()
            .map(|p| {
                if p.slurpy {
                    format!("...{}: any[]", p.name)
                } else {
                    format!("{}: any", p.name)
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Renders the plugin's mandatory host-facing entry point — same body-rendering machinery as
    /// [`Self::render_sub`], but exported under `export_name` and with its parameter binding
    /// rewritten to match the host's real calling convention (a single `hostArgs` object, not a
    /// spread positional array): see `EntryContext`'s docs and `render_variable_statement`'s
    /// `self.entry`-gated branches for the actual rewrite.
    pub fn render_entry_sub(
        &mut self,
        found: &FoundSub,
        export_name: &str,
        first_param_name: Option<&str>,
        has_info_hash: bool,
    ) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "export async function {export_name}(hostArgs: Record<string, unknown>) {{\n"
        ));
        if has_info_hash {
            out.push_str(&self.render_user_agent_hydration_preamble());
        }
        self.shift_cursor = 0;
        self.used_await = false;
        self.uses_regex_match = false;
        // See `EntryContext::info_hash_interface`'s own docs. Computed *before* rendering the
        // body (rather than discovered as a side effect of it, the way `used_await`/
        // `uses_regex_match` are) because the interface declaration itself — if one can be
        // generated — needs to be emitted ahead of the function that references it, and because
        // `render_variable_statement`'s `hostArgs as Record<string, any>` call site needs to know
        // the answer at the exact moment it renders that line, not after the fact.
        let info_hash_interface = if has_info_hash {
            find_info_hash_var_name(found.block).and_then(|var_name| {
                collect_static_hash_keys(found.block, &var_name)
                    .map(|keys| (info_hash_interface_name(export_name), keys))
            })
        } else {
            None
        };
        if let Some((iface_name, keys)) = &info_hash_interface {
            out.push_str(&render_info_hash_interface(iface_name, keys, export_name));
        }
        self.entry = Some(EntryContext {
            first_param_name: first_param_name.map(str::to_string),
            info_hash_bound: !has_info_hash,
            info_hash_interface,
        });
        let body = self.render_statement_list(found.block.children());
        out.push_str(&self.with_match_decl(body));
        out.push_str("}\n");
        self.entry = None;
        out
    }

    /// Legacy always bundles a `user_agent` key into the info hash it passes to
    /// `get_tags`/`provide_url`/`run_script` — fresh-logged-in via that plugin's own declared
    /// `login_from` if it has one, or a blank `Mojo::UserAgent->new` otherwise (see
    /// `~/LANraragi/lib/LANraragi/Model/Plugins.pm`'s `exec_login_plugin`, called unconditionally
    /// before every one of those three). The host mirrors this by running the `login_from`
    /// plugin fresh itself and folding the resulting cookies into `hostArgs.user_agent_cookies`
    /// (a plain, JSON-serializable array — a live `Mojo::UserAgent`-equivalent object can't cross
    /// the dispatcher's JSON-RPC boundary) — this preamble rehydrates that back into a real,
    /// usable `legacyCompat.userAgent()` instance *before* any of the plugin's own converted
    /// code runs, so `lrr_info["user_agent"]` (whatever the plugin itself calls its info-hash
    /// variable — this doesn't know or care) resolves correctly no matter which shift/destructure
    /// idiom that plugin's own code happens to use to bind it. Scoped in its own block so the
    /// `info` alias it introduces can never collide with a name the plugin's own code picks.
    fn render_user_agent_hydration_preamble(&mut self) -> String {
        "  {\n    \
         const info = hostArgs as Record<string, any>;\n    \
         info.user_agent = legacyCompat.userAgent();\n    \
         for (const c of (info.user_agent_cookies ?? []) as { name: string; value: string; domain: string; path: string }[]) {\n      \
         info.user_agent.cookie_jar.add(c);\n    \
         }\n    \
         const headers = (info.user_agent_headers ?? {}) as Record<string, string>;\n    \
         if (Object.keys(headers).length > 0) {\n      \
         info.user_agent.on(\"start\", (_ua: any, tx: any) => {\n        \
         for (const [name, value] of Object.entries(headers)) tx.req.headers.header(name, value);\n      \
         });\n    \
         }\n  \
         }\n"
            .to_string()
    }

    fn real_children<'a>(&self, nodes: &'a [PpiNode]) -> Vec<&'a PpiNode> {
        nodes
            .iter()
            .filter(|n| n.class != "PPI::Token::Whitespace")
            .collect()
    }

    fn render_statement_list(&mut self, nodes: &[PpiNode]) -> String {
        let mut out = String::new();
        let real: Vec<&PpiNode> = nodes
            .iter()
            .filter(|n| n.class != "PPI::Token::Whitespace")
            .collect();
        let mut i = 0;
        while i < real.len() {
            let node = real[i];
            if node.class == "PPI::Token::Comment" {
                out.push_str("  //");
                out.push_str(node.content.as_deref().unwrap_or(""));
                out.push('\n');
                i += 1;
                continue;
            }
            // `open my $fh, '>'|'>>', PATH or die ...; print $fh CONTENT; close $fh;` — Perl's
            // file-*write* idiom (the mirror image of `try_render_open_file_statement`'s file-
            // *read* one), always exactly these three consecutive statements in the real corpus
            // (`Download/Pixiv.pm`'s `image_res_to_zip`: writes a downloaded image to a temp path
            // before handing it to `Archive::Zip`). Unlike the read-file idiom (where the host
            // pre-resolves `PATH_EXPR` to file *content* before the plugin ever runs, so "opening"
            // it is just a variable read), a write genuinely needs a real filesystem call here —
            // `declared_permissions.write` is a real, honored permission
            // (`crates/lanrurugi-plugin/src/permissions.rs` turns it into a real `--allow-write`
            // Deno flag), so this maps onto a real `Deno.writeTextFile(...)` call rather than
            // anything host-resolved. Checked as a 3-statement window (not per-statement, unlike
            // every other `try_render_*` in this renderer) since the write only has anywhere to
            // put its content once the `print` statement is known too.
            if let Some((rendered, consumed)) = self.try_render_write_file_statements(&real[i..]) {
                for line in rendered.lines() {
                    out.push_str("  ");
                    out.push_str(line);
                    out.push('\n');
                }
                i += consumed;
                continue;
            }
            let rendered = self.render_statement(node);
            if !rendered.trim().is_empty() {
                for line in rendered.lines() {
                    out.push_str("  ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
            i += 1;
        }
        out
    }

    fn render_statement(&mut self, node: &PpiNode) -> String {
        match node.class.as_str() {
            "PPI::Statement::Variable" => self.render_variable_statement(node),
            "PPI::Statement::Compound" => self.render_compound_statement(node),
            "PPI::Statement::Break" => self.render_break_statement(node),
            "PPI::Statement::Package" | "PPI::Statement::Include" => String::new(),
            "PPI::Statement" | "PPI::Statement::Expression" => {
                if let Some(rendered) = self.try_render_bare_shift(node) {
                    return rendered;
                }
                if let Some(rendered) = self.try_render_open_file_statement(node) {
                    return rendered;
                }
                if let Some(rendered) = self.try_render_eval_block(node) {
                    return rendered;
                }
                // Must run before `try_render_postfix_modifier`/`try_render_list_assignment`
                // below — see `try_render_modern_try_catch`'s own docs for why a `try`/`catch`
                // this shape (`use experimental 'try'`, not `use feature 'try'`) lands as a plain
                // `PPI::Statement` here instead of the `PPI::Statement::Compound` branch.
                let real_children = self.real_children(node.children());
                if let Some(rendered) = self.try_render_modern_try_catch(&real_children) {
                    return rendered;
                }
                if let Some(rendered) = self.try_render_postfix_modifier(node) {
                    return rendered;
                }
                if let Some(rendered) = self.try_render_list_assignment(node) {
                    return rendered;
                }
                // `$x = URI->new(...)|URI->new;` (no `my`) — tracks whether this reassigns a
                // variable `try_match_legacy_http_constructor`'s `URI`→`URL` mapping already
                // applies to (see `uri_typed_vars`'s own docs for why this needs tracking at all:
                // a later `$x eq ""` comparison needs its `""` rewritten to `undefined` once `$x`'s
                // real type is `URL | undefined`, not `string`). Checked as a *pre-rendering*
                // token-shape match (not by rendering the RHS and inspecting the result) since
                // `render_expr_sequence` has side effects (`used_await`, `uses_regex_match`, etc.)
                // that must fire exactly once per real render — calling it here just to peek at
                // its output, then calling it again via `render_children_expr` below for the real
                // render, would double-apply all of them.
                if real_children.len() >= 2
                    && real_children[0].class == "PPI::Token::Symbol"
                    && is_operator(real_children[1], "=")
                    && real_children.get(2).map(|t| t.content.as_deref()) == Some(Some("URI"))
                    && real_children
                        .get(3)
                        .map(|t| is_operator(t, "->"))
                        .unwrap_or(false)
                    && real_children
                        .get(4)
                        .map(|t| is_word(t, "new"))
                        .unwrap_or(false)
                {
                    let name = strip_sigil(real_children[0].content.as_deref().unwrap_or(""));
                    self.uri_typed_vars.insert(name);
                }
                let expr = self.render_children_expr(node);
                if expr.trim().is_empty() {
                    String::new()
                } else if expr.trim_end().ends_with(';') {
                    expr
                } else {
                    format!("{expr};")
                }
            }
            _ => {
                self.warnings
                    .push(format!("unhandled statement class: {}", node.class));
                format!("/* TODO(perl-convert): {} */", node.source_text())
            }
        }
    }

    /// Bare `shift;` with no `my`/assignment — discards one positional arg (usually the OOP
    /// invocant/`$self`, or one of `@_`'s own slots). Shared between [`Self::render_variable_statement`]
    /// (in case PPI ever hands this a `PPI::Statement::Variable`) and [`Self::try_render_bare_shift`]
    /// (the class it actually uses in practice — see that method's docs).
    fn bare_shift_comment_if_matches(&mut self, children: &[&PpiNode]) -> Option<String> {
        let non_structure: Vec<&&PpiNode> = children
            .iter()
            .filter(|c| c.class != "PPI::Token::Structure")
            .collect();
        if non_structure.len() == 1 && is_word(non_structure[0], "shift") {
            self.shift_cursor += 1;
            Some(
                "// (shift) discarded positional arg — legacy Perl-OOP invocant/first @_ slot"
                    .to_string(),
            )
        } else {
            None
        }
    }

    /// Bare `shift;` (no `my`) is tokenized by PPI as a plain `PPI::Statement`, *not*
    /// `PPI::Statement::Variable` — unlike every other shift/`@_`-destructure idiom this renderer
    /// recognizes, which all start with `my` and really do get that class. Must be checked before
    /// the generic expression fallback in `render_statement`'s `PPI::Statement` branch, since a
    /// lone `shift` word would otherwise fall through to `render_word`'s generic
    /// `"shift" => "args.shift()"` mapping — which throws at runtime for the mandatory entry
    /// point, whose real parameter is a non-array `hostArgs` object, not `args`.
    fn try_render_bare_shift(&mut self, node: &PpiNode) -> Option<String> {
        let children = self.real_children(node.children());
        self.bare_shift_comment_if_matches(&children)
    }

    /// `my $x = EXPR;` / `my ($a, $b) = @_;` / `my $x = shift;` / bare `shift;`.
    /// `pub(crate)`, not private: also called directly from `lib.rs` to render top-level
    /// (package-scope, outside any `sub`) `my $x = ...;`/`my %x = (...);` declarations — see that
    /// call site's own docs for why those need the exact same rendering as an in-sub declaration.
    pub(crate) fn render_variable_statement(&mut self, node: &PpiNode) -> String {
        let children = self.real_children(node.children());

        if let Some(rendered) = self.bare_shift_comment_if_matches(&children) {
            return rendered;
        }

        let Some(my_idx) = children.iter().position(|c| is_word(c, "my")) else {
            return self.render_children_expr(node);
        };
        let Some(eq_idx) = children.iter().position(|c| is_operator(c, "=")) else {
            // Bare `my ($a, $b, $c);` — a *multi*-variable declaration with no initializer (real
            // corpus case: `Metadata/RegexParse.pm`'s `my ( $title, $trailing_tags,
            // $other_captures );`). Checked before the single-symbol case below since this shape
            // wraps its symbols in a `Structure::List` rather than exposing exactly one bare
            // `PPI::Token::Symbol` as `my`'s immediate next child.
            if let Some(list) = children.get(my_idx + 1) {
                if list.class == "PPI::Structure::List" {
                    let symbols = symbols_with_sigils_in(list);
                    if !symbols.is_empty() {
                        let decls: Vec<String> = symbols
                            .iter()
                            .map(|(sigil, name)| {
                                let init = match sigil {
                                    '@' => "[] as any[]",
                                    '%' => "{} as Record<string, any>",
                                    _ => "undefined",
                                };
                                format!("let {name} = {init};")
                            })
                            .collect();
                        return decls.join("\n");
                    }
                }
            }
            // Bare `my @arr;` / `my %hash;` / `my $x;` — a declaration with no initializer.
            // Perl auto-vivifies the empty container on first use either way, so initializing it
            // explicitly here (rather than leaving it `undefined`) keeps later `.push()`/property
            // access on it from throwing in JS, where there's no equivalent auto-vivification.
            if let Some(symbol) = children
                .get(my_idx + 1..)
                .and_then(|rest| rest.iter().find(|c| c.class == "PPI::Token::Symbol"))
            {
                let raw = symbol.content.as_deref().unwrap_or("");
                let name = strip_sigil(raw);
                // `as any[]`/`as Record<string, any>`, not just a bare `[]`/`{}` literal: Perl
                // carries no static element type at all, and TS's strict mode otherwise infers
                // `never[]` from an empty array literal with no other context, rejecting every
                // later `.push(...)` call on it (real corpus case: `Metadata/Hentag.pm`'s `my
                // @found_tags;`, later filled via `.push()` in a completely separate statement TS
                // can't unify with the declaration site).
                let init = match raw.chars().next() {
                    Some('@') => "[] as any[]",
                    Some('%') => "{} as Record<string, any>",
                    _ => "undefined",
                };
                return format!("let {name} = {init};");
            }
            return self.render_children_expr(node);
        };
        let lhs = &children[my_idx + 1..eq_idx];
        // Excludes the statement's terminal `;` (a `PPI::Token::Structure`) — every rendering
        // path below builds its own trailing `;` via `format!`, so keeping it here would double it.
        let rhs_no_semi: Vec<&PpiNode> = children[eq_idx + 1..]
            .iter()
            .copied()
            .filter(|n| n.class != "PPI::Token::Structure")
            .collect();

        // `my $x = shift;`
        if lhs.len() == 1 && lhs[0].class == "PPI::Token::Symbol" {
            let raw_lhs = lhs[0].content.as_deref().unwrap_or("");
            let name = strip_sigil(raw_lhs);
            if rhs_no_semi.len() == 1 && is_word(rhs_no_semi[0], "shift") {
                if let Some(ctx) = &mut self.entry {
                    if !ctx.info_hash_bound {
                        ctx.info_hash_bound = true;
                        return match &ctx.info_hash_interface {
                            Some((iface_name, _)) => {
                                format!("let {name} = hostArgs as unknown as {iface_name};")
                            }
                            None => format!("let {name} = hostArgs as Record<string, any>;"),
                        };
                    }
                    return format!("let {name} = hostArgs.arg as string;");
                }
                let idx = self.shift_cursor;
                self.shift_cursor += 1;
                return format!("let {name} = args[{idx}];");
            }
            // `my @arr = (a, b, c);` / `my %h = (k => v, ...);` — an array/hash-sigil'd LHS
            // being assigned a parenthesized list. Perl's `(...)` here is a genuine list, not a
            // grouping expression, so it must become a `[...]` array (or `{...}` object) literal
            // — rendering it through the generic path below would produce plain parens, which JS
            // reads as the *comma operator* (keeps only the last value) instead of a list. A
            // scalar-sigil'd LHS doesn't need this: Perl's own `my $x = (1, 2, 3);` *also*
            // keeps only the last element (list-in-scalar-context), so the generic path's literal
            // parens happen to already match Perl's real behavior there.
            if rhs_no_semi.len() == 1 && rhs_no_semi[0].class == "PPI::Structure::List" {
                let sigil = raw_lhs.chars().next();
                if sigil == Some('@') {
                    let inner = self.render_children_expr(rhs_no_semi[0]);
                    return format!("let {name} = [{inner}];");
                }
                if sigil == Some('%') {
                    let flat = self.flatten_statement_children(rhs_no_semi[0].children());
                    if let Some(fields) = self.render_key_value_pairs(&flat) {
                        // `: Record<string, any>`, not TS's inferred object-literal shape (just
                        // `{ tags: any }` here) — Perl hashes are freely-extensible at any later
                        // point (`$hashdata{title} = $x;` after the fact, verified as the real
                        // idiom this corpus uses this exact declaration shape for), which a bare
                        // object-literal-inferred type doesn't allow assigning a new key onto.
                        return format!(
                            "let {name}: Record<string, any> = {{ {} }};",
                            fields.join(", ")
                        );
                    }
                }
            }
            let expr = self.render_expr_sequence(&rhs_no_semi);
            self.track_uri_typed_var(&name, &expr);
            return format!("let {name} = {expr};");
        }

        // `my ($a, $b) = @_;`
        if lhs.len() == 1 && lhs[0].class == "PPI::Structure::List" {
            let is_args = rhs_no_semi.len() == 1
                && rhs_no_semi[0].class == "PPI::Token::Magic"
                && rhs_no_semi[0].content.as_deref() == Some("@_");
            // `my ($lrr_info, %params) = @_;` — the legacy entry point's single-statement,
            // mixed-sigil form (as opposed to the two-separate-`shift`s form the branch above
            // handles): still needs the same `hostArgs`-binding rewrite, but per-symbol, since a
            // `%`-sigil'd name here means "the plugin's custom args as a hash", not another plain
            // positional slot.
            if is_args && self.entry.is_some() {
                let symbols = symbols_with_sigils_in(lhs[0]);
                if !symbols.is_empty() {
                    return self.render_entry_destructure(&symbols);
                }
            }
            let names = symbol_names_in(lhs[0]);
            if is_args && !names.is_empty() {
                let start = self.shift_cursor;
                self.shift_cursor += names.len();
                return format!("let [{}] = args.slice({start});", names.join(", "));
            }
            let expr = self.render_expr_sequence(&rhs_no_semi);
            return format!("let [{}] = {};", names.join(", "), expr);
        }

        // Generic fallback: render both sides best-effort.
        let lhs_expr = self.render_expr_sequence(lhs);
        let rhs_expr = self.render_expr_sequence(&rhs_no_semi);
        format!("let {lhs_expr} = {rhs_expr};")
    }

    /// Modern `try { BLOCK } catch ($e) { BLOCK }` (`use feature 'try'` / `use experimental
    /// 'try'` — distinct from the older `eval { BLOCK }` idiom `try_render_eval_block` already
    /// handles). `children` is a flat `Word("try") Block Word("catch") List($e) Block` sequence
    /// either way, but which *statement class wraps that sequence* depends entirely on which
    /// pragma spelling the source used to enable the feature — verified against real
    /// `perl -MPPI::Dumper` output on actual legacy plugin files, not just hand-written test
    /// snippets: `use feature 'try'` produces a `PPI::Statement::Compound` (handled by
    /// `render_compound_statement`), but `use experimental 'try'` — the spelling every real
    /// plugin in the corpus actually uses — produces a plain `PPI::Statement` instead (handled in
    /// `render_statement`'s `PPI::Statement`/`PPI::Statement::Expression` branch). Before this
    /// fix covered both, `try`/`catch` fell through with no matching arm in whichever branch it
    /// landed in, so the *entire* try/catch (both blocks — real logic, not just boilerplate) was
    /// silently dropped from the output with no warning. JS's own `try`/`catch (e)` maps onto
    /// this directly; no translation needed beyond grabbing each block and the caught variable's
    /// name.
    fn try_render_modern_try_catch(&mut self, children: &[&PpiNode]) -> Option<String> {
        if !is_word(children.first()?, "try") {
            return None;
        }
        let try_block = self.find_next(children, 0, "PPI::Structure::Block")?;
        let catch_idx = children.iter().position(|c| is_word(c, "catch"))?;
        let catch_var = self
            .find_next(children, catch_idx, "PPI::Structure::List")
            .map(symbol_names_in)
            .filter(|names| !names.is_empty())
            .map(|names| names.join(", "))
            .unwrap_or_else(|| "caughtError".to_string());
        let catch_block = self.find_next(children, catch_idx, "PPI::Structure::Block");

        // `: any`, not TS's default `catch` binding type of `unknown` — legacy Perl code always
        // treats a caught error as a plain string (`$logger->error($e)`, `die $e`, string
        // interpolation), and `unknown` would force an explicit narrowing/cast at every one of
        // those sites (none of which this converter's rendering of the catch body actually adds),
        // producing type errors on otherwise-correct-looking generated code.
        let mut out = String::from("try {\n");
        out.push_str(&self.render_statement_list(try_block.children()));
        out.push_str(&format!("}} catch ({catch_var}: any) {{\n"));
        if let Some(block) = catch_block {
            out.push_str(&self.render_statement_list(block.children()));
        }
        out.push('}');
        Some(out)
    }

    /// `if (COND) { } elsif (COND2) { } else { }`, `unless (COND) { }`,
    /// `foreach my $x (LIST) { }` / `for my $x (LIST) { }`, `while (COND) { }`.
    fn render_compound_statement(&mut self, node: &PpiNode) -> String {
        let children = self.real_children(node.children());
        let Some(first_word) = children.first().and_then(|c| c.content.as_deref()) else {
            return format!("/* TODO(perl-convert): {} */", node.source_text());
        };

        if first_word == "foreach" || first_word == "for" {
            return self.render_foreach(&children);
        }

        // Modern `try { BLOCK } catch ($e) { BLOCK }` — see `try_render_modern_try_catch`'s own
        // docs for why this can show up here (as a `PPI::Statement::Compound`) as well as in the
        // plain-`PPI::Statement` branch below, depending on which pragma spelling the source used
        // to enable the feature.
        if let Some(rendered) = self.try_render_modern_try_catch(&children) {
            return rendered;
        }

        // if/unless/elsif/else/while — a flat sequence of Word, Condition, Block, [Word,
        // Condition, Block]*, [Word, Block]?
        let mut out = String::new();
        let mut i = 0;
        while i < children.len() {
            let Some(word) = children[i].content.as_deref() else {
                i += 1;
                continue;
            };
            match word {
                "if" | "while" | "until" => {
                    let cond = self.find_next(&children, i, "PPI::Structure::Condition");
                    let block = self.find_next(&children, i, "PPI::Structure::Block");
                    // `while (my $row = <$fh>) { BODY }` — Perl's line-by-line file-read loop
                    // (see `try_render_open_file_statement`'s own docs for the paired `open(...)`
                    // half of this idiom). Under this host's design `$fh` is already the whole
                    // file's content as one string, not a real handle, so this becomes real
                    // line-by-line iteration over that string split on newlines — preserving
                    // per-line semantics exactly (rather than collapsing straight to the whole
                    // content) matters for a plugin like `Metadata/EHDLInfo.pm` that parses each
                    // line differently, not just concatenating them.
                    if word == "while" {
                        if let Some(rendered) = self.try_render_readline_while(cond, block) {
                            out.push_str(&rendered);
                            i += 1;
                            continue;
                        }
                        // `while (my ($k, $v) = each %hash) { BODY }` — see
                        // `try_render_each_hash_while`'s own docs.
                        if let Some(rendered) = self.try_render_each_hash_while(cond, block) {
                            out.push_str(&rendered);
                            i += 1;
                            continue;
                        }
                    }
                    let cond_str = cond
                        .map(|c| self.render_children_expr(c))
                        .unwrap_or_default();
                    let js_kw = if word == "until" {
                        "while (!("
                    } else {
                        "while ("
                    };
                    let (open, close) = if word == "if" {
                        ("if (".to_string(), ")".to_string())
                    } else {
                        (
                            js_kw.to_string(),
                            if word == "until" {
                                "))".to_string()
                            } else {
                                ")".to_string()
                            },
                        )
                    };
                    out.push_str(&open);
                    out.push_str(&cond_str);
                    out.push_str(&close);
                    out.push_str(" {\n");
                    if let Some(block) = block {
                        out.push_str(&self.render_statement_list(block.children()));
                    }
                    out.push('}');
                    i += 1;
                }
                "unless" => {
                    let cond = self.find_next(&children, i, "PPI::Structure::Condition");
                    let block = self.find_next(&children, i, "PPI::Structure::Block");
                    let cond_str = cond
                        .map(|c| self.render_children_expr(c))
                        .unwrap_or_default();
                    out.push_str(&format!("if (!({cond_str})) {{\n"));
                    if let Some(block) = block {
                        out.push_str(&self.render_statement_list(block.children()));
                    }
                    out.push('}');
                    i += 1;
                }
                "elsif" => {
                    let cond = self.find_next(&children, i, "PPI::Structure::Condition");
                    let block = self.find_next(&children, i, "PPI::Structure::Block");
                    let cond_str = cond
                        .map(|c| self.render_children_expr(c))
                        .unwrap_or_default();
                    out.push_str(&format!(" else if ({cond_str}) {{\n"));
                    if let Some(block) = block {
                        out.push_str(&self.render_statement_list(block.children()));
                    }
                    out.push('}');
                    i += 1;
                }
                "else" => {
                    let block = self.find_next(&children, i, "PPI::Structure::Block");
                    out.push_str(" else {\n");
                    if let Some(block) = block {
                        out.push_str(&self.render_statement_list(block.children()));
                    }
                    out.push('}');
                    i += 1;
                }
                _ => i += 1,
            }
        }
        out
    }

    fn find_next<'a>(
        &self,
        children: &[&'a PpiNode],
        from: usize,
        class: &str,
    ) -> Option<&'a PpiNode> {
        children[from..].iter().find(|c| c.class == class).copied()
    }

    fn render_foreach(&mut self, children: &[&PpiNode]) -> String {
        let loop_var = children
            .iter()
            .find(|c| c.class == "PPI::Token::Symbol")
            .map(|c| strip_sigil(c.content.as_deref().unwrap_or("")));
        let list = children.iter().find(|c| c.class == "PPI::Structure::List");
        let block = children.iter().find(|c| c.class == "PPI::Structure::Block");

        // `foreach my $i (START .. END) { ... }` — Perl's range operator, tokenized (verified via
        // `perl -MPPI::Dumper`) as a `PPI::Structure::List` wrapping exactly one `PPI::Statement`
        // of the shape `START Operator("..") END` (`END` is very often a `PPI::Token::ArrayIndex`
        // like `$#pages`, which `render_children_expr` already turns into `(name.length - 1)` —
        // real corpus case: `Download/Pixiv.pm`'s `for (0 .. $#pages)`). JS has no range literal,
        // so this can't go through the generic `for...of` template below at all — left to fall
        // through, `render_operator`'s catch-all emits `..` completely unchanged (it's not one of
        // that function's mapped operators), producing `for (let i of 0 .. (pages.length - 1))`,
        // which isn't valid JS/TS syntax and fails even to parse. Rendered instead as a classic
        // counting loop, the only faithful JS equivalent of an inclusive integer range.
        if let Some(range) = list.and_then(|l| self.try_render_range_list(l)) {
            let (start_expr, end_expr) = range;
            let mut out = match &loop_var {
                Some(var) => {
                    format!("for (let {var} = {start_expr}; {var} <= {end_expr}; {var}++) {{\n")
                }
                None => format!("for (let it = {start_expr}; it <= {end_expr}; it++) {{\n"),
            };
            if let Some(block) = block {
                out.push_str(&self.render_statement_list(block.children()));
            }
            out.push('}');
            return out;
        }

        let list_expr = list
            .map(|l| self.render_children_expr(l))
            .unwrap_or_default();
        // `let`, not `const`: Perl's `foreach my $x (@list) { $x =~ s/.../ /; ... }` routinely
        // reassigns the loop variable itself within the body (real corpus case:
        // `Metadata/HDoujin.pm`'s `$tag =~ s/^\s+|\s+$//g;` inside its tag-cleanup loop) — Perl's
        // own foreach variable is actually a live *alias* into the source list (mutating it
        // mutates the original array too), which `let` doesn't replicate, but no real corpus loop
        // depends on that; every one only uses the reassigned value locally within the same
        // iteration afterward, which `let` handles correctly and `const` can't compile at all.
        let mut out = match loop_var {
            Some(var) => format!("for (let {var} of {list_expr}) {{\n"),
            None => format!("for (const it of {list_expr}) {{\n"),
        };
        if let Some(block) = block {
            out.push_str(&self.render_statement_list(block.children()));
        }
        out.push('}');
        out
    }

    /// Detects a `PPI::Structure::List` whose sole content is `START .. END` (Perl's range
    /// operator) and renders each side to a JS expression — see `render_foreach`'s call site for
    /// why this needs its own counting-loop template rather than the generic `for...of`. Returns
    /// `None` for every other list shape (a real element list, a function call's arguments, etc.),
    /// so callers fall back to the pre-existing generic rendering unchanged.
    fn try_render_range_list(&mut self, list: &PpiNode) -> Option<(String, String)> {
        let inner = self.real_children(list.children());
        let stmt = match inner.as_slice() {
            [stmt] if stmt.class == "PPI::Statement" => stmt,
            _ => return None,
        };
        let tokens = self.real_children(stmt.children());
        let dotdot_idx = tokens.iter().position(|t| {
            t.class == "PPI::Token::Operator" && t.content.as_deref() == Some("..")
        })?;
        let start_tokens = &tokens[..dotdot_idx];
        let end_tokens = &tokens[dotdot_idx + 1..];
        if start_tokens.is_empty() || end_tokens.is_empty() {
            return None;
        }
        let start_expr = self.render_expr_sequence(start_tokens);
        let end_expr = self.render_expr_sequence(end_tokens);
        Some((start_expr, end_expr))
    }

    /// `eval { BLOCK };` — Perl's exception-trapping block, tokenized as a plain `PPI::Statement`
    /// (`Word("eval")` + `Structure::Block`), not a `Compound` (PPI only classifies `if`/
    /// `unless`/`while`/`until`/`for`/`foreach` that way). Maps to `try { BLOCK } catch
    /// (perlError) {}` — Perl's own semantics (execute the block; on failure, set `$@` and
    /// *keep going*, checked by the caller afterward, not scoped to a handler) line up with
    /// JS's `try`/`catch` closely enough that this is a faithful, not just approximate,
    /// conversion — paired with `render_magic`'s `$@` → `perlError` mapping.
    fn try_render_eval_block(&mut self, node: &PpiNode) -> Option<String> {
        let children = self.real_children(node.children());
        let (first, second) = (children.first()?, children.get(1)?);
        if !is_word(first, "eval") || second.class != "PPI::Structure::Block" {
            return None;
        }
        let body = self.render_statement_list(second.children());
        // `perlError` must be declared *outside* the try/catch, not as the `catch` clause's own
        // parameter — Perl code checks `$@` *after* the eval block ends (`if ($@ || ...)`, e.g.),
        // and a JS `catch (perlError)` binding is scoped to just that block, so referencing
        // `perlError` afterward would be a `ReferenceError`, not the "no error occurred" falsy
        // check the Perl original relies on.
        Some(format!(
            "let perlError;\ntry {{\n{body}}} catch (caughtError) {{\n  perlError = caughtError;\n}}"
        ))
    }

    /// `while (my $row = <$fh>) { BODY }` half of the file-read idiom — see the call site's own
    /// docs (`render_compound_statement`'s `"while"` handling) and
    /// `try_render_open_file_statement`'s docs for the paired `open(...)` half.
    fn try_render_readline_while(
        &mut self,
        cond: Option<&PpiNode>,
        block: Option<&PpiNode>,
    ) -> Option<String> {
        let cond = cond?;
        let block = block?;
        let cond_children = self.real_children(cond.children());
        if cond_children.len() != 1 {
            return None;
        }
        let var_stmt = cond_children[0];
        if var_stmt.class != "PPI::Statement::Variable" {
            return None;
        }
        let vc = self.real_children(var_stmt.children());
        if !is_word(vc.first()?, "my") {
            return None;
        }
        let var_symbol = vc.get(1)?;
        if var_symbol.class != "PPI::Token::Symbol" {
            return None;
        }
        let var_name = strip_sigil(var_symbol.content.as_deref().unwrap_or(""));
        let readline_tok = vc.get(3)?;
        if readline_tok.class != "PPI::Token::QuoteLike::Readline" {
            return None;
        }
        let raw = readline_tok.content.as_deref().unwrap_or("");
        let fh_name = strip_sigil(raw.trim_start_matches('<').trim_end_matches('>'));
        let body = self.render_statement_list(block.children());
        // `let`, not `const` — same reasoning as `render_foreach`'s identical choice (its own
        // docs): the loop body routinely reassigns this variable (`chomp $row;`/`$row =~
        // s/.../ /;`, part of the shared file-read idiom every sidecar-metadata plugin's loop
        // body does).
        Some(format!(
            "for (let {var_name} of ({fh_name} ?? \"\").split(/\\r?\\n/)) {{\n{body}}}"
        ))
    }

    /// `while (my ($k, $v) = each %$hash) { BODY }` — Perl's stateful hash-iterator idiom, used
    /// in a plain `while` this way it always just means "iterate every entry once" — maps
    /// directly onto a real `for...of` over `Object.entries(...)`.
    fn try_render_each_hash_while(
        &mut self,
        cond: Option<&PpiNode>,
        block: Option<&PpiNode>,
    ) -> Option<String> {
        let cond = cond?;
        let block = block?;
        let cond_children = self.real_children(cond.children());
        if cond_children.len() != 1 {
            return None;
        }
        let var_stmt = cond_children[0];
        if var_stmt.class != "PPI::Statement::Variable" {
            return None;
        }
        let vc = self.real_children(var_stmt.children());
        if !is_word(vc.first()?, "my") {
            return None;
        }
        let list = vc.get(1)?;
        if list.class != "PPI::Structure::List" {
            return None;
        }
        let names = symbol_names_in(list);
        if names.len() != 2 {
            return None;
        }
        if !is_operator(vc.get(2)?, "=") || !is_word(vc.get(3)?, "each") {
            return None;
        }
        // `each %hash` (plain) or `each %$hash_ref` (Cast + Symbol dereference) — both end up as
        // whatever trailing tokens remain after `each`.
        let hash_tokens = &vc[4..];
        let hash_expr = self.render_expr_sequence(hash_tokens);
        let body = self.render_statement_list(block.children());
        Some(format!(
            "for (const [{}, {}] of Object.entries({hash_expr} ?? {{}})) {{\n{body}}}",
            names[0], names[1]
        ))
    }

    /// `open(my $fh, MODE, PATH_EXPR) or die MSG;` — legacy's real-file-open idiom, always paired
    /// with the sidecar-file-read pattern this host resolves before the plugin ever runs (see
    /// `render_named_call`'s `is_file_in_archive`/`extract_file_from_archive` cases): by the time
    /// `PATH_EXPR` reaches here it's already the resolved file *content* (or `undefined` if the
    /// sidecar file wasn't found), not a real path, so "opening" it is just a plain assignment,
    /// and Perl's `or die` (which handled a real open() failure) becomes a real not-found check
    /// on that `undefined` instead.
    fn try_render_open_file_statement(&mut self, node: &PpiNode) -> Option<String> {
        let children = self.real_children(node.children());
        let first = children.first()?;
        // `LANraragi::Utils::Path::open_path_or_die` (`Metadata/Eze.pm`) is a thin same-shape
        // wrapper around the builtin — it dies internally on failure instead of needing a
        // trailing `or die`, which the check just below already handles gracefully (no `or`/`die`
        // tokens follow, so `die_msg` just stays `None` — see the default fallback message below
        // for why this variant still gets a real check anyway).
        let is_open_path_or_die = is_word(first, "open_path_or_die");
        if !is_word(first, "open") && !is_open_path_or_die {
            return None;
        }
        let list = *children.get(1)?;
        if list.class != "PPI::Structure::List" {
            return None;
        }
        let inner = self.real_children(list.children());
        let var_stmt = *inner.first()?;
        if var_stmt.class != "PPI::Statement::Variable" || inner.len() != 1 {
            return None;
        }
        let vc = self.real_children(var_stmt.children());
        if !is_word(vc.first()?, "my") {
            return None;
        }
        let fh_symbol = vc.get(1)?;
        if fh_symbol.class != "PPI::Token::Symbol" {
            return None;
        }
        let fh_name = strip_sigil(fh_symbol.content.as_deref().unwrap_or(""));
        // `my $fh, MODE, PATH...` — skip past the *second* top-level comma (the first separates
        // `$fh` from `MODE`, the second separates `MODE` from `PATH`) to find where `PATH`'s own
        // tokens start; `PATH` itself may be a single symbol or a longer expression.
        let comma_positions: Vec<usize> = vc
            .iter()
            .enumerate()
            .filter(|(_, t)| is_operator(t, ","))
            .map(|(idx, _)| idx)
            .collect();
        let path_start = *comma_positions.get(1)? + 1;
        let path_tokens = vc[path_start..].to_vec();
        let path_expr = self.render_expr_sequence(&path_tokens);

        let mut die_msg = None;
        if children
            .get(2)
            .map(|t| is_operator(t, "or"))
            .unwrap_or(false)
            && children.get(3).map(|t| is_word(t, "die")).unwrap_or(false)
        {
            let msg_tokens: Vec<&PpiNode> = children[4..]
                .iter()
                .copied()
                .filter(|c| {
                    !(c.class == "PPI::Token::Structure" && c.content.as_deref() == Some(";"))
                })
                .collect();
            die_msg = Some(self.render_expr_sequence(&msg_tokens));
        } else if is_open_path_or_die {
            die_msg = Some("\"Could not open the requested file.\"".to_string());
        }

        let mut out = format!("let {fh_name} = {path_expr};");
        if let Some(msg) = die_msg {
            out.push_str(&format!(
                "\nif ({fh_name} === undefined) {{ throw new Error({msg}); }}"
            ));
        }
        Some(out)
    }

    /// `open my $fh, '>'|'>>', PATH or die ...; print $fh CONTENT; close $fh;` — see this
    /// function's call site in `render_statement_list` for the full rationale (the file-*write*
    /// mirror of `try_render_open_file_statement`'s file-*read* idiom, needing a real
    /// `Deno.writeTextFile`/`Deno.writeFile` call rather than anything host-pre-resolved). Returns
    /// `(rendered, statements_consumed)` — `statements_consumed` is always 3 on a match (the
    /// `close` is required, not optional: without it, this couldn't be distinguished from some
    /// other, unrelated three-statement sequence that merely happens to start with an `open`
    /// this function doesn't recognize, e.g. a real read-mode `open` already handled by
    /// `try_render_open_file_statement`, which runs its own, narrower single-statement match
    /// first and never reaches here for that case).
    ///
    /// `CONTENT`'s own shape decides which Deno call this becomes: a real corpus case
    /// (`Download/Pixiv.pm`) writes `$img_res->body` (an HTTP response body — could be binary, so
    /// this can't assume text) rather than a plain string literal, so this always renders
    /// `Deno.writeFile` with a `TextEncoder`-wrapped fallback for whatever isn't already binary —
    /// safe for both cases, unlike assuming `writeTextFile` and being wrong for a binary body.
    fn try_render_write_file_statements(&mut self, stmts: &[&PpiNode]) -> Option<(String, usize)> {
        let [open_stmt, print_stmt, close_stmt] = stmts.first_chunk::<3>()?;
        if open_stmt.class != "PPI::Statement"
            || print_stmt.class != "PPI::Statement"
            || close_stmt.class != "PPI::Statement"
        {
            return None;
        }

        // `open my $fh, MODE, PATH or die ...;` — unlike the parenthesized read-mode idiom
        // `try_render_open_file_statement` handles (`open(my $fh, MODE, PATH)`, where `my $fh`
        // sits inside a `Structure::List` and PPI nests it as its own `Statement::Variable`), this
        // *unparenthesized* form has no such nesting at all — `open`, `my`, `$fh`, and everything
        // after are all flat, equal-depth tokens in the same statement (verified via real
        // `perl -MPPI::Dumper` output on `Download/Pixiv.pm`'s own `image_res_to_zip`; an earlier
        // version of this function wrongly assumed the same nested shape the read-mode idiom has
        // and so never matched anything at all).
        let open_children = self.real_children(open_stmt.children());
        if !is_word(*open_children.first()?, "open") {
            return None;
        }
        if !is_word(*open_children.get(1)?, "my") {
            return None;
        }
        let fh_symbol = *open_children.get(2)?;
        if fh_symbol.class != "PPI::Token::Symbol" {
            return None;
        }
        let fh_name = strip_sigil(fh_symbol.content.as_deref().unwrap_or(""));
        let comma_positions: Vec<usize> = open_children
            .iter()
            .enumerate()
            .filter(|(_, t)| is_operator(t, ","))
            .map(|(idx, _)| idx)
            .collect();
        let mode_start = *comma_positions.first()? + 1;
        let mode_end = *comma_positions.get(1)?;
        let mode_tokens = &open_children[mode_start..mode_end];
        let [mode_tok] = mode_tokens else { return None };
        let mode = match mode_tok.class.as_str() {
            "PPI::Token::Quote::Single" => strip_quotes(mode_tok.content.as_deref()?, '\''),
            "PPI::Token::Quote::Double" => strip_quotes(mode_tok.content.as_deref()?, '"'),
            _ => return None,
        };
        if mode != ">" && mode != ">>" {
            return None; // some other open mode this converter doesn't specifically handle.
        }
        // `PATH` runs up to (not including) an ` or die ...` tail, if present — same reasoning as
        // `try_render_open_file_statement`'s own `or`/`die` handling, just without needing to
        // preserve a die message here (see this function's own doc comment on why `close`'s
        // presence, not a `die` message, is what confirms this is really the write idiom).
        let after_mode = &open_children[mode_end + 1..];
        let or_idx = after_mode.iter().position(|t| is_operator(t, "or"));
        let path_tokens = match or_idx {
            Some(idx) => &after_mode[..idx],
            None => after_mode,
        };
        let path_expr = self.render_expr_sequence(path_tokens);

        // `print $fh CONTENT;`
        let print_children = self.real_children(print_stmt.children());
        if !is_word(*print_children.first()?, "print") {
            return None;
        }
        let print_fh = print_children.get(1)?;
        if print_fh.class != "PPI::Token::Symbol"
            || strip_sigil(print_fh.content.as_deref().unwrap_or("")) != fh_name
        {
            return None; // printing to some other filehandle, not this one.
        }
        let content_tokens: Vec<&PpiNode> = print_children[2..]
            .iter()
            .copied()
            .filter(|c| !(c.class == "PPI::Token::Structure" && c.content.as_deref() == Some(";")))
            .collect();
        if content_tokens.is_empty() {
            return None;
        }
        let content_expr = self.render_expr_sequence(&content_tokens);

        // `close $fh;` — required to be sure this really is the write idiom's closing statement
        // (see this function's own docs), but Deno's `writeFile` needs no explicit close of its
        // own, so nothing from it is used beyond checking its shape matches.
        let close_children = self.real_children(close_stmt.children());
        if !is_word(*close_children.first()?, "close") {
            return None;
        }
        let close_fh = close_children.get(1)?;
        if close_fh.class != "PPI::Token::Symbol"
            || strip_sigil(close_fh.content.as_deref().unwrap_or("")) != fh_name
        {
            return None;
        }

        let flag = if mode == ">>" {
            ", { append: true }"
        } else {
            ""
        };
        self.used_await = true;
        Some((
            format!(
                "await Deno.writeFile({path_expr}, typeof {content_expr} === \"string\" ? new TextEncoder().encode({content_expr}) : {content_expr}{flag});"
            ),
            3,
        ))
    }

    /// Postfix statement modifiers — `EXPR unless COND;` / `EXPR if COND;` — which PPI leaves as
    /// a single flat `PPI::Statement` (the main expression's tokens, then the `if`/`unless`
    /// keyword, then the condition's tokens), not a `Compound` (there's no `{ }` block here).
    fn try_render_postfix_modifier(&mut self, node: &PpiNode) -> Option<String> {
        let children = self.real_children(node.children());
        let kw_idx = children.iter().position(|c| {
            is_word(c, "if") || is_word(c, "unless") || is_word(c, "for") || is_word(c, "foreach")
        })?;
        if kw_idx == 0 {
            return None;
        }
        let keyword = children[kw_idx].content.as_deref().unwrap_or("if");
        let expr_tokens: Vec<&PpiNode> = children[..kw_idx].to_vec();
        let rest_tokens: Vec<&PpiNode> = children[kw_idx + 1..]
            .iter()
            .copied()
            .filter(|c| !(c.class == "PPI::Token::Structure" && c.content.as_deref() == Some(";")))
            .collect();

        let expr = self.render_expr_sequence(&expr_tokens);
        // `EXPR for LIST;` / `EXPR foreach LIST;` — Perl's postfix loop modifier (real corpus
        // case: `Metadata/EHDLInfo.pm`'s `push @array, "$ns:$_" for @$value;`), binding each
        // `LIST` element to `$_` while `EXPR` runs once per item — `render_magic` already maps
        // every bare `$_` reference to `it` (see its own docs), matching the block-form
        // `grep`/`map` handling's identical convention.
        if keyword == "for" || keyword == "foreach" {
            let list = self.render_expr_sequence(&rest_tokens);
            return Some(format!("for (const it of {list}) {{ {expr}; }}"));
        }
        let cond = self.render_expr_sequence(&rest_tokens);
        Some(if keyword == "unless" {
            format!("if (!({cond})) {{ {expr}; }}")
        } else {
            format!("if ({cond}) {{ {expr}; }}")
        })
    }

    /// `($a, $b) = some_call(...);` — a re-assignment to already-declared variables (contrast
    /// with `my ($a, $b) = @_;`, which `render_variable_statement` handles; this is the no-`my`
    /// counterpart, e.g. re-binding a loop variable or a value first declared earlier in the sub).
    /// Rendering this through the generic expression fallback would keep the LHS as a literal
    /// `(gID, gToken)`, which isn't a valid JS assignment target at all (a parenthesized
    /// expression can't appear on the left of `=`) — must become a real array-destructuring
    /// assignment (`[gID, gToken] = ...;`), the no-`let` counterpart of what
    /// `render_variable_statement` already emits for the `my`-prefixed form.
    fn try_render_list_assignment(&mut self, node: &PpiNode) -> Option<String> {
        let children = self.real_children(node.children());
        let (first, second) = (children.first()?, children.get(1)?);
        if first.class != "PPI::Structure::List" || !is_operator(second, "=") {
            return None;
        }
        let names = symbol_names_in(first);
        if names.is_empty() {
            return None;
        }
        let rhs: Vec<&PpiNode> = children[2..]
            .iter()
            .copied()
            .filter(|n| !(n.class == "PPI::Token::Structure" && n.content.as_deref() == Some(";")))
            .collect();
        let expr = self.render_expr_sequence(&rhs);
        Some(format!("[{}] = {expr};", names.join(", ")))
    }

    /// `return (tags => $tags, title => $title);` / `return ($a, $b);` / `return $x;` /
    /// bare `return;`.
    fn render_break_statement(&mut self, node: &PpiNode) -> String {
        let children = self.real_children(node.children());
        let keyword = children
            .first()
            .and_then(|c| c.content.as_deref())
            .unwrap_or("return");
        // `next`/`last` are Perl's loop-control keywords (`redo` also exists but has no real JS
        // equivalent at all — not seen anywhere in the legacy plugin corpus, so left unmapped
        // rather than guessed at) — map onto their real JS statement equivalents rather than
        // emitting the bareword verbatim (`next;`/`last;` aren't valid JS on their own at all;
        // `continue`/`break` are what they actually mean here).
        let js_keyword = match keyword {
            "next" => "continue",
            "last" => "break",
            _ => keyword,
        };
        if keyword != "return" {
            // `next if COND;` / `last unless COND;` — same postfix-modifier shape as `return`
            // below (see its own docs for why `PPI::Statement::Break` needs this handled
            // separately from `try_render_postfix_modifier`); real corpus cases:
            // `GalleryDL.pm`'s `next unless exists $hash->{$field};`,
            // `RegexParse.pm`'s `next if $name eq 'title' || $name eq 'tail';`.
            if let Some((cond, kw_idx)) = self.find_postfix_condition(&children) {
                return if children[kw_idx].content.as_deref() == Some("unless") {
                    format!("if (!({cond})) {{ {js_keyword}; }}")
                } else {
                    format!("if ({cond}) {{ {js_keyword}; }}")
                };
            }
            return format!("{js_keyword};");
        }

        // `return EXPR if COND;` / `return EXPR unless COND;` — Perl's postfix statement
        // modifier, the same shape `try_render_postfix_modifier` handles for ordinary statements.
        // But PPI parses any statement starting with `return`/`last`/`next` as its own
        // `PPI::Statement::Break` node rather than `PPI::Statement`/`::Expression`, so it never
        // reaches that check at all — this fell straight through to the plain-return rendering
        // below with no idea `if`/`unless` here was a trailing modifier keyword rather than part
        // of the return value, emitting flatly invalid JS like `return if (cond);` (a real
        // instance found in the wild: `Metadata/CopyArchiveTags.pm`'s `return unless $oneshot &&
        // length($oneshot) >= 40;`).
        if let Some((cond, kw_idx)) = self.find_postfix_condition(&children) {
            let modifier_keyword = children[kw_idx].content.as_deref().unwrap_or("if");
            let inner_stmt = self.render_break_return_value(&children[1..kw_idx]);
            return if modifier_keyword == "unless" {
                format!("if (!({cond})) {{ {inner_stmt} }}")
            } else {
                format!("if ({cond}) {{ {inner_stmt} }}")
            };
        }

        self.render_break_return_value(&children[1..])
    }

    /// Finds a trailing `if COND`/`unless COND` postfix modifier among `children` (starting the
    /// search *after* the leading keyword at index 0, so `if`/`unless` can never be mistaken for
    /// itself), rendering `COND` — used by `render_break_statement`'s `return`/`next`/`last`
    /// handling. Returns the rendered condition plus the index the `if`/`unless` keyword itself
    /// was found at (the caller still needs that to know where the "real" value/keyword tokens
    /// end).
    fn find_postfix_condition(&mut self, children: &[&PpiNode]) -> Option<(String, usize)> {
        let kw_idx = children[1..]
            .iter()
            .position(|c| is_word(c, "if") || is_word(c, "unless"))
            .map(|i| i + 1)?;
        let cond_tokens: Vec<&PpiNode> = children[kw_idx + 1..]
            .iter()
            .copied()
            .filter(|c| !(c.class == "PPI::Token::Structure" && c.content.as_deref() == Some(";")))
            .collect();
        let cond = self.render_expr_sequence(&cond_tokens);
        Some((cond, kw_idx))
    }

    /// Renders everything after the `return` keyword (and, when a postfix `if`/`unless` modifier
    /// was present, after stripping that off too — see `render_break_statement`) into a
    /// `return ...;` statement.
    fn render_break_return_value(&mut self, after_return: &[&PpiNode]) -> String {
        let rest: Vec<&&PpiNode> = after_return
            .iter()
            .filter(|c| c.class != "PPI::Token::Structure")
            .collect();
        if rest.is_empty() {
            return "return;".to_string();
        }

        // `return (k => v, k2 => v2);` — the metadata-result hash-literal idiom.
        if rest.len() == 1 && rest[0].class == "PPI::Structure::List" {
            if let Some(rendered) = self.try_render_hash_return(rest[0]) {
                return rendered;
            }
        }

        // `return ($a, $b);` / `return $a, $b;` — Perl's multi-value list return. Rendering this
        // through the generic expression path below would keep the literal parens (or add none),
        // which JS reads as the *comma operator*: it silently evaluates every element and keeps
        // only the last one, discarding every earlier value instead of raising an error — exactly
        // the "wrong answer with no error" failure mode this converter otherwise tries hard to
        // avoid. A genuine multi-value return must become a real TS array so a caller's
        // `let [a, b] = helper(...)` destructure (which `render_variable_statement` already
        // generates for the equivalent Perl-side `my ($a, $b) = helper(...);`) actually works.
        let inner: Vec<&PpiNode> = if rest.len() == 1 && rest[0].class == "PPI::Structure::List" {
            self.flatten_statement_children(rest[0].children())
        } else {
            rest.iter().map(|n| **n).collect()
        };
        if has_top_level_comma(&inner) {
            let elements: Vec<String> = split_on_commas(&inner)
                .into_iter()
                .map(|group| self.render_expr_sequence(&group))
                .collect();
            return format!("return [{}];", elements.join(", "));
        }

        let expr = self.render_expr_sequence(&rest.iter().map(|n| **n).collect::<Vec<_>>());
        format!("return {expr};")
    }

    /// Binds the entry point's own shift/`@_`-destructured names against `hostArgs` — the first
    /// name (any sigil) is always the info hash (legacy's `$lrr_info`, holding `archive_id`/
    /// `file_path`/etc., which `hostArgs` already *is* under the host's real calling convention);
    /// every name after that is the plugin's one custom value (`hostArgs.arg`), wrapped in an
    /// object keyed by the plugin's first declared parameter name if the destructured slot was
    /// itself hash-sigil'd (`%params`). Emits one `let` per name rather than a single JS
    /// destructuring assignment, since each name's RHS mapping differs.
    fn render_entry_destructure(&mut self, symbols: &[(char, String)]) -> String {
        let first_param_name = self
            .entry
            .as_ref()
            .and_then(|ctx| ctx.first_param_name.clone())
            .unwrap_or_else(|| "value".to_string());
        let mut lines = Vec::new();
        for (sigil, name) in symbols {
            let already_bound = self
                .entry
                .as_ref()
                .map(|ctx| ctx.info_hash_bound)
                .unwrap_or(false);
            if !already_bound {
                let iface_name = self
                    .entry
                    .as_ref()
                    .and_then(|ctx| ctx.info_hash_interface.as_ref())
                    .map(|(iface_name, _)| iface_name.clone());
                lines.push(match iface_name {
                    Some(iface_name) => {
                        format!("let {name} = hostArgs as unknown as {iface_name};")
                    }
                    None => format!("let {name} = hostArgs as Record<string, any>;"),
                });
                if let Some(ctx) = &mut self.entry {
                    ctx.info_hash_bound = true;
                }
            } else if *sigil == '%' {
                lines.push(format!(
                    "let {name} = {{ {first_param_name}: hostArgs.arg as string }};"
                ));
            } else {
                lines.push(format!("let {name} = hostArgs.arg as string;"));
            }
        }
        lines.join("\n")
    }

    fn try_render_hash_return(&mut self, list: &PpiNode) -> Option<String> {
        let inner = self.flatten_statement_children(list.children());
        let fields = self.render_key_value_pairs(&inner)?;
        Some(format!("return {{ {} }};", fields.join(", ")))
    }

    /// Flattens a `Structure`'s children the same way [`Self::render_children_expr`] does
    /// (unwrapping any `Statement`/`Statement::Expression` wrapper nodes PPI inserts inside
    /// parens/braces), without immediately rendering them — used by callers that need to inspect
    /// the token sequence themselves first (finding `=>` positions, splitting on commas) before
    /// deciding how to render it.
    fn flatten_statement_children<'a>(&self, nodes: &'a [PpiNode]) -> Vec<&'a PpiNode> {
        self.real_children(nodes)
            .into_iter()
            .flat_map(|n| {
                if n.class == "PPI::Statement" || n.class == "PPI::Statement::Expression" {
                    self.real_children(n.children())
                } else {
                    vec![n]
                }
            })
            .collect()
    }

    /// Renders a flat token sequence as `key: value` pairs — Perl's `key => value, key2 => value2`
    /// hash-literal idiom, used both by the `return (...)` idiom above and by `{ ... }` hash
    /// constructors below. Returns `None` if any top-level comma-separated group doesn't contain
    /// a `=>` (i.e. this isn't actually hash-literal-shaped — could be a plain list/array, whose
    /// `=>` — if any appear at all — are just fat commas, ordinary argument separators with no
    /// key/value meaning, and must NOT be rendered as `key: value`; see `render_operator`'s docs).
    fn render_key_value_pairs(&mut self, tokens: &[&PpiNode]) -> Option<Vec<String>> {
        let groups = split_on_commas(tokens);
        let mut fields = Vec::new();
        for group in &groups {
            let pos = group.iter().position(|n| is_operator(n, "=>"))?;
            let key = group[..pos]
                .iter()
                .map(|n| bareword_or_string_content(n))
                .collect::<Vec<_>>()
                .join("");
            let value_tokens: Vec<&PpiNode> = group[pos + 1..].to_vec();
            let value = self.render_expr_sequence(&value_tokens);
            fields.push(format!("{key}: {value}"));
        }
        if fields.is_empty() {
            return None;
        }
        Some(fields)
    }

    fn render_children_expr(&mut self, node: &PpiNode) -> String {
        let children: Vec<&PpiNode> = node
            .children()
            .iter()
            .filter(|c| c.class != "PPI::Token::Whitespace" && c.class != "PPI::Token::Comment")
            .flat_map(|c| {
                if c.class == "PPI::Statement" || c.class == "PPI::Statement::Expression" {
                    self.real_children(c.children())
                } else {
                    vec![c]
                }
            })
            .collect();
        self.render_expr_sequence(&children)
    }

    fn render_expr_sequence(&mut self, tokens: &[&PpiNode]) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            // `EXPR->get(URL)->result` / `EXPR->post(URL, form => {...})->result` —
            // `legacyCompat.userAgent()`'s `get`/`post` are real `fetch()` calls under the hood
            // (see `dispatcher.ts`), unlike every synchronous call this renderer otherwise emits,
            // so this one chain shape needs an `await` wrapped around the *receiver plus the
            // call* (not just the call — `EXPR.get(url)` is the promise, not `EXPR` alone).
            // Checked narrowly (name must be exactly `get`/`post`, immediately followed by
            // `->result`) so an unrelated helper sub that happens to share one of these very
            // common names is never mistaken for the Mojo idiom. Must run *before*
            // `try_match_call` below, which would otherwise generically match `get`/`post` as an
            // ordinary (un-awaited) call first.
            // `EXPR =~ /pattern/flags` — Perl's regex-match operator. `render_operator`'s
            // context-free `=~` mapping can't do this alone (a lone operator token has no idea
            // what its left operand rendered to, or that a `Regexp::Match` token follows), so this
            // is handled here instead, where both are visible: everything accumulated in `parts`
            // so far *is* the left operand (LHS is whatever expression tokens came before this
            // point — could be a single `$var` or a longer chain like `$hash->{key}`), and
            // `tokens[i + 1]` is the regex literal (only the plain `/pattern/flags` delimiter form
            // is handled — `m{...}`/`m(...)` etc. are rare in the legacy plugin corpus and fall
            // through to the generic `=~` TODO-comment fallback below, same as before this fix).
            // Assigns into a `match` variable (rather than just calling `.test()`) because legacy
            // code routinely reads `$1`/`$2` afterward — already mapped by `render_magic` to
            // `match[1]`/`match[2]`, but that mapping was never actually backed by a real
            // `match` binding until now, since this operator itself was previously a no-op stub.
            if is_operator(tokens[i], "=~")
                && tokens.get(i + 1).map(|t| t.class.as_str()) == Some("PPI::Token::Regexp::Match")
            {
                if let Some(js_regex) = render_perl_regex_literal(tokens[i + 1]) {
                    // Perl's `/g` flag in list context (`my (@caps) = $x =~ /pat/g;`) returns
                    // every captured group across every match; JS's `.match()` with a `g` flag
                    // returns only the full-match strings (capture groups are unreachable that
                    // way — you'd need `matchAll` and to flatten each result's own groups
                    // instead). That's a real semantic difference this converter can't safely
                    // paper over without knowing whether the surrounding context is list or
                    // scalar (a distinction PPI's tree doesn't make explicit at this token), so
                    // it's surfaced as a warning for manual review rather than silently emitting
                    // code that quietly returns the wrong values.
                    if js_regex.contains('g') {
                        self.warnings.push(format!(
                            "=~ with /g in list context ({js_regex}) — Perl returns every captured group across all matches; JS .match() with /g returns only full-match strings, not captures. Verify this call site by hand."
                        ));
                    }
                    self.uses_regex_match = true;
                    let lhs = join_parts(&parts);
                    parts.clear();
                    parts.push(format!("(match = {lhs}.match({js_regex}))"));
                    i += 2;
                    continue;
                }
            }
            // `EXPR =~ !~ / etc.` negated match, boolean context (`if ($x !~ /pat/)`) — same
            // token shape as the plain `=~` + `Regexp::Match` case above, just wrapped in `!`;
            // must run before the generic `->`/operator fallback below, same reasoning as that
            // case.
            if is_operator(tokens[i], "!~")
                && tokens.get(i + 1).map(|t| t.class.as_str()) == Some("PPI::Token::Regexp::Match")
            {
                if let Some(js_regex) = render_perl_regex_literal(tokens[i + 1]) {
                    let lhs = join_parts(&parts);
                    parts.clear();
                    parts.push(format!("!{js_regex}.test({lhs})"));
                    i += 2;
                    continue;
                }
            }
            // `EXPR =~ $compiled_regex` / `EXPR !~ $compiled_regex` — matching against a
            // previously-`qr/.../`-compiled pattern stored in a variable, rather than an inline
            // regex literal (real corpus case: `Metadata/RegexParse.pm`'s `$filename =~ $regex`,
            // where `$regex` was built via `my $regex = qr/.../`, itself already rendered as a
            // real JS `RegExp` value by `render_perl_regex_literal`'s own `qr` handling — so this
            // just calls `.test()` on that value instead of parsing a pattern out of a token here.
            if (is_operator(tokens[i], "=~") || is_operator(tokens[i], "!~"))
                && tokens.get(i + 1).map(|t| t.class.as_str()) == Some("PPI::Token::Symbol")
            {
                let negate = is_operator(tokens[i], "!~");
                let lhs = join_parts(&parts);
                parts.clear();
                let pattern_var = strip_sigil(tokens[i + 1].content.as_deref().unwrap_or(""));
                let bang = if negate { "!" } else { "" };
                parts.push(format!("{bang}{pattern_var}.test({lhs})"));
                i += 2;
                continue;
            }
            // `EXPR =~ s/pattern/replacement/flags` — Perl's substitution operator, which
            // mutates `EXPR` in place (unless the non-destructive `/r` flag is present, in which
            // case it leaves `EXPR` untouched and evaluates to the substituted *copy* instead —
            // e.g. `my $namespace = $name =~ s/\d+$//r;` in the legacy corpus). Both map cleanly
            // onto JS's `String.prototype.replace`, which already treats `$1`/`$2` backreferences
            // in the replacement the same way Perl does.
            if is_operator(tokens[i], "=~")
                && tokens.get(i + 1).map(|t| t.class.as_str())
                    == Some("PPI::Token::Regexp::Substitute")
            {
                if let Some(rendered_call) =
                    self.render_regex_substitute_call(&join_parts(&parts), tokens[i + 1])
                {
                    parts.clear();
                    parts.push(rendered_call);
                    i += 2;
                    continue;
                }
            }
            // `EXPR->to_string` (`Mojo::DOM`'s no-parens string-conversion method — Perl doesn't
            // require parens on a zero-arg method call) — without this, the generic `->` → `.`
            // token mapping renders it as `expr.to_string` (a *property* access, not a call),
            // which is both the wrong JS method name (`LegacyDomNode` only exposes `toString()`,
            // matching the real JS stringification convention) and missing its `()` entirely.
            // Checked narrowly (must be immediately preceded by a rendered `->` in `parts`) so an
            // unrelated bareword named `to_string` elsewhere is never mistaken for this.
            if is_word(tokens[i], "to_string") && parts.last().map(|p| p == ".").unwrap_or(false) {
                parts.pop();
                parts.push(format!("{NO_SPACE_BEFORE}.toString()"));
                i += 1;
                continue;
            }
            // `EXPR->as_string` — `URI`'s no-parens stringification method (see
            // `try_match_legacy_http_constructor`'s own docs on the `URI`→`URL` mapping this
            // belongs to). Same reasoning and shape as the `to_string` case just above (Perl
            // doesn't require parens on a zero-arg method call, so without this it would render as
            // a property access on a name that doesn't exist on `URL` at all) — `URL`'s own
            // equivalent is `.href` (a getter, still no parens needed, unlike `toString()`).
            if is_word(tokens[i], "as_string") && parts.last().map(|p| p == ".").unwrap_or(false) {
                parts.pop();
                parts.push(format!("{NO_SPACE_BEFORE}.href"));
                i += 1;
                continue;
            }
            // `EXPR->query("start=1")` — `URI`'s query-string setter. `URL`'s own equivalent
            // (`.search`) is a plain assignable property, not a method — this rewrites the whole
            // `->query(ARG)` call into a `.search = ARG` assignment rather than emitting an
            // (invalid, `URL` has no `query()` method) call.
            if is_word(tokens[i], "query")
                && parts.last().map(|p| p == ".").unwrap_or(false)
                && tokens.get(i + 1).map(|t| t.class.as_str()) == Some("PPI::Structure::List")
            {
                let args = self.render_call_args(tokens[i + 1]);
                if args.len() == 1 {
                    parts.pop();
                    parts.push(format!("{NO_SPACE_BEFORE}.search = {}", args[0]));
                    i += 2;
                    continue;
                }
            }
            // `$uri_typed_var eq ""` — real corpus case: `Download/EHentai.pm`'s `if ($@ ||
            // $finalURL eq "")`, checking whether a `URI->new()`-initialized variable was ever
            // reassigned a real value (see `uri_typed_vars`'s own docs). Once that variable's
            // mapped JS type is `URL | undefined` (not `string`), comparing it against `""`
            // directly is a `deno check` error (`URL | undefined` and `string` share no values at
            // all, unlike Perl, which lets any value compare against any string) — rewritten to
            // compare against `undefined` instead, which is exactly as always-`false` as the
            // original Perl comparison was (a real `URI` object reference is never `eq` an empty
            // string either), so this changes nothing about the actual runtime behavior, only
            // what makes it type-check.
            if tokens[i].class == "PPI::Token::Symbol"
                && self
                    .uri_typed_vars
                    .contains(&strip_sigil(tokens[i].content.as_deref().unwrap_or("")))
                && tokens
                    .get(i + 1)
                    .map(|t| is_operator(t, "eq"))
                    .unwrap_or(false)
                && matches!(
                    tokens.get(i + 2).map(|t| t.class.as_str()),
                    Some("PPI::Token::Quote::Single" | "PPI::Token::Quote::Double")
                )
                && tokens
                    .get(i + 2)
                    .and_then(|t| t.content.as_deref())
                    .is_some_and(|c| c == "''" || c == "\"\"")
            {
                let var_name = strip_sigil(tokens[i].content.as_deref().unwrap_or(""));
                parts.push(format!("{var_name} === undefined"));
                i += 3;
                continue;
            }
            // `split(/pattern/, LIST)` / `split(qr/.../ , LIST)` — a regex literal used as a
            // plain *value* (split's own pattern argument), not a boolean condition. Must run
            // before the generic call-argument machinery below: each argument there gets rendered
            // through the same per-token dispatch a bare (non-`=~`) `Regexp::Match` hits inside a
            // `grep`/`map` condition — where it's *correctly* turned into `/pattern/.test(it)`,
            // since it implicitly tests against `$_` there — but `split`'s pattern argument was
            // never a condition at all, so that rule doesn't apply and previously produced
            // `value.split(/,/.test(it))`, a real bug found in the wild
            // (`Metadata/HDoujin.pm`/`RegexParse.pm`'s own `split(/,/, $value)` calls).
            if is_word(tokens[i], "split")
                && tokens.get(i + 1).map(|t| t.class.as_str()) == Some("PPI::Structure::List")
            {
                let list = tokens[i + 1];
                let inner: Vec<&PpiNode> = list
                    .children()
                    .iter()
                    .filter(|c| c.class != "PPI::Token::Whitespace")
                    .flat_map(|c| {
                        if c.class == "PPI::Statement" || c.class == "PPI::Statement::Expression" {
                            self.real_children(c.children())
                        } else {
                            vec![c]
                        }
                    })
                    .collect();
                let groups = split_on_commas(&inner);
                if groups.len() == 2
                    && groups[0].len() == 1
                    && matches!(
                        groups[0][0].class.as_str(),
                        "PPI::Token::Regexp::Match" | "PPI::Token::QuoteLike::Regexp"
                    )
                {
                    if let Some(regex) = render_perl_regex_literal(groups[0][0]) {
                        let list_expr = self.render_expr_sequence(&groups[1]);
                        parts.push(format!("{}.split({regex})", paren_wrap(&list_expr)));
                        i += 2;
                        continue;
                    }
                }
            }
            if tokens[i].class == "PPI::Token::Word"
                && matches!(tokens[i].content.as_deref(), Some("get") | Some("post"))
                && tokens.get(i + 1).map(|t| t.class.as_str()) == Some("PPI::Structure::List")
                && tokens
                    .get(i + 2)
                    .map(|t| is_operator(t, "->"))
                    .unwrap_or(false)
                && tokens
                    .get(i + 3)
                    .map(|t| is_word(t, "result"))
                    .unwrap_or(false)
            {
                let name = tokens[i].content.as_deref().unwrap();
                let args = if name == "post" {
                    self.render_post_call_args(tokens[i + 1])
                } else {
                    self.render_call_args(tokens[i + 1])
                };
                if parts.last().map(|p| p == ".").unwrap_or(false) {
                    parts.pop();
                }
                let receiver = join_parts(&parts);
                parts.clear();
                self.used_await = true;
                parts.push(format!(
                    "(await {receiver}.{name}({})).result",
                    args.join(", ")
                ));
                i += 4;
                continue;
            }
            // `&subname(...)` — Perl's explicit-call-with-`&`-sigil syntax (bypasses prototype
            // checking; legacy plugin code uses it purely as a stylistic "this is a sub call, not
            // a builtin" marker, verified across the corpus — never for its other, rarer effect of
            // reusing the *caller's* own `@_` when parens are omitted, which isn't this shape).
            // PPI tokenizes the whole `&subname` as a *single* `PPI::Token::Symbol` (content
            // `"&subname"`, verified against real `perl -MPPI::Dumper` output) rather than a
            // `Cast('&')` + `Word` pair — a shape `try_match_call` below never recognizes at all
            // (it requires `tokens[i].class == "PPI::Token::Word"`), so a call written this way
            // fell through to the generic `Symbol` renderer (`strip_sigil` on `"&subname"`,
            // i.e. just `subname` with no `()` at all — wait, that can't be right either) — in
            // practice it *did* still call the function (the following `Structure::List` renders
            // as its own token right after), but crucially never went through `render_named_call`,
            // so `known_async_subs`' `await`-insertion (this whole mechanism's reason for being)
            // never triggered for it. Every real call site in the corpus is a plain call, so
            // this maps straight onto the same `render_named_call` path a bare `subname(...)`
            // already uses.
            if tokens[i].class == "PPI::Token::Symbol"
                && tokens[i]
                    .content
                    .as_deref()
                    .is_some_and(|c| c.starts_with('&'))
                && tokens.get(i + 1).map(|t| t.class.as_str()) == Some("PPI::Structure::List")
            {
                let name = tokens[i]
                    .content
                    .as_deref()
                    .unwrap()
                    .trim_start_matches('&');
                let args = self.render_call_args(tokens[i + 1]);
                parts.push(self.render_named_call(name, &args));
                i += 2;
                continue;
            }
            // `grep { COND } LIST` / `map { EXPR } LIST` — Perl's block-form syntax (as opposed
            // to the parenthesized `grep(COND, LIST)` form `render_named_call`'s own "grep" case
            // handles). No comma or parens separate the block from `LIST` at all, so unlike every
            // other special case here this one claims *every remaining token* in this slice as
            // `LIST` — verified safe against the real corpus (`Metadata/RegexParse.pm`'s four
            // uses), where a block-form grep/map is always already the entire contents of its own
            // isolated expression group (an argument by itself, a full RHS, or a full return
            // value) by the time it reaches this function, never followed by sibling tokens at
            // this same level. `COND`/`EXPR` implicitly binds each `LIST` element to `$_` —
            // `render_magic` already maps every bare `$_` reference to `it` (see its own docs),
            // so the block's rendered body just needs wrapping in a real `(it) => (...)` — same
            // reasoning as the parenthesized form's own wrapping.
            if tokens[i].class == "PPI::Token::Word"
                && matches!(tokens[i].content.as_deref(), Some("grep") | Some("map"))
                && tokens.get(i + 1).map(|t| t.class.as_str()) == Some("PPI::Structure::Block")
                && i + 2 < tokens.len()
            {
                let method = if tokens[i].content.as_deref() == Some("grep") {
                    "filter"
                } else {
                    "map"
                };
                let body = self.render_children_expr(tokens[i + 1]);
                // Strip a trailing statement-terminating `;` — `tokens` here can be an entire
                // statement's flattened children (e.g. `@tags = grep { ... } @tags;`, where this
                // whole slice reaches `render_expr_sequence` as the statement's RHS), not always
                // an already-isolated expression group.
                let list_tokens: Vec<&PpiNode> = tokens[i + 2..]
                    .iter()
                    .copied()
                    .filter(|t| {
                        !(t.class == "PPI::Token::Structure" && t.content.as_deref() == Some(";"))
                    })
                    .collect();
                let list_expr = self.render_expr_sequence(&list_tokens);
                parts.push(format!(
                    "{}.{method}((it) => ({body}))",
                    paren_wrap(&list_expr)
                ));
                i = tokens.len();
                continue;
            }
            // `sub (PARAMS) { BODY }` / `sub { BODY }` used as a *value* — e.g. a callback
            // argument like `$ua->on(start => sub ($ua, $tx) {...})` — as opposed to a top-level
            // named `sub name { ... }` declaration, which `find_subs`/`render_sub` handle
            // entirely separately and never reach this per-expression path. Must run before
            // `try_match_call` below: that function requires `tokens[i].class ==
            // "PPI::Token::Word"` with no special-casing for the bareword `sub`, so without this
            // check it would misparse `sub` as a *called* function named `sub`, with the
            // signature/prototype structure that follows misread as that "call"'s argument list
            // and the actual body (`PPI::Structure::Block`) left as an inert, never-rendered
            // trailing token (surfaced as an "unhandled token class: PPI::Structure::Block"
            // warning with nothing usable emitted in its place).
            if is_word(tokens[i], "sub") {
                let mut j = i + 1;
                // A *named* sub's modern signature is `PPI::Structure::Signature`; an anonymous
                // sub's `sub ($a, $b) {...}` parameter list is, verified against real
                // `perl -MPPI::Dumper` output, a plain `PPI::Structure::List` instead (PPI doesn't
                // recognize it as a signature at all without a name attached) — `parse_signature_params`
                // itself doesn't care which class it's handed, only that `.children()` holds a
                // comma-separated run of `$var` symbols, so the same function covers both shapes.
                let params = match tokens.get(j).map(|t| t.class.as_str()) {
                    Some("PPI::Structure::Signature") | Some("PPI::Structure::List") => {
                        let p = parse_signature_params(tokens[j]);
                        j += 1;
                        Some(p)
                    }
                    _ => None,
                };
                if tokens.get(j).map(|t| t.class.as_str()) == Some("PPI::Structure::Block") {
                    let block = tokens[j];
                    let outer_used_await = self.used_await;
                    self.used_await = false;
                    let body = self.render_statement_list(block.children());
                    let body = self.with_match_decl(body);
                    let closure_is_async = self.used_await;
                    // An `await` inside this closure's body still needs its enclosing named sub
                    // marked `async` too (the closure isn't hoisted out into its own separate
                    // top-level function), so the flag propagates outward rather than being
                    // reset — it just doesn't retroactively make an *unrelated* outer `await`
                    // disappear either.
                    self.used_await = outer_used_await || closure_is_async;
                    let keyword = if closure_is_async { "async " } else { "" };
                    let params_list = match &params {
                        Some(p) if !p.is_empty() => self.render_signature_params(p),
                        _ => "...args: any[]".to_string(),
                    };
                    parts.push(format!("{keyword}({params_list}) => {{\n{body}}}"));
                    i = j + 1;
                    continue;
                }
            }
            if let Some((rendered, consumed)) = self.try_match_call(tokens, i) {
                parts.push(rendered);
                i += consumed;
                continue;
            }
            if let Some((rendered, consumed)) = try_match_ref_eq(tokens, i) {
                parts.push(rendered);
                i += consumed;
                continue;
            }
            if let Some((rendered, consumed)) = self.try_match_legacy_http_constructor(tokens, i) {
                parts.push(rendered);
                i += consumed;
                continue;
            }
            // `->` immediately before a `{...}`/`[...]` subscript needs no connecting dot at all
            // (`$hash->{"Title"}` → `hash["Title"]`, not `hash.["Title"]`) — only `->` before a
            // bareword/method-call needs the dot `render_operator` normally produces.
            if tokens[i].class == "PPI::Token::Operator"
                && tokens[i].content.as_deref() == Some("->")
                && tokens.get(i + 1).map(|t| t.class.as_str()) == Some("PPI::Structure::Subscript")
            {
                i += 1;
                continue;
            }
            // `$_[N]` — Perl's positional-arg idiom (PPI tokenizes this as a `$_` Magic token
            // immediately followed by a `[N]` Subscript, not one atomic token) — means "the Nth
            // element of `@_`", i.e. `args[N]`, same array `shift`/`my (...) = @_` already read
            // from elsewhere in this renderer.
            if tokens[i].class == "PPI::Token::Magic"
                && tokens[i].content.as_deref() == Some("$_")
                && tokens.get(i + 1).map(|t| t.class.as_str()) == Some("PPI::Structure::Subscript")
            {
                let index = self.render_children_expr(tokens[i + 1]);
                parts.push(format!("args[{index}]"));
                i += 2;
                continue;
            }
            // `defined $x` (Perl's "named unary operator" form — no parens, binds to just the
            // next single term) — the parenthesized form `defined($x)` is already handled by
            // `try_match_call`'s generic `Word` + `Structure::List` case; this covers the other
            // spelling, which that pattern doesn't match since there's no `Structure::List` here.
            if is_word(tokens[i], "defined") {
                if let Some(operand) = tokens.get(i + 1) {
                    let rendered_operand = self.render_token(operand);
                    parts.push(format!(
                        "({rendered_operand} !== undefined && {rendered_operand} !== null)"
                    ));
                    i += 2;
                    continue;
                }
            }
            // `exists $hash->{key}` (same named-unary-operator shape as `defined` above, but the
            // operand is routinely a full `$x->{a}->{b}` chain, not a single token — consuming
            // just `tokens[i + 1]` the way `defined` does would leave the rest of the chain
            // dangling as separate, un-joined tokens). JS has no direct `exists`-on-a-property
            // equivalent that also tolerates an intermediate `undefined` in the chain without
            // throwing, so this renders to `EXPR !== undefined` on the *whole* rendered chain —
            // not a perfect match (Perl's `exists` is true even for a key whose value is
            // explicitly `undef`, this treats "exists" and "is defined" as the same thing), but
            // every real call site in the legacy plugin corpus uses `exists` purely to guard
            // against a missing key before reading it, where the two are equivalent in practice.
            if is_word(tokens[i], "exists") && tokens.get(i + 1).is_some() {
                let end = self.chain_end(tokens, i + 1);
                let operand = self.render_expr_sequence(&tokens[i + 1..end]);
                parts.push(format!("({operand} !== undefined)"));
                i = end;
                continue;
            }
            // `lc EXPR` / `uc EXPR` (Perl's named-unary-operator, no-parens form — the
            // parenthesized `lc(EXPR)`/`uc(EXPR)` form is already handled by `try_match_call`'s
            // generic `Word` + `Structure::List` case via `render_named_call`). Same reasoning as
            // `exists`/`defined` above: without this, `lc $x->{key}` fell through to
            // `render_word`'s generic mapping (`_ => word.to_string()`, i.e. just the bareword
            // `lc` emitted literally), which is a syntax error in the output (`lc` isn't a
            // function JS knows), not merely a missed optimization.
            if matches!(tokens[i].content.as_deref(), Some("lc") | Some("uc"))
                && tokens[i].class == "PPI::Token::Word"
                && tokens.get(i + 1).is_some()
                && tokens.get(i + 1).map(|t| t.class.as_str()) != Some("PPI::Structure::List")
            {
                let method = if tokens[i].content.as_deref() == Some("lc") {
                    "toLowerCase"
                } else {
                    "toUpperCase"
                };
                let end = self.chain_end(tokens, i + 1);
                let operand = self.render_expr_sequence(&tokens[i + 1..end]);
                parts.push(format!("{}.{method}()", paren_wrap(&operand)));
                i = end;
                continue;
            }
            // `ref EXPR` (Perl's no-parens named-unary-operator form — `ref(EXPR)` is handled by
            // `try_match_call`'s generic "Word + `Structure::List`" case via `render_named_call`
            // instead) — same shape as `lc`/`uc` above, just routed through `legacyCompat.refType`
            // (see its own docs) instead of a native JS method. Covers both a value position
            // (`my $t = ref $x;`) and a bare boolean-truthiness position (`return if ref $x;`,
            // which works unmodified in JS too — `""` is falsy, a non-empty type name isn't,
            // exactly matching Perl's own string-truthiness rule here).
            if is_word(tokens[i], "ref")
                && tokens.get(i + 1).is_some()
                && tokens.get(i + 1).map(|t| t.class.as_str()) != Some("PPI::Structure::List")
            {
                let end = self.chain_end(tokens, i + 1);
                let operand = self.render_expr_sequence(&tokens[i + 1..end]);
                parts.push(format!("legacyCompat.refType({operand})"));
                i = end;
                continue;
            }
            // `chomp $x;` / `unlink $x;` / `close $x;` / `from_json $x;` — Perl's no-parens
            // named-unary-operator call form for functions that already have a parenthesized
            // mapping in `render_named_call` (same reasoning as `lc`/`uc`/`ref` above: without
            // this, e.g. `chomp $row;` — part of the shared file-read idiom's loop body in every
            // sidecar-metadata plugin, see `try_render_readline_while` — fell through to
            // `render_word`'s generic bareword mapping, emitting the syntactically invalid
            // `chomp row;`).
            if tokens[i].class == "PPI::Token::Word"
                && matches!(
                    tokens[i].content.as_deref(),
                    Some("chomp")
                        | Some("unlink")
                        | Some("close")
                        | Some("from_json")
                        | Some("to_json")
                        | Some("decode_json")
                        | Some("encode_json")
                        | Some("trim")
                        | Some("scalar")
                )
                && tokens.get(i + 1).is_some()
                && tokens.get(i + 1).map(|t| t.class.as_str()) != Some("PPI::Structure::List")
            {
                let name = tokens[i].content.as_deref().unwrap().to_string();
                let end = self.chain_end(tokens, i + 1);
                let operand = self.render_expr_sequence(&tokens[i + 1..end]);
                parts.push(self.render_named_call(&name, &[operand]));
                i = end;
                continue;
            }
            // `sort LIST` / `sort keys HASH` / `keys HASH` / `values HASH` — Perl's no-parens
            // list-operator forms (the parenthesized forms — `sort(LIST)`, `keys(HASH)` — already
            // work via `try_match_call`'s generic "Word + `Structure::List`" case through
            // `render_named_call`). `sort keys HASH` is special-cased as its own two-word combo
            // (`Object.keys(HASH).sort()`) rather than trying to make this generically chain two
            // no-parens operators back to back — the only shape real `Metadata/Eze.pm`/
            // `GalleryDL.pm` actually use, and correctly chaining `chain_end` through an inner
            // operator's *own* no-parens operand isn't otherwise needed anywhere in this corpus.
            // Perl's `sort` (no comparator block) does a plain string sort without mutating its
            // argument, matching JS's own default `.sort()` comparator closely enough — spread
            // into a fresh array first so the *non*-mutating part matches too.
            if tokens[i].class == "PPI::Token::Word"
                && matches!(
                    tokens[i].content.as_deref(),
                    Some("sort") | Some("keys") | Some("values")
                )
                && tokens.get(i + 1).is_some()
                && tokens.get(i + 1).map(|t| t.class.as_str()) != Some("PPI::Structure::List")
            {
                let name = tokens[i].content.as_deref().unwrap();
                if name == "sort" && tokens.get(i + 1).is_some_and(|t| is_word(t, "keys")) {
                    let end = self.chain_end(tokens, i + 2);
                    let operand = self.render_expr_sequence(&tokens[i + 2..end]);
                    parts.push(format!("Object.keys({operand}).sort()"));
                    i = end;
                    continue;
                }
                let end = self.chain_end(tokens, i + 1);
                let operand = self.render_expr_sequence(&tokens[i + 1..end]);
                let rendered = match name {
                    "sort" => format!("[...{operand}].sort()"),
                    "keys" => format!("Object.keys({operand})"),
                    _ => format!("Object.values({operand})"),
                };
                parts.push(rendered);
                i = end;
                continue;
            }
            // `-e`/`-f`/`-d`/... Perl's filesystem-test operators — no JS equivalent under this
            // host's design at all: plugins never get real filesystem access (constitution
            // Principle IV), only whatever specific sidecar-file *contents* the host pre-fetches
            // from *inside* the current archive (see `is_file_in_archive`'s own docs above). Every
            // real use in the legacy plugin corpus is a fallback path checking for a metadata file
            // sitting *next to* the archive on disk rather than embedded in it — a capability this
            // host deliberately doesn't support, so this always evaluates to `false` (the operand
            // is still rendered and discarded, in case it has side effects, though none of the
            // real corpus's own operands do) rather than leaving obviously-invalid JS in the
            // output.
            if tokens[i].class == "PPI::Token::Operator"
                && tokens[i].content.as_deref().is_some_and(|c| {
                    c.len() == 2 && c.starts_with('-') && c.as_bytes()[1].is_ascii_alphabetic()
                })
            {
                let op = tokens[i].content.as_deref().unwrap();
                self.warnings.push(format!(
                    "file-test operator {op} always evaluates to false — this host never gives plugins real filesystem access outside the current archive"
                ));
                let end = self.chain_end(tokens, i + 1);
                i = end;
                parts.push("false".to_string());
                continue;
            }
            // Reference-taking `\EXPR` (`PPI::Token::Cast` with content `\`) — JS arrays/hashes
            // are already reference types, so taking "a reference" is a no-op here; render just
            // the operand. Must come before the generic sigil-cast handling in `render_token`,
            // which only knows what to do with `@$x`-style sigil casts, not `\`.
            if tokens[i].class == "PPI::Token::Cast" && tokens[i].content.as_deref() == Some("\\") {
                i += 1;
                continue;
            }
            // Block-form dereference `@{ EXPR }` / `%{ EXPR }` / `${ EXPR }` (a sigil `Cast`
            // token followed by a `Structure::Block`) — again a no-op in JS, since whatever
            // `EXPR` evaluates to is already the real array/hash object; render just its content.
            if tokens[i].class == "PPI::Token::Cast"
                && tokens.get(i + 1).map(|t| t.class.as_str()) == Some("PPI::Structure::Block")
            {
                parts.push(self.render_statement_list_as_expr(tokens[i + 1]));
                i += 2;
                continue;
            }
            // `@$ref` / `%$ref` / `$$ref` (a sigil `Cast` directly on a `Symbol`, no braces) —
            // same no-op rationale: dereferencing is meaningless once sigils are gone, so this
            // is just `ref` itself. Without this, the `Cast` alone would render as an empty
            // string (see `render_token`), leaving a stray extra space where it used to be.
            if tokens[i].class == "PPI::Token::Cast"
                && tokens.get(i + 1).map(|t| t.class.as_str()) == Some("PPI::Token::Symbol")
            {
                parts.push(self.render_token(tokens[i + 1]));
                i += 2;
                continue;
            }
            // A bareword immediately before `=>` is auto-quoted by Perl itself (`start => sub
            // {...}` means the same as `'start' => sub {...}`) — outside an actual hash-literal
            // context (`render_key_value_pairs` handles that one separately), this still applies
            // to any fat-comma-separated positional call args like `$ua->on(start => sub {...})`.
            // Without this, the bareword rendered as a plain (and here undeclared) JS identifier
            // via `render_token`'s generic `Word` case instead of the string literal Perl means.
            if tokens[i].class == "PPI::Token::Word"
                && tokens
                    .get(i + 1)
                    .map(|t| is_operator(t, "=>"))
                    .unwrap_or(false)
            {
                let word = tokens[i].content.as_deref().unwrap_or("");
                parts.push(format!("{word:?}"));
                i += 1;
                continue;
            }
            parts.push(self.render_token(tokens[i]));
            i += 1;
        }
        tidy_spacing(&join_parts(&parts))
    }

    /// Renders a `Structure::Block`'s contents as a single expression — used only for the
    /// block-form dereference case above (`@{ EXPR }` etc.), where the block holds exactly one
    /// expression-statement rather than a sequence of executable statements.
    fn render_statement_list_as_expr(&mut self, block: &PpiNode) -> String {
        self.render_children_expr(block)
    }

    /// Records `name` in `uri_typed_vars` if `rendered_rhs` is exactly what
    /// `try_match_legacy_http_constructor`'s `URI`→`URL` mapping produces (`"undefined"` or a
    /// `"new URL(...)"` call) — see that field's own docs for why this tracking exists at all.
    /// Takes the *already-rendered* RHS string (not raw tokens) since every call site here already
    /// has one in hand from its own real render, avoiding a second, side-effecting
    /// `render_expr_sequence` call just to check.
    fn track_uri_typed_var(&mut self, name: &str, rendered_rhs: &str) {
        if rendered_rhs == "undefined" || rendered_rhs.starts_with("new URL(") {
            self.uri_typed_vars.insert(name.to_string());
        }
    }

    /// `Mojo::UserAgent->new` / `Mojo::UserAgent->new()` and `Mojo::Cookie::Response->new(k => v,
    /// ...)` — the two legacy-plugin-corpus idioms that aren't just vocabulary substitution:
    /// `Mojo::UserAgent` is a real CPAN HTTP client with no JS equivalent at all (routed through
    /// `legacyCompat.userAgent()`, a `fetch()`-backed shim — see `dispatcher.ts`), and
    /// `Mojo::Cookie::Response->new(...)` is Mojo's hash-literal-shaped constructor call for a
    /// plain data record (`{name, value, domain, path}`) that `legacyCompat.userAgent()`'s
    /// `cookie_jar.add` expects as a plain object, not a real class instance — so this renders
    /// straight to `{ name: ..., value: ..., domain: ..., path: ... }` rather than `new
    /// Mojo::Cookie::Response(...)` (which wouldn't even be valid JS: `::` isn't a legal
    /// identifier character). Perl package names with other `::`-qualified classes aren't handled
    /// here — only these two concrete, verified-in-the-corpus idioms.
    fn try_match_legacy_http_constructor(
        &mut self,
        tokens: &[&PpiNode],
        i: usize,
    ) -> Option<(String, usize)> {
        let word = tokens[i];
        if word.class != "PPI::Token::Word" {
            return None;
        }
        let name = word.content.as_deref()?;
        if name != "Mojo::UserAgent"
            && name != "Mojo::Cookie::Response"
            && name != "Mojo::DOM"
            && name != "URI"
        {
            return None;
        }
        if !is_operator(tokens.get(i + 1)?, "->") {
            return None;
        }
        if !is_word(tokens.get(i + 2)?, "new") {
            return None;
        }
        let has_list = tokens.get(i + 3).map(|t| t.class.as_str()) == Some("PPI::Structure::List");

        if name == "Mojo::UserAgent" {
            let consumed = if has_list { 4 } else { 3 };
            return Some(("legacyCompat.userAgent()".to_string(), consumed));
        }

        // `URI->new()` (bare placeholder, real corpus case: `Download/EHentai.pm`'s `my $finalURL
        // = URI->new();`, immediately overwritten by a real `URI->new($1)` a few lines later once
        // a URL is actually known — never used bare beyond that point) / `URI->new($expr)` (a
        // real, immediately-usable URL) — `URI` is a real CPAN module with a genuine, if partial,
        // JS equivalent (the standard `URL` class), unlike `Mojo::UserAgent`, which has none at
        // all. But `new URL()` with *no* argument throws (`TypeError: Invalid URL`) rather than
        // constructing an empty placeholder the way Perl's bare `URI->new()` does — so the
        // zero-arg call maps to `undefined` instead (matching every later comparison against it
        // in the real corpus, e.g. `$finalURL eq ""`: a real `URI` object reference is never
        // `eq` to the empty string either, so both `undefined === ""` here and Perl's own
        // `eq ""` there are equally always-false checks — this preserves that pre-existing
        // legacy behavior rather than "fixing" it). `URI->new(EXPR)` maps directly to `new
        // URL(EXPR)`. Any further method call on the result (`->query(...)`, `->as_string`) needs
        // its own separate mapping — see `try_match_uri_method_call`, since those happen at a
        // different token position (after this whole `->new(...)` chain has already been
        // consumed here).
        if name == "URI" {
            let consumed = if has_list { 4 } else { 3 };
            // `has_list` alone doesn't distinguish `URI->new()` (empty parens — still a real
            // `Structure::List` node, just with nothing inside it) from `URI->new` (no parens at
            // all) — both need the same `undefined` treatment, so this checks the list's actual
            // contents rather than just whether a list node is present at all (an earlier version
            // of this match wrongly treated `URI->new()`'s empty list the same as a real
            // one-argument call, fell through to the one-argument check below, found zero
            // arguments instead of one, and silently returned `None` — leaving `URI->new()`
            // completely unhandled and unwarned-about despite this whole match supposedly
            // covering it).
            let args = if has_list {
                self.render_call_args(tokens[i + 3])
            } else {
                Vec::new()
            };
            return match args.as_slice() {
                [] => Some(("undefined".to_string(), consumed)),
                [arg] => Some((format!("new URL({arg})"), consumed)),
                _ => None, // some other arg shape this converter doesn't specifically handle.
            };
        }

        // `Mojo::DOM->new(html)` (parses immediately) / `Mojo::DOM->new->xml(1)->parse(xml_str)`
        // (constructs empty, flips XML mode, then parses) — both end up calling
        // `legacyCompat.parseHtml(markup, xml)`; only the argument source and the `xml` flag differ.
        // Verified against every real call site in the legacy plugin corpus: `Mojo::DOM->new`
        // is always immediately followed by either `(markup)` or `->xml(1)->parse(markup)`, never
        // used bare (e.g. built once and parsed later) or with `->xml(0)`/no `xml` call at all in
        // a way this wouldn't already cover (the default un-called state is HTML mode, matching
        // `xml: false` being the parameter default here too).
        if name == "Mojo::DOM" {
            if has_list {
                // `Mojo::DOM->new(html)`
                let list = tokens[i + 3];
                let args = self.render_call_args(list);
                if args.len() == 1 {
                    return Some((format!("legacyCompat.parseHtml({})", args[0]), 4));
                }
                return None;
            }
            // `Mojo::DOM->new->xml(1)->parse(xml_str)` — a `Structure::List` may or may not
            // follow the bare `->new` (both `Mojo::DOM->new` and `Mojo::DOM->new()` are valid
            // Perl); account for either before expecting `->xml(...)`.
            let after_new =
                if tokens.get(i + 3).map(|t| t.class.as_str()) == Some("PPI::Structure::List") {
                    i + 4
                } else {
                    i + 3
                };
            if !is_operator(tokens.get(after_new)?, "->") {
                return None;
            }
            if !is_word(tokens.get(after_new + 1)?, "xml") {
                return None;
            }
            let xml_list = tokens.get(after_new + 2)?;
            if xml_list.class != "PPI::Structure::List" {
                return None;
            }
            let xml_flag_tokens = self.flatten_statement_children(xml_list.children());
            let xml_on = xml_flag_tokens
                .first()
                .map(|t| t.content.as_deref() != Some("0"))
                .unwrap_or(true);

            let after_xml = after_new + 3;
            if !is_operator(tokens.get(after_xml)?, "->") {
                return None;
            }
            if !is_word(tokens.get(after_xml + 1)?, "parse") {
                return None;
            }
            let parse_list = tokens.get(after_xml + 2)?;
            if parse_list.class != "PPI::Structure::List" {
                return None;
            }
            let parse_args = self.render_call_args(parse_list);
            if parse_args.len() != 1 {
                return None;
            }
            let xml_arg = if xml_on { "true" } else { "false" };
            return Some((
                format!("legacyCompat.parseHtml({}, {xml_arg})", parse_args[0]),
                after_xml + 3 - i,
            ));
        }

        // Mojo::Cookie::Response->new(...) always takes its fields as a hash-literal-shaped
        // argument list in the legacy corpus — no bare `->new` / `->new()` form to worry about.
        let list = tokens.get(i + 3)?;
        if list.class != "PPI::Structure::List" {
            return None;
        }
        let flat = self.flatten_statement_children(list.children());
        let fields = self.render_key_value_pairs(&flat)?;
        Some((format!("{{ {} }}", fields.join(", ")), 4))
    }

    /// `NAME(ARGS)` (paren call form) and `NAME ARGS` (bareword call form, e.g. `push @a, $x`) —
    /// covers both Perl builtins with special JS mappings (`push`/`join`/`split`/`die`/`defined`)
    /// and ordinary sub calls (which just become `name(args)` since every sub in the file is
    /// being converted to a same-named TS function).
    fn try_match_call(&mut self, tokens: &[&PpiNode], i: usize) -> Option<(String, usize)> {
        let word = tokens[i];
        if word.class != "PPI::Token::Word" {
            return None;
        }
        let raw_name = word.content.as_deref()?;
        // A fully package-qualified call (`LANraragi::Utils::Database::get_tags(...)`) — same
        // "no JS equivalent, but `::` is a syntax error inside a JS identifier" situation
        // `render_token`'s own `::`-sanitizing arm handles for a bare (non-call) reference, just
        // reached via a different path here: `try_match_call` runs *before* `render_token`'s
        // per-token fallback ever sees this word, so without this it emitted the raw `::` straight
        // into `render_named_call`'s generic `name(args)` fallback (real corpus case:
        // `Metadata/CopyArchiveTags.pm`'s `LANraragi::Utils::Database::get_tags($lrr_gid)`),
        // producing invalid JS deno's parser rejected outright rather than the usual "unresolved
        // name" TS error.
        let sanitized;
        let name = if raw_name.contains("::") {
            self.warnings.push(format!(
                "external Perl module reference has no JS equivalent: {raw_name}"
            ));
            sanitized = raw_name.replace("::", "_");
            sanitized.as_str()
        } else {
            raw_name
        };

        if tokens.get(i + 1).map(|t| t.class.as_str()) == Some("PPI::Structure::List") {
            let list = tokens[i + 1];
            let args = self.render_call_args(list);
            return Some((self.render_named_call(name, &args), 2));
        }

        // Bareword-form builtin calls without parens, e.g. `push @arr, $x;` or `die "msg";` —
        // consume the rest of the statement as the argument list (this only runs at the top of
        // an expression sequence in practice, since these are statement-level idioms). Excludes
        // the statement's terminal `;`, which would otherwise render as a literal semicolon
        // glued onto the last argument's own text.
        if matches!(name, "push" | "die" | "return") && i == 0 {
            let rest: Vec<&PpiNode> = tokens[i + 1..]
                .iter()
                .copied()
                .filter(|t| {
                    !(t.class == "PPI::Token::Structure" && t.content.as_deref() == Some(";"))
                })
                .collect();
            let groups = split_on_commas(&rest);
            let args: Vec<String> = groups
                .iter()
                .map(|g| self.render_expr_sequence(g))
                .collect();
            return Some((self.render_named_call(name, &args), tokens.len()));
        }

        None
    }

    /// Given `start` pointing at the first token of an operand for a no-parens named-unary
    /// operator (`exists`/`lc`/`uc`, ...), returns the index just past the end of the full
    /// `base-term (-> Subscript)*` chain — e.g. `$x->{a}->{b}` (an `Operator("->")` +
    /// `Structure::Subscript` pair repeated) or `$x->{a}{b}` (PPI omits the `->` between
    /// consecutive subscripts but still emits back-to-back `Structure::Subscript`s for it). Shared
    /// by every no-parens named-unary-operator special case so they all consume exactly the same
    /// operand shape a real Perl parser would bind the operator to.
    ///
    /// The base term itself may be more than one token — verified against real
    /// `perl -MPPI::Dumper` output on `lc @$data[0]->{"category"}`: `@$data` alone is a
    /// `Cast('@')` immediately followed by `Symbol('$data')`, two tokens for what's really one
    /// term. Treating `start` as if it were always a single-token term (this function's own
    /// earlier bug) truncated the operand after just the `Cast`, corrupting everything downstream
    /// in the rendered expression. A leading sigil `Cast` is skipped so `end` lands on the actual
    /// base token before the subscript-chain scan below begins.
    fn chain_end(&self, tokens: &[&PpiNode], start: usize) -> usize {
        let mut end = start + 1;
        if tokens.get(start).map(|t| t.class.as_str()) == Some("PPI::Token::Cast")
            && tokens.get(end).map(|t| t.class.as_str()) == Some("PPI::Token::Symbol")
        {
            end += 1;
        }
        loop {
            let at_arrow_subscript = tokens
                .get(end)
                .map(|t| is_operator(t, "->"))
                .unwrap_or(false)
                && tokens.get(end + 1).map(|t| t.class.as_str())
                    == Some("PPI::Structure::Subscript");
            let at_bare_subscript =
                tokens.get(end).map(|t| t.class.as_str()) == Some("PPI::Structure::Subscript");
            if at_arrow_subscript {
                end += 2;
            } else if at_bare_subscript {
                end += 1;
            } else {
                break;
            }
        }
        end
    }

    /// `$ua->post($url => form => {...})` / `$ua->post($url => json => {...})` — Mojo::UserAgent's
    /// own fat-comma-separated "content-type hint literal" calling convention (verified against
    /// every real `->post(...)` call site in the legacy plugin corpus): the middle argument is
    /// always the bareword `form` or `json`, never a real expression, telling `post` how to
    /// encode the third (data) argument. Rendered through the generic `render_call_args` path,
    /// that bareword falls through to `render_word`'s catch-all (`_ => word.to_string()`) and
    /// comes out as a bare, undeclared JS identifier reference — this instead forces it to a
    /// string literal, matching `LegacyUserAgent.post`'s own `kind: "form" | "json"` parameter
    /// (see `dispatcher.ts`).
    fn render_post_call_args(&mut self, list: &PpiNode) -> Vec<String> {
        let inner: Vec<&PpiNode> = list
            .children()
            .iter()
            .filter(|c| c.class != "PPI::Token::Whitespace")
            .flat_map(|c| {
                if c.class == "PPI::Statement" || c.class == "PPI::Statement::Expression" {
                    self.real_children(c.children())
                } else {
                    vec![c]
                }
            })
            .collect();
        // Splits on *both* `,` and `=>` — `render_call_args`'s own `split_on_commas` deliberately
        // only recognizes plain `,` (its other callers, e.g. `render_key_value_pairs`, need `=>`
        // left intact within a group so they can find and interpret it themselves as a key/value
        // separator). This call's three arguments are fat-comma-separated (`$url => kind =>
        // {data}`), not real hash pairs, so splitting on `=>` here is correct and doesn't collide
        // with that other meaning.
        let groups = split_on_commas_and_fat_commas(&inner);
        groups
            .iter()
            .enumerate()
            .map(|(idx, group)| {
                if idx == 1 && group.len() == 1 && group[0].class == "PPI::Token::Word" {
                    format!("{:?}", group[0].content.as_deref().unwrap_or(""))
                } else {
                    self.render_expr_sequence(group)
                }
            })
            .collect()
    }

    fn render_call_args(&mut self, list: &PpiNode) -> Vec<String> {
        let inner: Vec<&PpiNode> = list
            .children()
            .iter()
            .filter(|c| c.class != "PPI::Token::Whitespace")
            .flat_map(|c| {
                if c.class == "PPI::Statement" || c.class == "PPI::Statement::Expression" {
                    self.real_children(c.children())
                } else {
                    vec![c]
                }
            })
            .collect();
        split_on_commas(&inner)
            .iter()
            .map(|g| self.render_expr_sequence(g))
            .collect()
    }

    fn render_named_call(&mut self, name: &str, args: &[String]) -> String {
        match name {
            "push" if args.len() >= 2 => {
                format!("{}.push({})", paren_wrap(&args[0]), args[1..].join(", "))
            }
            "join" if args.len() == 2 => format!("{}.join({})", paren_wrap(&args[1]), args[0]),
            "split" if args.len() == 2 => format!("{}.split({})", paren_wrap(&args[1]), args[0]),
            "die" => format!("throw new Error({})", args.join(" + ")),
            "defined" if args.len() == 1 => {
                let operand = paren_wrap(&args[0]);
                format!("({operand} !== undefined && {operand} !== null)")
            }
            // Perl's `chomp($x)` mutates `$x` in place; JS can't do that through a function call,
            // so the reassignment has to happen at the call site. Routed through the shared
            // `legacyCompat.chomp` (see `dispatcher.ts`) rather than inlined, both for the fix to
            // live in one place and because Perl's `chomp` only strips a trailing `"\n"` (its
            // `$/` record separator), not `"\r\n"` — an easy detail to get subtly wrong inline.
            "chomp" if args.len() == 1 => {
                let operand = paren_wrap(&args[0]);
                format!("{operand} = legacyCompat.chomp({operand})")
            }
            "lc" if args.len() == 1 => format!("{}.toLowerCase()", paren_wrap(&args[0])),
            "uc" if args.len() == 1 => format!("{}.toUpperCase()", paren_wrap(&args[0])),
            // `LANraragi::Utils::String::trim` — imported in 8 of the 19 Metadata plugins.
            "trim" if args.len() == 1 => {
                format!("legacyCompat.trim({})", args[0])
            }
            // `File::Basename::fileparse($path, qr/\.[^.]*/)` — see `legacyCompat.fileparse`'s own
            // docs (`dispatcher.ts`) for why the second argument is dropped rather than rendered.
            "fileparse" if args.len() == 2 => {
                format!("legacyCompat.fileparse({})", args[0])
            }
            "redis_decode" if args.len() == 1 => {
                format!("legacyCompat.redis_decode({})", args[0])
            }
            // Perl's `int` truncates toward zero (`int(-1.5) == -1`), same as `Math.trunc` (not
            // `Math.floor`, which would round the wrong way for negatives — a real behavior
            // difference the legacy corpus doesn't happen to exercise, but worth getting right
            // rather than by luck).
            "int" if args.len() == 1 => format!("Math.trunc({})", args[0]),
            // Perl's `rand($n)` returns a float in `[0, $n)`, matching `Math.random() * n`
            // exactly (`rand()` with no argument, i.e. `[0, 1)`, is just `Math.random()` — the
            // `* args[0]` below is skipped for that case since there's no arg to multiply by).
            "rand" if args.len() == 1 => format!("(Math.random() * {})", args[0]),
            "rand" if args.is_empty() => "Math.random()".to_string(),
            // Perl's `sleep($seconds)` blocks synchronously. JS has no synchronous sleep at all
            // (short of a busy-wait or `Atomics.wait`, neither appropriate here) — routed through
            // `legacyCompat.sleep` (a `setTimeout`-backed `Promise`, see `dispatcher.ts`) and
            // `await`-ed, which forces the enclosing sub to become `async` (see `used_await`'s own
            // docs — the same mechanism the `->get()`/`->post()` Mojo idiom already relies on).
            "sleep" if args.len() == 1 => {
                self.used_await = true;
                format!("await legacyCompat.sleep({})", args[0])
            }
            "length" if args.len() == 1 => format!("{}.length", paren_wrap(&args[0])),
            "scalar" if args.len() == 1 => format!("{}.length", paren_wrap(&args[0])),
            // `ref($x)` (parenthesized form — the no-parens `ref $x` form is handled by its own
            // check in `render_expr_sequence`, same reasoning as `lc`/`uc` there) — see
            // `legacyCompat.refType`'s own docs for exactly what's covered (hashref/arrayref
            // detection only, the only two kinds this corpus ever actually tests for).
            "ref" if args.len() == 1 => {
                format!("legacyCompat.refType({})", args[0])
            }
            "keys" if args.len() == 1 => format!("Object.keys({})", args[0]),
            "values" if args.len() == 1 => format!("Object.values({})", args[0]),
            // `is_file_in_archive($archive, "name.ext")` (`LANraragi::Utils::Archive`) — the
            // host pre-fetches every sidecar filename this plugin declared (see
            // `lib.rs::SIDECAR_FILE_RE`, which scans the source for exactly these calls to build
            // `pluginInfo().sidecar_files`) *before* the plugin ever runs, handing back whatever
            // it found as `hostArgs.sidecar_files: Record<string, string>` — content, not a path
            // (constitution Principle IV: the plugin itself never gets real filesystem access for
            // this). So this maps straight to a lookup rather than a real archive search; `args[0]`
            // (the archive-path expression) is unused since the host already scoped the search to
            // the current archive on its own.
            "is_file_in_archive" if args.len() == 2 => {
                // `hostArgs` is typed `Record<string, unknown>` (every entry point's parameter
                // shape), so `.sidecar_files` alone is `unknown` — cast to what the host actually
                // sends before indexing into it, or TS rejects the index access entirely.
                format!(
                    "(hostArgs.sidecar_files as Record<string, string> | undefined)?.[{}]",
                    args[1]
                )
            }
            // `extract_file_from_archive($archive, $path_in_archive)` — legacy extracts the file
            // found above to a temp path and hands that back; since the host already resolved the
            // *content* up front (see `is_file_in_archive` above), `$path_in_archive` here is
            // already that content, and "extracting" it is a no-op pass-through.
            "extract_file_from_archive" if args.len() == 2 => args[1].clone(),
            // `unlink($fh)` / `close($fh)` on the file handle from the `open(...)`/`<$fh>` idiom
            // (see `try_render_open_file_statement`/`try_render_readline_while`) — cleanup for a
            // real temp file legacy's own `extract_file_from_archive` created, which never exists
            // under this host's design (see above), so there's nothing left to clean up.
            "unlink" | "close" => String::new(),
            // `grep(COND, LIST)` implicitly binds each `LIST` element to `$_` while evaluating
            // `COND` (same as the block form `grep { COND } LIST`, just written with parens and a
            // leading comma instead) — `render_magic` already maps every other bare `$_` reference
            // to the JS identifier `it` (see its own docs), so `COND` — already rendered by this
            // point, `args[0]` — needs wrapping in `(it) => (...)` for that binding to actually
            // exist as a real parameter, rather than passing a bare boolean value where
            // `.filter()` expects a function.
            "grep" if args.len() == 2 => {
                format!("{}.filter((it) => ({}))", paren_wrap(&args[1]), args[0])
            }
            "index" if args.len() == 2 => {
                format!("{}.indexOf({})", paren_wrap(&args[0]), args[1])
            }
            "pop" if args.len() == 1 => format!("{}.pop()", paren_wrap(&args[0])),
            "unshift" if args.len() >= 2 => {
                format!("{}.unshift({})", paren_wrap(&args[0]), args[1..].join(", "))
            }
            // Perl's `reverse` returns a *new* reversed list; JS's `.reverse()` mutates the
            // array in place. Routed through `legacyCompat.reverse` (which spreads into a fresh
            // array first) rather than inlined, so the non-mutating fix lives in one place.
            "reverse" if args.len() == 1 => {
                format!("legacyCompat.reverse({})", args[0])
            }
            // Perl's `sprintf` has no native JS equivalent at all (unlike the above, which are
            // JS-native methods with a subtly different calling convention or mutation
            // behavior) — `legacyCompat.sprintf` covers the format-spec subset the legacy plugin
            // corpus actually uses (see `dispatcher.ts`), not Perl's full `sprintf` grammar.
            "sprintf" if !args.is_empty() => {
                format!("legacyCompat.sprintf({})", args.join(", "))
            }
            // `LANraragi::Utils::Logging::get_logger($name, $category)` writes to a real
            // rotating log file on disk — most converted plugins don't get filesystem write
            // access (constitution Principle IV's narrow `declared_permissions`), so
            // `legacyCompat.getLogger` logs to stderr instead (safe: `dispatcher.ts`'s stdout is
            // reserved for its own newline-delimited JSON protocol, and stderr doesn't share it).
            "get_logger" if args.len() == 2 => {
                format!("legacyCompat.getLogger({}, {})", args[0], args[1])
            }
            // `get_plugin_logger()` takes no arguments at all — at runtime it fills in the
            // logger's name via Perl's `caller` reflection plus the calling plugin's own
            // `plugin_info`. There's no equivalent reflection available in the converted TS, so
            // this resolves the same name at *conversion* time instead, from the same metadata
            // this converter already parsed (`Renderer::plugin_name`).
            "get_plugin_logger" if args.is_empty() => {
                let name = self.plugin_name.as_deref().unwrap_or("plugin");
                format!("legacyCompat.getLogger({name:?}, \"plugins\")")
            }
            // `URI::Escape::uri_escape_utf8` and JS's native `encodeURIComponent` reserve the
            // exact same "never escape" character set (`A-Za-z0-9-_.!~*'()`, verified against
            // real `perl -MURI::Escape -e 'print uri_escape_utf8(...)'` output across every ASCII
            // punctuation character) and both percent-encode multi-byte UTF-8 sequences byte-by-
            // byte in uppercase hex — a direct native mapping, no `legacyCompat` shim needed.
            "uri_escape_utf8" | "uri_escape" if args.len() == 1 => {
                format!("encodeURIComponent({})", args[0])
            }
            // `Mojo::JSON::decode_json`/`from_json` both parse a JSON string into a plain
            // Perl data structure — exactly what `JSON.parse` does; no `legacyCompat` shim needed.
            // (`Mojo::JSON::encode_json`/`to_json`, the encode direction, map the same way onto
            // `JSON.stringify` — included here even though the corpus scan that motivated this
            // pass only found the decode direction in actual use, since it's the same reasoning
            // and costs nothing extra to cover.)
            "decode_json" | "from_json" if args.len() == 1 => {
                format!("JSON.parse({})", args[0])
            }
            "encode_json" | "to_json" if args.len() == 1 => {
                format!("JSON.stringify({})", args[0])
            }
            // `Mojo::Util::html_unescape` decodes HTML entities (`&amp;` → `&`, `&#39;` → `'`,
            // etc.) — no native JS equivalent outside a real DOM (which this Deno sandbox doesn't
            // have), so routed through `legacyCompat.htmlUnescape` (see `dispatcher.ts`).
            "html_unescape" if args.len() == 1 => {
                format!("legacyCompat.htmlUnescape({})", args[0])
            }
            // A call to another sub in this same file, already known (from an earlier
            // fixed-point pass — see `known_async_subs`'s own docs) to render as `async`. Perl
            // itself has no sync/async distinction, so nothing in the source marks this; the
            // `await` has to come from here, the one place that knows both "this is a plain call
            // to a same-file sub" and "that sub ended up async". Setting `used_await` cascades
            // the same requirement to *this* sub's own signature (`render_sub`'s own check).
            _ if self.known_async_subs.contains(name) => {
                self.used_await = true;
                format!("await {}({})", name, args.join(", "))
            }
            _ => format!("{}({})", name, args.join(", ")),
        }
    }

    fn render_token(&mut self, node: &PpiNode) -> String {
        match node.class.as_str() {
            "PPI::Token::Symbol" => strip_sigil(node.content.as_deref().unwrap_or("")),
            "PPI::Token::Magic" => render_magic(node.content.as_deref().unwrap_or("")),
            "PPI::Token::Number" => node.content.clone().unwrap_or_default(),
            "PPI::Token::Quote::Single" => convert_string_content(&FoundString {
                quote: '\'',
                content: strip_quotes(node.content.as_deref().unwrap_or(""), '\''),
            }),
            "PPI::Token::Quote::Double" | "PPI::Token::Quote::Interpolate" => {
                convert_string_content(&FoundString {
                    quote: '"',
                    content: strip_quotes(node.content.as_deref().unwrap_or(""), '"'),
                })
            }
            // `LANraragi::Utils::Generic::get_version` (imported via `use ... qw(get_version)`)
            // called as a bare, parens-less, zero-arg word — legal Perl once the sub's been
            // imported, but indistinguishable at the token level from a plain unquoted string, so
            // this can't be routed through `try_match_call`'s "Word + `Structure::List`" pattern
            // the way every *parenthesized* call is (there's no list here at all to match on).
            "PPI::Token::Word" if node.content.as_deref() == Some("get_version") => {
                "legacyCompat.getVersion()".to_string()
            }
            // A fully package-qualified name (`LANraragi::Utils::Database::get_tags`,
            // `Time::Piece`, `Mojo::File`, ...) — either an external CPAN module or another
            // LANraragi-internal module this plugin's own conversion has no way to reach (no JS
            // equivalent exists for either). Left as a plain bareword, `::` isn't valid inside a
            // JS identifier at all — a real syntax error deno's parser catches immediately,
            // *worse* than the usual "unresolved name" case (which is at least syntactically
            // valid, just wrong at runtime) — so this sanitizes it into a valid-looking
            // placeholder identifier instead, downgrading a syntax error into the same
            // "manual attention needed" category as any other unresolved external reference.
            "PPI::Token::Word" if node.content.as_deref().is_some_and(|c| c.contains("::")) => {
                let raw = node.content.as_deref().unwrap();
                self.warnings.push(format!(
                    "external Perl module reference has no JS equivalent: {raw}"
                ));
                raw.replace("::", "_")
            }
            // A single-segment external module name (`URI`, `JSON`, ...) referenced bare — same
            // "no JS equivalent" situation as the `::`-containing case just above, just without
            // the `::` that case keys off. Only the *name itself* is checked (not, say, that it's
            // immediately followed by `->`) — `imported_external_modules` was built from this
            // exact file's own `use` statements, so a bareword matching one of them is already
            // about as reliable a signal as the pre-existing `::` check right above, which makes
            // the same trade-off (see real corpus case: `Download/EHentai.pm`'s `use URI;` +
            // `URI->new(...)`, previously rendered as a silently-undeclared `URI.new()` with zero
            // warning at all — `imported_external_modules`'s own docs have the full story). Left
            // as-is (not sanitized like the `::` case) since a bare word with no `::` in it is
            // already a syntactically valid JS identifier, just an unresolved one — the same
            // "manual attention needed" category, without needing the `::` case's extra rewrite.
            "PPI::Token::Word"
                if node
                    .content
                    .as_deref()
                    .is_some_and(|c| self.imported_external_modules.contains(c)) =>
            {
                let raw = node.content.as_deref().unwrap();
                self.warnings.push(format!(
                    "external Perl module reference has no JS equivalent: {raw}"
                ));
                raw.to_string()
            }
            "PPI::Token::Word" => render_word(node.content.as_deref().unwrap_or("")),
            "PPI::Token::QuoteLike::Words" => render_qw(node.content.as_deref().unwrap_or("")),
            // A bare regex match reaching this generic fallback (rather than the dedicated `=~`/
            // `!~` handling in `render_expr_sequence`) means there was no explicit match operator
            // at all — Perl implicitly tests a standalone `/pattern/` against `$_` in that
            // position (e.g. `grep(!/pattern/, @list)`'s condition), which `render_magic` already
            // maps to the JS identifier `it` for every other bare-`$_` reference, so the two stay
            // consistent.
            // A bare `/* TODO */` comment isn't a valid standalone JS expression on its own
            // (`x.test(/* comment */)` is a syntax error, not a call with a placeholder argument
            // — deno's parser catches exactly this), so every fallback below wraps the comment
            // around a real placeholder value (`undefined`) instead of being just the comment.
            "PPI::Token::Regexp::Match" => match render_perl_regex_literal(node) {
                Some(regex) => format!("{regex}.test(it)"),
                None => {
                    self.warnings.push(format!(
                        "unsupported regex delimiter or /x flag: {}",
                        node.source_text()
                    ));
                    format!(
                        "(undefined /* TODO(perl-convert): {} */)",
                        node.source_text()
                    )
                }
            },
            "PPI::Token::QuoteLike::Regexp" => match render_perl_regex_literal(node) {
                Some(regex) => regex,
                None => {
                    self.warnings.push(format!(
                        "unsupported regex delimiter or /x flag: {}",
                        node.source_text()
                    ));
                    format!(
                        "(undefined /* TODO(perl-convert): {} */)",
                        node.source_text()
                    )
                }
            },
            "PPI::Token::Regexp::Substitute" => self.render_regex_substitute(node),
            "PPI::Token::Operator" => render_operator(node.content.as_deref().unwrap_or("")),
            "PPI::Token::Structure" => node.content.clone().unwrap_or_default(),
            "PPI::Token::Cast" => strip_sigil(node.content.as_deref().unwrap_or("")),
            "PPI::Token::ArrayIndex" => {
                let name = node
                    .content
                    .as_deref()
                    .unwrap_or("")
                    .trim_start_matches("$#");
                format!("({name}.length - 1)")
            }
            // An empty `()` reaching this expression-value renderer is Perl's empty-list literal
            // used as a value (real corpus case: `Metadata/ChaikaFile.pm`'s
            // `$json->{"tags"} || ();`, an empty-array fallback) — `()` alone isn't a valid
            // standalone JS expression (`x || ()` is a syntax error), so this renders the
            // semantically closest JS equivalent, an empty array, instead of the generic
            // parenthesized-expression case below (which would just produce empty, invalid
            // parens for a genuinely empty child list).
            "PPI::Structure::List"
                if self
                    .real_children(node.children())
                    .iter()
                    .all(|c| c.class == "PPI::Token::Whitespace") =>
            {
                "[]".to_string()
            }
            "PPI::Structure::List" | "PPI::Structure::Condition" => {
                format!("({})", self.render_children_expr(node))
            }
            "PPI::Structure::Constructor" => self.render_constructor(node),
            // An empty `{}` reaching this *expression-value* renderer (as opposed to
            // `render_statement_list`, which handles a block used as an actual statement body by
            // iterating its children directly and never delegates to this function at all) can
            // only be Perl's ambiguous anonymous-hashref-vs-empty-block grammar resolving to
            // `PPI::Structure::Block` instead of `PPI::Structure::Constructor` (verified: real
            // `perl -MPPI::Dumper` output for `ref {}` — see `try_match_ref_eq`'s identical
            // handling of the same ambiguity) — there's no valid Perl expression-position empty
            // block that *isn't* meant as a hashref, so this is safe unconditionally here.
            "PPI::Structure::Block"
                if node
                    .children()
                    .iter()
                    .all(|c| c.class == "PPI::Token::Whitespace") =>
            {
                "{}".to_string()
            }
            "PPI::Structure::Subscript" => {
                let flat = self.flatten_statement_children(node.children());
                // A bareword hash key (`$x->{key}`, `$hash{key}`) is Perl's auto-quoting sugar
                // for `$x->{'key'}` — the bareword itself was never a variable/function
                // reference. Rendering it as-is would produce `x[key]`, which JS reads as "look
                // up the value of a variable named `key`" — almost always undefined, and a
                // silent-wrong-value bug rather than a loud syntax error. Only applies when the
                // subscript is *exactly* one bare word; `$x->{$key}` (an actual variable) and
                // `$x->{"key"}` (already-quoted) fall through to the normal renderer untouched.
                if flat.len() == 1 && flat[0].class == "PPI::Token::Word" {
                    let key = flat[0].content.as_deref().unwrap_or("");
                    format!("{NO_SPACE_BEFORE}[{key:?}]")
                } else {
                    let inner = self.render_children_expr(node);
                    format!("{NO_SPACE_BEFORE}[{inner}]")
                }
            }
            "PPI::Statement" | "PPI::Statement::Expression" => self.render_children_expr(node),
            _ => {
                self.warnings
                    .push(format!("unhandled token class: {}", node.class));
                // A bare comment isn't a valid standalone JS expression on its own (see the
                // identical `undefined /* TODO */` wrapping a few cases above, for the same
                // reason) — this is `render_token`'s last-resort catch-all, reached in
                // expression-value position just as often as those more specific cases are.
                format!(
                    "(undefined /* TODO(perl-convert): {} */)",
                    node.source_text()
                )
            }
        }
    }

    /// A bare `s/pattern/replacement/flags` reaching `render_token`'s generic fallback (rather
    /// than the dedicated `=~ s///` handling in `render_expr_sequence`) means there was no
    /// explicit binding operator — Perl implicitly substitutes against `$_` in that position,
    /// same reasoning as the bare-match case right above this one.
    fn render_regex_substitute(&mut self, node: &PpiNode) -> String {
        // See the identical `undefined /* TODO */` wrapping in `render_token`'s
        // `Regexp::Match`/`QuoteLike::Regexp` cases for why a bare comment alone isn't safe here.
        self.render_regex_substitute_call("it", node)
            .unwrap_or_else(|| {
                format!(
                    "(undefined /* TODO(perl-convert): {} */)",
                    node.source_text()
                )
            })
    }

    /// Renders `lhs =~ s/pattern/replacement/flags` as a JS `.replace()` call. Perl's
    /// substitution mutates `lhs` in place unless the non-destructive `/r` flag is present, in
    /// which case it leaves `lhs` untouched and evaluates to the substituted copy instead — both
    /// map cleanly onto `String.prototype.replace`, which already treats `$1`/`$2` backreferences
    /// in the replacement the same way Perl does. Returns `None` (pushing a warning) for
    /// constructs with no safe JS equivalent: the `/x` (extended/whitespace-insensitive) flag —
    /// same reasoning as `render_perl_regex_literal` — and recursive subpatterns (`(?0)`, `(?R)`,
    /// ...), which JS regex syntax has no equivalent for at all.
    fn render_regex_substitute_call(&mut self, lhs: &str, node: &PpiNode) -> Option<String> {
        let raw = node.content.as_deref()?;
        let rest = raw.strip_prefix('s')?;
        let Some((pattern, replacement, flags)) = split_substitute_parts(rest) else {
            self.warnings.push(format!(
                "unrecognized regex substitution delimiters: s{rest}"
            ));
            return None;
        };
        if flags.contains('x') || contains_recursive_subpattern(pattern) {
            self.warnings.push(format!(
                "unsupported regex substitution (recursive subpattern or /x flag): s{rest}"
            ));
            return None;
        }
        let has_r = flags.contains('r');
        let js_flags: String = flags
            .chars()
            .filter(|c| matches!(c, 'g' | 'i' | 'm' | 's' | 'u'))
            .collect();
        let pattern = escape_unescaped_slashes(pattern);
        let replacement_js = format!("{replacement:?}");
        if has_r {
            Some(format!(
                "{lhs}.replace(/{pattern}/{js_flags}, {replacement_js})"
            ))
        } else {
            Some(format!(
                "({lhs} = {lhs}.replace(/{pattern}/{js_flags}, {replacement_js}))"
            ))
        }
    }

    fn render_constructor(&mut self, node: &PpiNode) -> String {
        match node.start.as_deref() {
            Some("[") => format!("[{}]", self.render_children_expr(node)),
            // `{ key => value, ... }` anonymous hashref — rendered as `key: value` pairs
            // (`render_key_value_pairs`) rather than through the generic expression renderer,
            // since a bare `=>` defaults to a comma there (see `render_operator`'s docs) and
            // would otherwise turn this into a plain (wrong) comma-joined list. Falls back to
            // the generic renderer for a `{ }` that turns out not to be hash-literal-shaped
            // (e.g. an empty hashref `{}`, or a block PPI misclassified as a constructor).
            Some("{") => {
                let flat = self.flatten_statement_children(node.children());
                match self.render_key_value_pairs(&flat) {
                    Some(fields) => format!("{{ {} }}", fields.join(", ")),
                    None => format!("{{{}}}", self.render_children_expr(node)),
                }
            }
            _ => self.render_children_expr(node),
        }
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

fn is_word(node: &PpiNode, word: &str) -> bool {
    node.class == "PPI::Token::Word" && node.content.as_deref() == Some(word)
}

fn is_operator(node: &PpiNode, op: &str) -> bool {
    (node.class == "PPI::Token::Operator" || node.class == "PPI::Token::Structure")
        && node.content.as_deref() == Some(op)
}

/// Perl has no restriction on using a reserved-word-shaped name as a variable — its `$`/`@`/`%`
/// sigils already disambiguate a variable reference from the keyword itself, so `my $return = ...`
/// is entirely ordinary Perl (real corpus case: `Metadata/Eze.pm`'s own `$return` accumulator
/// variable). JS has no such disambiguation — `return` bare is always the keyword — so a stripped
/// name landing on one of these needs a suffix to stay a valid, non-keyword JS identifier. Not an
/// exhaustive JS reserved-word list, just the ones realistically likely to show up as an
/// unprefixed Perl variable name.
const JS_RESERVED_WORDS: &[&str] = &[
    "return",
    "class",
    "delete",
    "new",
    "typeof",
    "in",
    "of",
    "function",
    "var",
    "let",
    "const",
    "if",
    "else",
    "for",
    "while",
    "do",
    "switch",
    "case",
    "break",
    "continue",
    "default",
    "throw",
    "try",
    "catch",
    "finally",
    "void",
    "this",
    "super",
    "extends",
    "static",
    "yield",
    "async",
    "await",
    "import",
    "export",
    "null",
    "true",
    "false",
    "instanceof",
    "with",
    "debugger",
];

fn strip_sigil(raw: &str) -> String {
    if raw == "@_" {
        return "args".to_string();
    }
    let name = raw.trim_start_matches(['$', '@', '%', '&']).to_string();
    if JS_RESERVED_WORDS.contains(&name.as_str()) {
        format!("{name}_")
    } else {
        name
    }
}

fn render_magic(raw: &str) -> String {
    match raw {
        "@_" => "args".to_string(),
        "$_" => "it".to_string(),
        // Perl's error variable, set by a failed `eval { }`/`eval "string"` — `strip_sigil`
        // alone would reduce this to an empty string (it trims *every* leading `$`/`@`/`%`/`&`
        // character, and `$@` is made up entirely of two of those), silently deleting the
        // condition it was part of (e.g. `if ($@ || ...)` → `if ( || ...)`, still-valid-looking
        // but wrong JS). Paired with the `eval { }` → `try { } catch (perlError)` conversion.
        "$@" => "perlError".to_string(),
        // Perl's named-capture-group magic: `%+` (a hash of every `(?<name>...)` capture from
        // the *most recent successful match*) and its single-element form `$+{'name'}` (a bare
        // `$+` Magic token immediately followed by a `Structure::Subscript`, handled generically
        // by the subscript-chain rendering once this returns the base expression) — real corpus
        // case: `Metadata/RegexParse.pm`'s `my %captures = %+;` / `$+{'ttags'}`, both following a
        // `$filename =~ $regex;` match against `$regex`, itself already rendered as a real JS
        // `RegExp` bound to the `match` variable this codebase declares up front in every sub
        // that uses `=~`. `strip_sigil` alone would reduce `$+`/`%+` to the bare identifier `+`,
        // silently producing nonsense (a numeric unary-plus expression) rather than a real error.
        "%+" | "$+" => "(match?.groups ?? {})".to_string(),
        other if other.len() > 1 && other[1..].chars().all(|c| c.is_ascii_digit()) => {
            format!("match[{}]", &other[1..])
        }
        other => strip_sigil(other),
    }
}

/// Splits `raw` (starting at its opening delimiter, e.g. `/foo(\d+)/gi`, `{foo}gi`, `(foo)gi`)
/// into `(pattern, flags)`. Handles both same-character delimiters (`/.../ `, `!...!`, `|...|`,
/// where the *last* unescaped occurrence of the same char closes it) and Perl's paired-bracket
/// delimiters (`{...}`, `(...)`, `[...]`, `<...>`, which nest — `m{foo{2}bar}` has to track depth
/// so the `{2}` quantifier's own braces don't get mistaken for the closer).
fn split_delimited_pattern(raw: &str) -> Option<(&str, &str)> {
    let mut chars = raw.char_indices();
    let (_, open) = chars.next()?;
    let close_char = match open {
        '{' => '}',
        '(' => ')',
        '[' => ']',
        '<' => '>',
        c => c,
    };
    let paired = open != close_char;
    let mut depth = 1usize;
    let mut close_idx = None;
    let rest_start = open.len_utf8();
    let mut iter = raw[rest_start..].char_indices().peekable();
    while let Some((idx, c)) = iter.next() {
        if c == '\\' {
            iter.next();
            continue;
        }
        if paired && c == open {
            depth += 1;
        } else if c == close_char {
            depth -= 1;
            if depth == 0 {
                close_idx = Some(idx);
                if !paired {
                    // Same-char delimiters: keep scanning — Perl's own grammar always treats the
                    // *last* unescaped occurrence as the real closer (matches the pre-existing
                    // `/.../ ` behavior this generalizes), so don't stop at the first one.
                    depth = 1;
                    continue;
                }
                break;
            }
        }
    }
    let close_idx = close_idx?;
    let pattern = &raw[rest_start..rest_start + close_idx];
    let flags = &raw[rest_start + close_idx + close_char.len_utf8()..];
    Some((pattern, flags))
}

/// Finds `open`'s matching `close` (respecting nesting/escapes) in `s`, which must start with
/// `open` itself. Returns `(span_between_the_brackets, byte_index_just_past_the_closing_bracket)`.
fn find_bracket_span(s: &str, open: char, close: char) -> Option<(&str, usize)> {
    let rest_start = open.len_utf8();
    let mut depth = 1usize;
    let mut iter = s[rest_start..].char_indices();
    while let Some((idx, c)) = iter.next() {
        if c == '\\' {
            iter.next();
            continue;
        }
        if open != close && c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some((
                    &s[rest_start..rest_start + idx],
                    rest_start + idx + close.len_utf8(),
                ));
            }
        }
    }
    None
}

/// Splits an `s<delim>...` substitution token's content — with the leading `s` already stripped
/// by the caller — into `(pattern, replacement, flags)`. Same-char delimiters (`s/a/b/g`, the
/// common case) find the 2nd and 3rd unescaped occurrence of the delimiter in sequence; Perl's
/// paired-bracket delimiters (`s{a}{b}g`) use two separate bracket-matched spans back to back
/// (verified: real Perl allows the second pair to use a *different* bracket style than the first,
/// e.g. `s{a}[b]g`, but every real use in the legacy plugin corpus reuses the same one, so only
/// that shape — re-deriving the closing bracket from whatever opens the replacement span — is
/// covered here).
fn split_substitute_parts(rest: &str) -> Option<(&str, &str, &str)> {
    let mut chars = rest.char_indices();
    let (_, open) = chars.next()?;
    let close_char = match open {
        '{' => '}',
        '(' => ')',
        '[' => ']',
        '<' => '>',
        c => c,
    };
    if open == close_char {
        let body = &rest[open.len_utf8()..];
        let mut delim_idxs = Vec::new();
        let mut iter = body.char_indices();
        while let Some((idx, c)) = iter.next() {
            if c == '\\' {
                iter.next();
                continue;
            }
            if c == open {
                delim_idxs.push(idx);
            }
        }
        if delim_idxs.len() < 2 {
            return None;
        }
        let pattern = &body[..delim_idxs[0]];
        let replacement = &body[delim_idxs[0] + open.len_utf8()..delim_idxs[1]];
        let flags = &body[delim_idxs[1] + open.len_utf8()..];
        Some((pattern, replacement, flags))
    } else {
        let (pattern, after_pattern) = find_bracket_span(rest, open, close_char)?;
        let remainder = &rest[after_pattern..];
        let trimmed = remainder.trim_start();
        let repl_open = trimmed.chars().next()?;
        let repl_close = match repl_open {
            '{' => '}',
            '(' => ')',
            '[' => ']',
            '<' => '>',
            c => c,
        };
        let (replacement, after_replacement) = find_bracket_span(trimmed, repl_open, repl_close)?;
        let flags = &trimmed[after_replacement..];
        Some((pattern, replacement, flags))
    }
}

/// Perl's recursive-subpattern syntax (`(?0)` = "recurse into the whole pattern", `(?1)`/`(?2)`/
/// ... = "recurse into capture group N", `(?R)` = same as `(?0)`) has no JS regex equivalent at
/// all (verified: `Metadata/Fakku.pm`'s `s/\[([^\[\]]|(?0))*]//g`, a balanced-bracket matcher that
/// depends on this). A plain substring scan rather than a full regex-syntax parser, since this
/// only needs to *detect* the construct to bail out safely, not actually support it.
fn contains_recursive_subpattern(pattern: &str) -> bool {
    pattern.contains("(?R)") || (0..=9).any(|d| pattern.contains(&format!("(?{d})")))
}

/// Escapes every *unescaped* `/` in `pattern` (so it's safe to splice into a JS `/.../ ` regex
/// literal) without double-escaping ones that were already escaped in the Perl source — real bug
/// this fixes: a plain `pattern.replace('/', "\\/")` turns an already-correct `\/` (from a Perl
/// pattern whose own delimiter was also `/`, so any literal `/` in the pattern body already needed
/// escaping there too) into `\\/`, a literal backslash followed by an unescaped `/` — which
/// prematurely ends the JS regex literal instead of matching a `/` character, a real syntax error
/// deno's parser caught (verified against `Metadata/Chaika.pm`'s own `https?:\/\/panda\.chaika...`
/// pattern). Walks the string tracking backslash-escapes so an already-escaped `\/` (or any other
/// `\X` escape) is copied through unchanged, and only a bare, unescaped `/` gets a new backslash.
fn escape_unescaped_slashes(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.push(c);
            if let Some(next) = chars.next() {
                out.push(next);
            }
            continue;
        }
        if c == '/' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Converts a `PPI::Token::Regexp::Match` (raw content like `/foo(\d+)/gi`, or the explicit
/// `m{foo}gi`/`m(foo)gi`/etc. forms) into a JS `RegExp` literal string (`/foo(\d+)/gi`). Perl and
/// JS regex syntax agree closely enough for the patterns seen in the legacy plugin corpus
/// (character classes, quantifiers, non-capturing groups, `i`/`g` flags) that no pattern-body
/// translation is needed beyond re-escaping the delimiter.
fn render_perl_regex_literal(node: &PpiNode) -> Option<String> {
    let raw = node.content.as_deref()?;
    // Bare form (`/foo/gi`) already starts at its own delimiter; the explicit `m`/`qr` forms
    // (`m{foo}gi`, `qr/foo/gi`) need that prefix stripped first so both land on the same shape —
    // a string starting with the opening delimiter itself, which `split_delimited_pattern`
    // expects.
    let rest = if let Some(stripped) = raw.strip_prefix("qr") {
        stripped
    } else if let Some(stripped) = raw.strip_prefix('m') {
        stripped
    } else if raw.starts_with('/') {
        raw
    } else {
        return None;
    };
    let (pattern, flags) = split_delimited_pattern(rest)?;
    // JS only recognizes a handful of flag letters; Perl's `x` (extended/whitespace-insensitive)
    // has no JS equivalent and silently changes the pattern's meaning if passed through verbatim
    // (JS would treat the literal whitespace/`#`-comments in the pattern as real, matchable
    // characters) — safer to bail out to the TODO fallback than emit a subtly-wrong regex.
    if flags.contains('x') {
        return None;
    }
    // A `$var`/`${...}`/`@array` inside the pattern body is Perl variable interpolation — a
    // genuinely *dynamic* regex built from a runtime value (verified real case:
    // `Metadata/RegexParse.pm`'s `qr/$params->{'regex_string'}/`), not a static literal this
    // function's plain text-substitution approach can handle at all — treating the interpolated
    // text as literal regex syntax would silently produce a regex that doesn't do what the
    // original dynamic one did. A real end-of-string `$` anchor is never followed by a word
    // character or `{` (it's always alone, or before `)`/`|`/end-of-pattern), so this narrow
    // check doesn't false-positive on ordinary anchors.
    if pattern
        .match_indices('$')
        .any(|(idx, _)| matches!(pattern[idx + 1..].chars().next(), Some(c) if c.is_alphabetic() || c == '_' || c == '{'))
        || pattern.contains('@')
    {
        return None;
    }
    let mut js_flags: String = flags
        .chars()
        .filter(|c| matches!(c, 'g' | 'i' | 'm' | 's' | 'u'))
        .collect();
    // Perl's `(?i)`/`(?im)` *inline* modifier prefix — as opposed to a scoped `(?i:...)` group,
    // which JS's regex engine doesn't support at all — turns the named flags on for the rest of
    // the pattern from that point forward. When it's the very first thing in the pattern (the
    // only shape seen in the real corpus: `Metadata/HDoujin.pm`'s twice-repeated
    // `m/(?i)^(artist|...)/`), that's equivalent to just setting the same flag on the whole JS
    // regex literal, so it's stripped from the pattern body and folded into `js_flags` instead —
    // left as-is, `(?i)` isn't valid JS regex syntax at all (no bare non-capturing modifier
    // group), a real syntax error deno's parser caught.
    let mut pattern = pattern;
    if let Some(rest) = pattern.strip_prefix("(?") {
        if let Some(close) = rest.find(')') {
            let modifiers = &rest[..close];
            if !modifiers.is_empty() && modifiers.chars().all(|c| matches!(c, 'i' | 'm' | 's')) {
                for c in modifiers.chars() {
                    if !js_flags.contains(c) {
                        js_flags.push(c);
                    }
                }
                pattern = &rest[close + 1..];
            }
        }
    }
    // JS regex literals use `/` as their only delimiter regardless of what Perl's source used —
    // an unescaped `/` surviving from a paired-delimiter pattern (`m{a/b}`, where `/` needs no
    // escaping in Perl) would otherwise prematurely end the JS literal, so it's escaped here —
    // see `escape_unescaped_slashes`'s own docs for why a plain blanket replace is wrong.
    let pattern = escape_unescaped_slashes(pattern);
    Some(format!("/{pattern}/{js_flags}"))
}

/// `qw(a b c)` → `["a", "b", "c"]` — Perl's whitespace-separated word-list quoting. Any of Perl's
/// bracket/brace/bang delimiters (`()`, `[]`, `{}`, `//`, `!!`, ...) are accepted (the raw content
/// always starts `qw` followed by exactly one opening delimiter character and ends with its
/// matching closer), since PPI hands this token over as one opaque string regardless of which
/// delimiter the source used.
///
/// A `qw(single-word)` with no whitespace at all — verified in the legacy plugin corpus as a
/// (fairly odd, but real) way of spelling a literal string via `qw(")` string concatenation,
/// relying on Perl's list-in-scalar-context "last element wins" rule — renders as a plain JS
/// string instead of a one-element array: in a `.`-concatenation chain (the only context this
/// idiom is ever used in), a real one-element *array* would stringify to the same text either
/// way once JS's own array-to-string coercion kicks in, EXCEPT that behavior isn't guaranteed
/// identical to Perl's across every possible surrounding expression, so the plain string is the
/// closer-to-correct, safer rendering for that specific single-word case.
fn render_qw(raw: &str) -> String {
    let after_qw = raw.strip_prefix("qw").unwrap_or(raw);
    let mut chars = after_qw.chars();
    let Some(open) = chars.next() else {
        return "[]".to_string();
    };
    let close = match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '<' => '>',
        other => other,
    };
    let inner = after_qw
        .strip_prefix(open)
        .and_then(|s| s.strip_suffix(close))
        .unwrap_or("");
    let words: Vec<&str> = inner.split_whitespace().collect();
    if words.len() == 1 {
        return format!("{:?}", words[0]);
    }
    format!(
        "[{}]",
        words
            .iter()
            .map(|w| format!("{w:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn render_word(word: &str) -> String {
    match word {
        "eq" => "===".to_string(),
        "ne" => "!==".to_string(),
        "lt" => "<".to_string(),
        "gt" => ">".to_string(),
        "le" => "<=".to_string(),
        "ge" => ">=".to_string(),
        "and" => "&&".to_string(),
        "or" => "||".to_string(),
        "not" => "!".to_string(),
        "undef" => "undefined".to_string(),
        "shift" => "args.shift()".to_string(),
        _ => word.to_string(),
    }
}

fn render_operator(op: &str) -> String {
    match op {
        "." => "+".to_string(),
        ".=" => "+=".to_string(),
        "->" => ".".to_string(),
        // A bare `=>` is just a comma with an autoquoting side effect on a preceding bareword
        // (Perl's "fat comma") — it means "this is a key/value pair" only when the *surrounding
        // structure* is being read as a hash literal, which isn't knowable from the operator
        // token alone (the exact same `=>` also shows up as a plain argument separator, e.g.
        // `->post($url => form => {...})`, which is a 3-argument call, not a hash literal).
        // `render_key_value_pairs` handles the "yes, this is a hash literal" contexts
        // specifically (the `return (...)` idiom, `{ ... }` constructors) and renders `=>` as
        // `:` itself; everywhere else, this default of a plain comma is the correct general case.
        "=>" => ",".to_string(),
        "eq" => "===".to_string(),
        "ne" => "!==".to_string(),
        // Defined-or. `//` left untranslated would silently become a JS line comment, turning
        // `$x // "default"` (a fallback value) into `x` with the rest of the line commented out
        // — a correctness bug, not just a style one, so this mapping matters more than most.
        "//" => "??".to_string(),
        "//=" => "??=".to_string(),
        "=~" => {
            "/* TODO(perl-convert): =~ regex match — verify against original */.match".to_string()
        }
        _ => op.to_string(),
    }
}

fn strip_quotes(raw: &str, quote: char) -> String {
    raw.strip_prefix(quote)
        .and_then(|s| s.strip_suffix(quote))
        .unwrap_or(raw)
        .to_string()
}

/// Collects every `PPI::Token::Symbol` inside a structure (used for `my ($a, $b) = @_;`'s LHS).
fn symbol_names_in(node: &PpiNode) -> Vec<String> {
    let mut names = Vec::new();
    collect_symbols(node, &mut names);
    names
}

fn collect_symbols(node: &PpiNode, out: &mut Vec<String>) {
    if node.class == "PPI::Token::Symbol" {
        out.push(strip_sigil(node.content.as_deref().unwrap_or("")));
    }
    for child in node.children() {
        collect_symbols(child, out);
    }
}

/// Same walk as [`symbol_names_in`], but keeping each symbol's own leading sigil character
/// alongside its stripped name — needed by [`Renderer::render_entry_destructure`] to tell a
/// `%params`-style hash slot apart from an ordinary `$x` scalar slot in a mixed destructure.
fn symbols_with_sigils_in(node: &PpiNode) -> Vec<(char, String)> {
    let mut names = Vec::new();
    collect_symbols_with_sigils(node, &mut names);
    names
}

fn collect_symbols_with_sigils(node: &PpiNode, out: &mut Vec<(char, String)>) {
    if node.class == "PPI::Token::Symbol" {
        let raw = node.content.as_deref().unwrap_or("");
        out.push((raw.chars().next().unwrap_or('$'), strip_sigil(raw)));
    }
    for child in node.children() {
        collect_symbols_with_sigils(child, out);
    }
}

/// Splits a flat token sequence on top-level `,` tokens (`PPI::Token::Operator` with content
/// `,`) — no depth tracking needed since nested lists are already their own `PPI::Structure::*`
/// nodes in the tree, not raw comma tokens at this level.
/// Whether `tokens` contains a `,` operator at this level — nested lists/calls are already their
/// own `PPI::Structure::*` node (their commas live one level deeper, inside that node's own
/// `children()`), so this never mistakes e.g. `foo(a, b)`'s argument comma for a top-level one.
fn has_top_level_comma(tokens: &[&PpiNode]) -> bool {
    tokens.iter().any(|t| is_operator(t, ","))
}

/// Like `split_on_commas`, but also splits on `=>` (the fat comma) — for the one call site
/// (`render_post_call_args`) whose arguments are separated by a *mix* of `,`/`=>` with no
/// key/value meaning attached to any of them (`$url => kind => {data}` is three positional
/// arguments, not hash pairs), unlike every other `split_on_commas` caller.
fn split_on_commas_and_fat_commas<'a>(tokens: &[&'a PpiNode]) -> Vec<Vec<&'a PpiNode>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    for &t in tokens {
        if is_operator(t, ",") || is_operator(t, "=>") {
            if !current.is_empty() {
                groups.push(std::mem::take(&mut current));
            }
        } else {
            current.push(t);
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

fn split_on_commas<'a>(tokens: &[&'a PpiNode]) -> Vec<Vec<&'a PpiNode>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    for &t in tokens {
        if is_operator(t, ",") {
            if !current.is_empty() {
                groups.push(std::mem::take(&mut current));
            }
        } else {
            current.push(t);
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

fn bareword_or_string_content(node: &PpiNode) -> String {
    match node.class.as_str() {
        "PPI::Token::Word" => node.content.clone().unwrap_or_default(),
        "PPI::Token::Quote::Single" => strip_quotes(node.content.as_deref().unwrap_or(""), '\''),
        "PPI::Token::Quote::Double" => strip_quotes(node.content.as_deref().unwrap_or(""), '"'),
        _ => node.source_text(),
    }
}

/// `ref $x eq 'ARRAY'` / `ref($x) eq 'ARRAY'` → `Array.isArray(x)` (and `'HASH'` → a plain-object
/// check). Checked as a lookahead window since it spans several sibling tokens.
fn try_match_ref_eq(tokens: &[&PpiNode], i: usize) -> Option<(String, usize)> {
    if !is_word(tokens[i], "ref") {
        return None;
    }
    let mut j = i + 1;
    let operand = if tokens.get(j)?.class == "PPI::Structure::List" {
        let inner = tokens[j].source_text();
        j += 1;
        inner
            .trim_start_matches('(')
            .trim_end_matches(')')
            .trim_start_matches('$')
            .to_string()
    } else if tokens.get(j)?.class == "PPI::Token::Symbol" {
        let name = strip_sigil(tokens[j].content.as_deref().unwrap_or(""));
        j += 1;
        name
    } else {
        return None;
    };
    let eq_matches = tokens
        .get(j)
        .map(|t| is_word(t, "eq") || is_operator(t, "eq"))
        .unwrap_or(false);
    if !eq_matches {
        return None;
    }
    j += 1;
    let kind_token = tokens.get(j)?;
    // `ref $x eq ref {}` / `ref $x eq ref []` — an alternative spelling of the same "is this a
    // hashref/arrayref" test as the quoted-string form below, comparing against a fresh empty
    // hashref/arrayref's own `ref()` result instead (verified: `Metadata/GalleryDL.pm` uses this
    // spelling exclusively, never the string-literal one).
    if is_word(kind_token, "ref") {
        let ctor = tokens.get(j + 1)?;
        // `{}` here is genuinely ambiguous in Perl's own grammar (an anonymous-hashref
        // constructor, or an empty code block?) — PPI's heuristic resolves it as
        // `PPI::Structure::Block`, not `PPI::Structure::Constructor`, when it follows a bareword
        // like `ref` with no clearer hash-context hint (verified against real
        // `perl -MPPI::Dumper` output for `Metadata/GalleryDL.pm`'s own `ref {}`) — `[]` has no
        // such ambiguity and always parses as `Structure::Constructor`.
        let is_empty_block = ctor.class == "PPI::Structure::Block"
            && ctor
                .children()
                .iter()
                .all(|c| c.class == "PPI::Token::Whitespace");
        let rendered = if is_empty_block || ctor.start.as_deref() == Some("{") {
            format!(
                "(typeof {operand} === \"object\" && {operand} !== null && !Array.isArray({operand}))"
            )
        } else if ctor.class == "PPI::Structure::Constructor" && ctor.start.as_deref() == Some("[")
        {
            format!("Array.isArray({operand})")
        } else {
            return None;
        };
        return Some((rendered, j + 2 - i));
    }
    let kind = strip_quotes(kind_token.content.as_deref().unwrap_or(""), '\'');
    let rendered = match kind.as_str() {
        "ARRAY" => format!("Array.isArray({operand})"),
        "HASH" => format!(
            "(typeof {operand} === \"object\" && {operand} !== null && !Array.isArray({operand}))"
        ),
        _ => return None,
    };
    Some((rendered, j + 1 - i))
}

/// Wraps `expr` in parens before splicing it into a `EXPR.method(...)` position, unless it's
/// already a single simple term — needed because `render_named_call`'s callers hand it whatever
/// arbitrary expression was in that argument position, and `.method()` binds tighter than nearly
/// every Perl operator that could produce it (`$genre // ""` naively spliced into `.split()`
/// would render as `genre ?? "".split(',')`, which — since `.split` binds tighter than `??` in
/// JS too — actually calls `.split` on the empty string literal, not on the coalesced value; a
/// silent correctness bug, not just a style one). A simple term never contains a space (per
/// `join_parts`'s spacing rules, every rendered binary-operator expression does), so that's used
/// as the "does this need protecting" heuristic — redundant parens around a bare identifier are
/// harmless either way.
fn paren_wrap(expr: &str) -> String {
    if expr.contains(' ') {
        format!("({expr})")
    } else {
        expr.to_string()
    }
}

/// Joins rendered token parts with a single space, except around a lone `.` (member-access,
/// from a converted Perl `->` — string concatenation `.` never reaches this function as a bare
/// `.` since `render_operator` already turns it into `+`), which gets no surrounding space at
/// all so `logger . debug(...)` comes out as `logger.debug(...)`.
/// Marks a rendered part (e.g. a `[subscript]`) as needing no space before it even though
/// `join_parts` otherwise inserts one between every pair of parts — stripped back out before the
/// part is appended.
const NO_SPACE_BEFORE: char = '\u{1}';

fn join_parts(parts: &[String]) -> String {
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        let no_space_before = part.starts_with(NO_SPACE_BEFORE);
        let clean = part.strip_prefix(NO_SPACE_BEFORE).unwrap_or(part);
        let is_dot = clean == ".";
        let prev_is_dot = i > 0 && parts[i - 1] == ".";
        if i > 0 && !is_dot && !prev_is_dot && !no_space_before {
            out.push(' ');
        }
        out.push_str(clean);
    }
    out
}

/// Cosmetic-only cleanup of the uniform single-space joining `render_expr_sequence` does —
/// collapses `x , y` → `x, y`, `( x` → `(x`, etc. Never changes meaning, only whitespace.
fn tidy_spacing(s: &str) -> String {
    let re_before = Regex::new(r"\s+([,;)\]}])").unwrap();
    let re_after = Regex::new(r"([(\[{])\s+").unwrap();
    let s = re_before.replace_all(s, "$1").to_string();
    re_after.replace_all(&s, "$1").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(class: &str, content: Option<&str>, children: Vec<PpiNode>) -> PpiNode {
        PpiNode {
            class: class.to_string(),
            content: content.map(String::from),
            children: if children.is_empty() {
                None
            } else {
                Some(children)
            },
            start: None,
            finish: None,
        }
    }

    fn word(w: &str) -> PpiNode {
        node("PPI::Token::Word", Some(w), vec![])
    }
    fn symbol(s: &str) -> PpiNode {
        node("PPI::Token::Symbol", Some(s), vec![])
    }
    fn op(o: &str) -> PpiNode {
        node("PPI::Token::Operator", Some(o), vec![])
    }
    fn structure(s: &str) -> PpiNode {
        node("PPI::Token::Structure", Some(s), vec![])
    }
    fn ws() -> PpiNode {
        node("PPI::Token::Whitespace", Some(" "), vec![])
    }

    #[test]
    fn renders_bare_shift_as_discard_comment() {
        // Real PPI classifies a bare `shift;` (no `my`) as a plain `PPI::Statement`, not
        // `PPI::Statement::Variable` — verified against actual `perl -MPPI` output on a real
        // legacy plugin file, see `try_render_bare_shift`'s docs. A test asserting the wrong
        // class here would pass while the real renderer emits a broken literal `args.shift()`.
        let stmt = node("PPI::Statement", None, vec![word("shift"), structure(";")]);
        let mut r = Renderer::new();
        let out = r.render_statement(&stmt);
        assert!(out.contains("discarded positional arg"));
        assert_eq!(r.shift_cursor, 1);
    }

    #[test]
    fn renders_my_x_equals_shift_with_incrementing_cursor() {
        let stmt1 = node(
            "PPI::Statement::Variable",
            None,
            vec![
                word("my"),
                ws(),
                symbol("$x"),
                ws(),
                op("="),
                ws(),
                word("shift"),
                structure(";"),
            ],
        );
        let mut r = Renderer::new();
        assert_eq!(r.render_statement(&stmt1), "let x = args[0];");
        let stmt2 = node(
            "PPI::Statement::Variable",
            None,
            vec![
                word("my"),
                ws(),
                symbol("$y"),
                ws(),
                op("="),
                ws(),
                word("shift"),
                structure(";"),
            ],
        );
        assert_eq!(r.render_statement(&stmt2), "let y = args[1];");
    }

    #[test]
    fn renders_array_sigil_list_assignment_as_an_array_literal() {
        // `my @tags = ("a", "b", "c");` — the parenthesized list must become `[...]`, not plain
        // `(...)` (which JS reads as the comma operator, keeping only the last value).
        let list = node(
            "PPI::Structure::List",
            None,
            vec![node(
                "PPI::Statement::Expression",
                None,
                vec![
                    node("PPI::Token::Quote::Double", Some("\"a\""), vec![]),
                    structure(","),
                    ws(),
                    node("PPI::Token::Quote::Double", Some("\"b\""), vec![]),
                ],
            )],
        );
        let stmt = node(
            "PPI::Statement::Variable",
            None,
            vec![
                word("my"),
                ws(),
                symbol("@tags"),
                ws(),
                op("="),
                ws(),
                list,
                structure(";"),
            ],
        );
        let mut r = Renderer::new();
        assert_eq!(r.render_statement(&stmt), "let tags = [\"a\", \"b\"];");
    }

    #[test]
    fn renders_hash_sigil_list_assignment_as_an_object_literal() {
        let list = node(
            "PPI::Structure::List",
            None,
            vec![node(
                "PPI::Statement::Expression",
                None,
                vec![
                    word("a"),
                    op("=>"),
                    ws(),
                    node("PPI::Token::Number", Some("1"), vec![]),
                ],
            )],
        );
        let stmt = node(
            "PPI::Statement::Variable",
            None,
            vec![
                word("my"),
                ws(),
                symbol("%h"),
                ws(),
                op("="),
                ws(),
                list,
                structure(";"),
            ],
        );
        let mut r = Renderer::new();
        // `Record<string, any>`, not the bare object-literal-inferred type — Perl hashes accept
        // new keys added after declaration (`$h{newkey} = ...`), which a plain `{ a: 1 }` literal
        // type wouldn't allow assigning onto (verified against real `deno check` output on the
        // converted `EHentai.pm`, whose `%hashdata` declaration is later extended with `title`).
        assert_eq!(
            r.render_statement(&stmt),
            "let h: Record<string, any> = { a: 1 };"
        );
    }

    #[test]
    fn renders_destructure_from_args() {
        let list = node(
            "PPI::Structure::List",
            None,
            vec![node(
                "PPI::Statement::Expression",
                None,
                vec![symbol("$a"), structure(","), ws(), symbol("$b")],
            )],
        );
        let stmt = node(
            "PPI::Statement::Variable",
            None,
            vec![
                word("my"),
                ws(),
                list,
                ws(),
                op("="),
                ws(),
                node("PPI::Token::Magic", Some("@_"), vec![]),
                structure(";"),
            ],
        );
        let mut r = Renderer::new();
        assert_eq!(r.render_statement(&stmt), "let [a, b] = args.slice(0);");
    }

    #[test]
    fn renders_hash_return_idiom() {
        let list = node(
            "PPI::Structure::List",
            None,
            vec![node(
                "PPI::Statement::Expression",
                None,
                vec![
                    word("tags"),
                    ws(),
                    op("=>"),
                    ws(),
                    symbol("$tags"),
                    structure(","),
                    ws(),
                    word("title"),
                    ws(),
                    op("=>"),
                    ws(),
                    symbol("$title"),
                ],
            )],
        );
        let stmt = node(
            "PPI::Statement::Break",
            None,
            vec![word("return"), ws(), list, structure(";")],
        );
        let mut r = Renderer::new();
        assert_eq!(
            r.render_statement(&stmt),
            "return { tags: tags, title: title };"
        );
    }

    #[test]
    fn renders_ref_array_check() {
        let tokens = [
            word("ref"),
            ws(),
            symbol("$x"),
            ws(),
            word("eq"),
            ws(),
            node("PPI::Token::Quote::Single", Some("'ARRAY'"), vec![]),
        ];
        let refs: Vec<&PpiNode> = tokens
            .iter()
            .filter(|n| n.class != "PPI::Token::Whitespace")
            .collect();
        let (rendered, consumed) = try_match_ref_eq(&refs, 0).unwrap();
        assert_eq!(rendered, "Array.isArray(x)");
        assert_eq!(consumed, 4);
    }

    #[test]
    fn strip_sigil_handles_underscore_array() {
        assert_eq!(strip_sigil("@_"), "args");
        assert_eq!(strip_sigil("$lrr_info"), "lrr_info");
        assert_eq!(strip_sigil("@found_tags"), "found_tags");
    }

    #[test]
    fn render_named_call_maps_push_join_die_defined() {
        let mut r = Renderer::new();
        assert_eq!(
            r.render_named_call("push", &["found_tags".into(), "\"x\"".into()]),
            "found_tags.push(\"x\")"
        );
        assert_eq!(
            r.render_named_call("join", &["\", \"".into(), "found_tags".into()]),
            "found_tags.join(\", \")"
        );
        assert_eq!(
            r.render_named_call("die", &["\"oops\"".into()]),
            "throw new Error(\"oops\")"
        );
        assert_eq!(
            r.render_named_call("defined", &["result".into()]),
            "(result !== undefined && result !== null)"
        );
    }

    #[test]
    fn tidy_spacing_collapses_punctuation_whitespace() {
        assert_eq!(
            tidy_spacing("found_tags . push( x , y )"),
            "found_tags . push(x, y)"
        );
    }

    fn subscript(start: &str, finish: &str, children: Vec<PpiNode>) -> PpiNode {
        PpiNode {
            class: "PPI::Structure::Subscript".to_string(),
            content: None,
            children: Some(children),
            start: Some(start.to_string()),
            finish: Some(finish.to_string()),
        }
    }

    #[test]
    fn bareword_hash_subscript_is_quoted_as_a_string_key() {
        // `$lrr_info->{url}` — a bareword key, auto-quoted by Perl itself — must render as
        // `lrr_info["url"]`, not `lrr_info[url]` (which JS reads as "look up variable `url`").
        let tokens = [
            symbol("$lrr_info"),
            op("->"),
            subscript("{", "}", vec![word("url")]),
        ];
        let refs: Vec<&PpiNode> = tokens.iter().collect();
        let mut r = Renderer::new();
        assert_eq!(r.render_expr_sequence(&refs), "lrr_info[\"url\"]");
    }

    #[test]
    fn quoted_string_hash_subscript_is_left_alone() {
        let tokens = [
            symbol("$hash"),
            op("->"),
            subscript(
                "{",
                "}",
                vec![node("PPI::Token::Quote::Double", Some("\"Title\""), vec![])],
            ),
        ];
        let refs: Vec<&PpiNode> = tokens.iter().collect();
        let mut r = Renderer::new();
        assert_eq!(r.render_expr_sequence(&refs), "hash[\"Title\"]");
    }

    #[test]
    fn fat_comma_defaults_to_a_plain_comma_outside_hash_literals() {
        // `->post($url => form => {...})` is a 3-argument call, not a hash literal — a bare
        // `=>` there must stay a comma, not become `:` (which would be invalid JS in this
        // position).
        assert_eq!(render_operator("=>"), ",");
    }

    #[test]
    fn hash_constructor_still_renders_key_value_pairs_via_shared_helper() {
        let constructor = node(
            "PPI::Structure::Constructor",
            None,
            vec![node(
                "PPI::Statement::Expression",
                None,
                vec![word("dltype"), op("=>"), ws(), symbol("$dltype")],
            )],
        );
        let mut constructor = constructor;
        constructor.start = Some("{".to_string());
        constructor.finish = Some("}".to_string());
        let mut r = Renderer::new();
        assert_eq!(r.render_constructor(&constructor), "{ dltype: dltype }");
    }

    #[test]
    fn perl_error_variable_maps_to_a_real_identifier() {
        assert_eq!(render_magic("$@"), "perlError");
    }

    #[test]
    fn render_named_call_maps_index_to_index_of() {
        let mut r = Renderer::new();
        assert_eq!(
            r.render_named_call("index", &["haystack".into(), "\"needle\"".into()]),
            "haystack.indexOf(\"needle\")"
        );
    }

    #[test]
    fn render_named_call_maps_get_logger_through_perl_compat() {
        let mut r = Renderer::new();
        assert_eq!(
            r.render_named_call(
                "get_logger",
                &["\"EH Downloader\"".into(), "\"plugins\"".into()]
            ),
            "legacyCompat.getLogger(\"EH Downloader\", \"plugins\")"
        );
    }

    #[test]
    fn render_named_call_resolves_get_plugin_logger_from_the_plugins_own_name() {
        // `get_plugin_logger()` takes no arguments — at runtime it uses Perl's `caller` reflection
        // plus the calling plugin's own `plugin_info` to fill in the name. There's no equivalent
        // reflection in the converted TS, so the converter resolves it at conversion time instead,
        // from the same `plugin_info` metadata it already parsed (`Renderer::plugin_name`).
        let mut r = Renderer::new();
        r.plugin_name = Some("Tag Copier".to_string());
        assert_eq!(
            r.render_named_call("get_plugin_logger", &[]),
            "legacyCompat.getLogger(\"Tag Copier\", \"plugins\")"
        );
    }

    #[test]
    fn render_named_call_routes_reverse_chomp_sprintf_through_perl_compat() {
        let mut r = Renderer::new();

        assert_eq!(
            r.render_named_call("reverse", &["list".into()]),
            "legacyCompat.reverse(list)"
        );

        let mut r = Renderer::new();
        assert_eq!(
            r.render_named_call("chomp", &["line".into()]),
            "line = legacyCompat.chomp(line)"
        );

        let mut r = Renderer::new();
        assert_eq!(
            r.render_named_call(
                "sprintf",
                &["\"%s: %d\"".into(), "name".into(), "count".into()]
            ),
            "legacyCompat.sprintf(\"%s: %d\", name, count)"
        );
    }

    #[test]
    fn simple_builtins_do_not_flag_perl_compat_usage() {
        let mut r = Renderer::new();
        r.render_named_call("push", &["arr".into(), "x".into()]);
        r.render_named_call("index", &["haystack".into(), "\"needle\"".into()]);
    }

    #[test]
    fn eval_block_becomes_try_catch() {
        let stmt = node(
            "PPI::Statement",
            None,
            vec![
                word("eval"),
                ws(),
                node(
                    "PPI::Structure::Block",
                    None,
                    vec![node(
                        "PPI::Statement::Variable",
                        None,
                        vec![
                            word("my"),
                            ws(),
                            symbol("$x"),
                            ws(),
                            op("="),
                            ws(),
                            word("shift"),
                            structure(";"),
                        ],
                    )],
                ),
                structure(";"),
            ],
        );
        let mut r = Renderer::new();
        let out = r.render_statement(&stmt);
        assert!(
            out.starts_with("let perlError;\ntry {"),
            "expected perlError declared before the try block, got: {out}"
        );
        assert!(
            out.contains("perlError = caughtError;"),
            "expected the outer perlError to be assigned from the catch, got: {out}"
        );
        assert!(
            out.contains("args[0]"),
            "expected the block's own statements to still run, got: {out}"
        );
    }

    #[test]
    fn entry_point_names_maps_plugin_type_to_legacy_and_export_names() {
        assert_eq!(
            entry_point_names("metadata"),
            Some(("get_tags", "execMetadata"))
        );
        assert_eq!(entry_point_names("login"), Some(("do_login", "execLogin")));
        assert_eq!(
            entry_point_names("download"),
            Some(("provide_url", "execDownload"))
        );
        assert_eq!(
            entry_point_names("script"),
            Some(("run_script", "runScript"))
        );
        assert_eq!(entry_point_names("something_unknown"), None);
    }

    #[test]
    fn entry_sub_with_info_hash_gets_a_user_agent_hydration_preamble() {
        // Legacy unconditionally bundles a fresh-logged-in (or blank) `user_agent` into the info
        // hash for every get_tags/provide_url/run_script call — mirrored here by rehydrating
        // `hostArgs.user_agent_cookies` (whatever the host attached, via that plugin's own
        // `login_from`) into a real, usable `legacyCompat.userAgent()` instance before the
        // plugin's own converted code runs, regardless of what that plugin calls its info-hash
        // variable.
        let block = node(
            "PPI::Structure::Block",
            None,
            vec![node(
                "PPI::Statement::Variable",
                None,
                vec![
                    word("my"),
                    ws(),
                    symbol("$x"),
                    ws(),
                    op("="),
                    ws(),
                    node("PPI::Token::Number", Some("1"), vec![]),
                    structure(";"),
                ],
            )],
        );
        let found = FoundSub {
            name: "get_tags".to_string(),
            block: &block,
            params: vec![],
        };
        let mut r = Renderer::new();
        let out = r.render_entry_sub(&found, "execMetadata", None, true);
        assert!(
            out.contains("info.user_agent = legacyCompat.userAgent();"),
            "got: {out}"
        );
        assert!(
            out.contains("info.user_agent.cookie_jar.add(c);"),
            "got: {out}"
        );

        // Must NOT appear for `do_login` (`has_info_hash = false`) — legacy's `do_login` gets no
        // info hash at all, so there's nothing to hang a `user_agent` off of.
        let mut r2 = Renderer::new();
        let out2 = r2.render_entry_sub(&found, "execLogin", None, false);
        assert!(
            !out2.contains("legacyCompat.userAgent()"),
            "login has no info hash to hydrate a user_agent onto, got: {out2}"
        );
    }

    #[test]
    fn render_entry_sub_exports_under_the_host_name_and_binds_two_separate_shifts() {
        // The `shift; my $lrr_info = shift; my ($tagstocopy) = @_;` convention (e.g. CopyTags.pm):
        // an invocant-discard, then the info hash via its own `shift`, then one custom param via
        // a single-name `@_` destructure.
        let block = node(
            "PPI::Structure::Block",
            None,
            vec![
                node("PPI::Statement", None, vec![word("shift"), structure(";")]),
                node(
                    "PPI::Statement::Variable",
                    None,
                    vec![
                        word("my"),
                        ws(),
                        symbol("$lrr_info"),
                        ws(),
                        op("="),
                        ws(),
                        word("shift"),
                        structure(";"),
                    ],
                ),
                node(
                    "PPI::Statement::Variable",
                    None,
                    vec![
                        word("my"),
                        ws(),
                        node(
                            "PPI::Structure::List",
                            None,
                            vec![node(
                                "PPI::Statement::Expression",
                                None,
                                vec![symbol("$tagstocopy")],
                            )],
                        ),
                        ws(),
                        op("="),
                        ws(),
                        node("PPI::Token::Magic", Some("@_"), vec![]),
                        structure(";"),
                    ],
                ),
            ],
        );
        let found = FoundSub {
            name: "get_tags".to_string(),
            block: &block,
            params: vec![],
        };
        let mut r = Renderer::new();
        let out = r.render_entry_sub(&found, "execMetadata", Some("tagstocopy"), true);
        assert!(
            out.starts_with(
                "export async function execMetadata(hostArgs: Record<string, unknown>) {"
            ),
            "got: {out}"
        );
        // No `$lrr_info->{...}` access at all appears in this body, so `collect_static_hash_keys`
        // finds zero *dynamic* keys (vacuously "safe") and a real interface gets generated —
        // narrower than the `Record<string, any>` fallback, which only kicks in when a dynamic
        // key access makes a complete key list impossible to know.
        assert!(
            out.contains("let lrr_info = hostArgs as unknown as ExecMetadataInfo;"),
            "got: {out}"
        );
        assert!(out.contains("interface ExecMetadataInfo"), "got: {out}");
        assert!(
            out.contains("let tagstocopy = hostArgs.arg as string;"),
            "got: {out}"
        );
        assert!(
            !out.contains("args["),
            "should not reference the old positional array: {out}"
        );
    }

    #[test]
    fn render_entry_sub_binds_mixed_sigil_combined_destructure() {
        // The `shift; my ( $lrr_info, %params ) = @_;` convention (e.g. EHentai.pm's
        // `provide_url`): a single combined destructure, `%params` meaning "the plugin's custom
        // args as a hash", not another plain positional slot.
        let block = node(
            "PPI::Structure::Block",
            None,
            vec![
                node("PPI::Statement", None, vec![word("shift"), structure(";")]),
                node(
                    "PPI::Statement::Variable",
                    None,
                    vec![
                        word("my"),
                        ws(),
                        node(
                            "PPI::Structure::List",
                            None,
                            vec![node(
                                "PPI::Statement::Expression",
                                None,
                                vec![symbol("$lrr_info"), structure(","), ws(), symbol("%params")],
                            )],
                        ),
                        ws(),
                        op("="),
                        ws(),
                        node("PPI::Token::Magic", Some("@_"), vec![]),
                        structure(";"),
                    ],
                ),
            ],
        );
        let found = FoundSub {
            name: "provide_url".to_string(),
            block: &block,
            params: vec![],
        };
        let mut r = Renderer::new();
        let out = r.render_entry_sub(&found, "execDownload", Some("forceresampled"), true);
        assert!(
            out.starts_with(
                "export async function execDownload(hostArgs: Record<string, unknown>) {"
            ),
            "got: {out}"
        );
        // Same reasoning as `render_entry_sub_exports_under_the_host_name_and_binds_two_separate_shifts`'s
        // own updated assertion — no hash-key access in this body either, so a real interface
        // (rather than the `any` fallback) gets generated.
        assert!(
            out.contains("let lrr_info = hostArgs as unknown as ExecDownloadInfo;"),
            "got: {out}"
        );
        assert!(out.contains("interface ExecDownloadInfo"), "got: {out}");
        assert!(
            out.contains("let params = { forceresampled: hostArgs.arg as string };"),
            "got: {out}"
        );
    }

    #[test]
    fn helper_subs_are_unaffected_by_entry_mode() {
        // A non-entry helper sub rendered via `render_sub` (not `render_entry_sub`) must keep the
        // existing `...args: any[]` convention untouched, even after an entry sub has already
        // been rendered by the same `Renderer` (guards against `self.entry` leaking across subs).
        let entry_block = node(
            "PPI::Structure::Block",
            None,
            vec![node(
                "PPI::Statement",
                None,
                vec![word("shift"), structure(";")],
            )],
        );
        let entry_found = FoundSub {
            name: "run_script".to_string(),
            block: &entry_block,
            params: vec![],
        };
        let mut r = Renderer::new();
        r.render_entry_sub(&entry_found, "runScript", None, true);

        let helper_block = node(
            "PPI::Structure::Block",
            None,
            vec![node(
                "PPI::Statement::Variable",
                None,
                vec![
                    word("my"),
                    ws(),
                    symbol("$x"),
                    ws(),
                    op("="),
                    ws(),
                    word("shift"),
                    structure(";"),
                ],
            )],
        );
        let helper_found = FoundSub {
            name: "helper".to_string(),
            block: &helper_block,
            params: vec![],
        };
        let out = r.render_sub(&helper_found);
        assert!(
            out.starts_with("function helper(...args: any[]) {"),
            "got: {out}"
        );
        assert!(out.contains("let x = args[0];"), "got: {out}");
    }

    #[test]
    fn entry_point_has_info_hash_is_false_only_for_login() {
        // `do_login` is the one legacy entry point called with *no* info hash at all
        // (`$loginplugin->do_login( @{ $loginargs{customargs} } )` — contrast
        // `$plugin->get_tags( \%infohash, @{ $args{customargs} } )` for every other kind).
        assert!(!entry_point_has_info_hash("login"));
        assert!(entry_point_has_info_hash("metadata"));
        assert!(entry_point_has_info_hash("download"));
        assert!(entry_point_has_info_hash("script"));
    }

    #[test]
    fn render_entry_sub_without_info_hash_binds_every_destructured_name_to_host_arg() {
        // `do_login`'s own convention: `shift; my ($a, $b, $c, $d) = @_;` with *no* separate
        // info-hash slot — every one of those four names is a custom parameter, so (unlike the
        // `get_tags`/`provide_url` cases) even the *first* one must map to `hostArgs.arg`, not to
        // the whole `hostArgs` object.
        let block = node(
            "PPI::Structure::Block",
            None,
            vec![
                node("PPI::Statement", None, vec![word("shift"), structure(";")]),
                node(
                    "PPI::Statement::Variable",
                    None,
                    vec![
                        word("my"),
                        ws(),
                        node(
                            "PPI::Structure::List",
                            None,
                            vec![node(
                                "PPI::Statement::Expression",
                                None,
                                vec![
                                    symbol("$ipb_member_id"),
                                    structure(","),
                                    ws(),
                                    symbol("$ipb_pass_hash"),
                                ],
                            )],
                        ),
                        ws(),
                        op("="),
                        ws(),
                        node("PPI::Token::Magic", Some("@_"), vec![]),
                        structure(";"),
                    ],
                ),
            ],
        );
        let found = FoundSub {
            name: "do_login".to_string(),
            block: &block,
            params: vec![],
        };
        let mut r = Renderer::new();
        let out = r.render_entry_sub(&found, "execLogin", Some("ipb_member_id"), false);
        assert!(
            out.starts_with("export async function execLogin(hostArgs: Record<string, unknown>) {"),
            "got: {out}"
        );
        assert!(
            !out.contains("let ipb_member_id = hostArgs as Record<string, any>;"),
            "login has no info-hash slot — the first name must NOT be bound to the whole hostArgs \
             object, got: {out}"
        );
        assert!(
            out.contains("let ipb_member_id = hostArgs.arg as string;"),
            "got: {out}"
        );
        assert!(
            out.contains("let ipb_pass_hash = hostArgs.arg as string;"),
            "got: {out}"
        );
    }

    #[test]
    fn legacy_http_client_new_maps_to_perl_compat_user_agent() {
        // Bare `Mojo::UserAgent->new` (no parens) — the form the actual legacy corpus uses.
        let tokens = [word("Mojo::UserAgent"), op("->"), word("new")];
        let refs: Vec<&PpiNode> = tokens.iter().collect();
        let mut r = Renderer::new();
        assert_eq!(r.render_expr_sequence(&refs), "legacyCompat.userAgent()");

        // Also accepts the `->new()` parenthesized-empty-call spelling.
        let tokens_with_parens = [
            word("Mojo::UserAgent"),
            op("->"),
            word("new"),
            node("PPI::Structure::List", None, vec![]),
        ];
        let refs2: Vec<&PpiNode> = tokens_with_parens.iter().collect();
        let mut r2 = Renderer::new();
        assert_eq!(r2.render_expr_sequence(&refs2), "legacyCompat.userAgent()");
    }

    #[test]
    fn mojo_cookie_response_new_maps_to_a_plain_object_not_a_class_instance() {
        // `Mojo::Cookie::Response->new(name => 'x', value => $v)` — Mojo's hash-literal-shaped
        // constructor call; `legacyCompat.userAgent()`'s `cookie_jar.add` expects a plain
        // object, not `new Mojo::Cookie::Response(...)` (which also wouldn't be valid JS: `::`
        // isn't a legal identifier character).
        let list = node(
            "PPI::Structure::List",
            None,
            vec![node(
                "PPI::Statement::Expression",
                None,
                vec![
                    word("name"),
                    op("=>"),
                    ws(),
                    node("PPI::Token::Quote::Single", Some("'x'"), vec![]),
                    structure(","),
                    ws(),
                    word("value"),
                    op("=>"),
                    ws(),
                    symbol("$v"),
                ],
            )],
        );
        let tokens = [word("Mojo::Cookie::Response"), op("->"), word("new"), list];
        let refs: Vec<&PpiNode> = tokens.iter().collect();
        let mut r = Renderer::new();
        // Single-quoted Perl strings stay single-quoted (JS single-quotes don't interpolate
        // either, so no conversion is needed — see `strings::convert_string_content`), and the
        // outer `render_expr_sequence`'s own `tidy_spacing` pass strips the space just inside the
        // braces when this constructor is the sole top-level result, same as it would for any
        // other `{ ... }` produced by this renderer.
        assert_eq!(r.render_expr_sequence(&refs), "{name: 'x', value: v}");
    }

    #[test]
    fn mojo_get_result_chain_gets_awaited_and_makes_its_sub_async() {
        // `$ua->get($url)->result` — `legacyCompat.userAgent()`'s `get`/`post` are real
        // `fetch()` calls, so this chain needs `await` wrapped around the *receiver plus the
        // call* (`ua.get(url)`, not just `get(url)`), and the sub it appears in must become
        // `async function` (it isn't the mandatory entry point, which is already always async).
        let block = node(
            "PPI::Structure::Block",
            None,
            vec![node(
                "PPI::Statement::Variable",
                None,
                vec![
                    word("my"),
                    ws(),
                    symbol("$x"),
                    ws(),
                    op("="),
                    ws(),
                    symbol("$ua"),
                    op("->"),
                    word("get"),
                    node(
                        "PPI::Structure::List",
                        None,
                        vec![node(
                            "PPI::Statement::Expression",
                            None,
                            vec![symbol("$url")],
                        )],
                    ),
                    op("->"),
                    word("result"),
                    structure(";"),
                ],
            )],
        );
        let found = FoundSub {
            name: "helper".to_string(),
            block: &block,
            params: vec![],
        };
        let mut r = Renderer::new();
        let out = r.render_sub(&found);
        assert!(
            out.starts_with("async function helper(...args: any[]) {"),
            "got: {out}"
        );
        assert!(
            out.contains("let x = (await ua.get(url)).result;"),
            "got: {out}"
        );
    }

    #[test]
    fn helper_sub_without_await_stays_synchronous() {
        // Guards against `used_await` leaking across separate `render_sub` calls on the same
        // `Renderer` — a plain helper with no Mojo `get`/`post` usage must stay a non-async
        // `function`, even after an earlier sub on the same renderer did use `await`.
        let awaited_block = node(
            "PPI::Structure::Block",
            None,
            vec![node(
                "PPI::Statement::Variable",
                None,
                vec![
                    word("my"),
                    ws(),
                    symbol("$x"),
                    ws(),
                    op("="),
                    ws(),
                    symbol("$ua"),
                    op("->"),
                    word("get"),
                    node(
                        "PPI::Structure::List",
                        None,
                        vec![node(
                            "PPI::Statement::Expression",
                            None,
                            vec![symbol("$url")],
                        )],
                    ),
                    op("->"),
                    word("result"),
                    structure(";"),
                ],
            )],
        );
        let awaited_found = FoundSub {
            name: "does_await".to_string(),
            block: &awaited_block,
            params: vec![],
        };
        let mut r = Renderer::new();
        r.render_sub(&awaited_found);

        let plain_block = node(
            "PPI::Structure::Block",
            None,
            vec![node(
                "PPI::Statement::Variable",
                None,
                vec![
                    word("my"),
                    ws(),
                    symbol("$y"),
                    ws(),
                    op("="),
                    ws(),
                    node("PPI::Token::Number", Some("1"), vec![]),
                    structure(";"),
                ],
            )],
        );
        let plain_found = FoundSub {
            name: "plain".to_string(),
            block: &plain_block,
            params: vec![],
        };
        let out = r.render_sub(&plain_found);
        assert!(
            out.starts_with("function plain(...args: any[]) {"),
            "got: {out}"
        );
    }

    #[test]
    fn anonymous_sub_with_signature_becomes_an_arrow_function() {
        // `sub ($a, $b) { ... }` used as a *value* (e.g. a callback argument), not a top-level
        // named sub — PPI gives an anonymous sub's parameter list as a plain
        // `PPI::Structure::List` (verified against real `perl -MPPI::Dumper` output), not the
        // `PPI::Structure::Signature` a *named* sub's modern signature gets.
        // Note: no `ws()` tokens between top-level items here — every real caller of
        // `render_expr_sequence` first strips whitespace via `real_children` (see its docs),
        // so this mirrors what this function actually receives in practice.
        let mut r = Renderer::new();
        let tokens = [
            word("sub"),
            node(
                "PPI::Structure::List",
                None,
                vec![node(
                    "PPI::Statement::Expression",
                    None,
                    vec![symbol("$a"), op(","), ws(), symbol("$b")],
                )],
            ),
            node(
                "PPI::Structure::Block",
                None,
                vec![node(
                    "PPI::Statement",
                    None,
                    vec![symbol("$a"), structure(";")],
                )],
            ),
        ];
        let refs: Vec<&PpiNode> = tokens.iter().collect();
        let out = r.render_expr_sequence(&refs);
        assert!(
            out.contains("(a: any, b: any) =>"),
            "expected an arrow function with named params, got: {out}"
        );
        assert!(
            !out.contains("sub("),
            "must not call a function named sub: {out}"
        );
    }

    #[test]
    fn bareword_before_fat_comma_is_quoted_as_a_string() {
        // `$ua->on(start => sub {...})` — `start` is a bareword auto-quoted by Perl itself
        // because it's immediately followed by `=>`; outside this, an unquoted bareword renders
        // as a (here undeclared) JS identifier instead of the string literal Perl actually means.
        let mut r = Renderer::new();
        let tokens = [word("start"), op("=>")];
        let refs: Vec<&PpiNode> = tokens.iter().collect();
        let out = r.render_expr_sequence(&refs);
        assert!(out.starts_with("\"start\""), "got: {out}");
    }

    #[test]
    fn get_version_bareword_call_maps_to_perl_compat() {
        // `get_version` (imported via `use LANraragi::Utils::Generic qw(get_version)`) called
        // with no parens at all — legal Perl once imported, but indistinguishable at the token
        // level from a plain bareword, so it can't be routed through the generic "Word +
        // `Structure::List`" call-matching path the way every parenthesized call is.
        let mut r = Renderer::new();
        let tokens = [word("get_version")];
        let refs: Vec<&PpiNode> = tokens.iter().collect();
        let out = r.render_expr_sequence(&refs);
        assert_eq!(out, "legacyCompat.getVersion()");
    }

    #[test]
    fn package_qualified_call_is_sanitized_instead_of_emitting_invalid_double_colons() {
        // `LANraragi::Utils::Database::get_tags($lrr_gid)` — a real line from
        // `Metadata/CopyArchiveTags.pm`. `try_match_call` runs before `render_token`'s own
        // `::`-sanitizing fallback ever sees this word, so without its own sanitizing it emitted
        // the raw `::` straight through, a syntax error deno's parser rejected outright (worse
        // than the usual "unresolved name" case every other unmappable external reference gets).
        let mut r = Renderer::new();
        let tokens = [
            word("LANraragi::Utils::Database::get_tags"),
            node(
                "PPI::Structure::List",
                None,
                vec![node(
                    "PPI::Statement::Expression",
                    None,
                    vec![symbol("$lrr_gid")],
                )],
            ),
        ];
        let refs: Vec<&PpiNode> = tokens.iter().collect();
        let out = r.render_expr_sequence(&refs);
        assert_eq!(out, "LANraragi_Utils_Database_get_tags(lrr_gid)");
        assert!(r
            .warnings
            .iter()
            .any(|w| w.contains("LANraragi::Utils::Database::get_tags")));
    }

    #[test]
    fn named_capture_magic_vars_map_to_the_matchs_groups_property() {
        // `%+` (bare, e.g. `my %captures = %+;`) and `$+{'name'}` (subscripted, e.g.
        // `$+{'ttags'}`) — Perl's named-capture-group magic variables, populated by the most
        // recent successful `=~` match. Real corpus case: `Metadata/RegexParse.pm`. Previously
        // `strip_sigil` reduced both to the bare identifier `+`, silently producing a nonsense
        // unary-plus expression instead of a real error or a working translation.
        assert_eq!(render_magic("%+"), "(match?.groups ?? {})");
        assert_eq!(render_magic("$+"), "(match?.groups ?? {})");
    }

    #[test]
    fn split_delimited_pattern_handles_bare_slash_and_explicit_brace_forms() {
        assert_eq!(split_delimited_pattern("/foo/gi"), Some(("foo", "gi")));
        assert_eq!(split_delimited_pattern("{foo}gi"), Some(("foo", "gi")));
        // Nested braces inside the pattern (e.g. a `{2,3}` quantifier) must not be mistaken for
        // the closing delimiter.
        assert_eq!(
            split_delimited_pattern("{fo{2}o}gi"),
            Some(("fo{2}o", "gi"))
        );
    }

    #[test]
    fn render_perl_regex_literal_handles_explicit_m_and_qr_prefixes() {
        let m_brace = node(
            "PPI::Token::Regexp::Match",
            Some("m{^https://www\\.}i"),
            vec![],
        );
        assert_eq!(
            render_perl_regex_literal(&m_brace),
            Some("/^https:\\/\\/www\\./i".to_string())
        );
        let qr = node("PPI::Token::QuoteLike::Regexp", Some("qr/foo/i"), vec![]);
        assert_eq!(render_perl_regex_literal(&qr), Some("/foo/i".to_string()));
    }

    #[test]
    fn render_perl_regex_literal_folds_a_leading_inline_modifier_into_flags() {
        // `m/(?i)^(artist|...)/` — a real line from `Metadata/HDoujin.pm`, twice over. JS has no
        // equivalent to Perl's bare `(?i)` inline-modifier-prefix syntax (only a scoped
        // `(?i:...)` group, which JS also doesn't support) — left as-is, it's a real syntax error
        // deno's parser rejects outright. Folding the recognized letters into the JS literal's own
        // trailing flags and dropping the `(?i)` prefix from the pattern body is equivalent here
        // since it's the very first thing in the pattern (applies for its whole remaining length).
        let m = node("PPI::Token::Regexp::Match", Some("m/(?i)^foo/"), vec![]);
        assert_eq!(render_perl_regex_literal(&m), Some("/^foo/i".to_string()));
    }

    #[test]
    fn split_substitute_parts_handles_same_char_and_paired_delimiters() {
        assert_eq!(split_substitute_parts("/_/ /g"), Some(("_", " ", "g")));
        assert_eq!(
            split_substitute_parts("{^https://www\\.}{}i"),
            Some(("^https://www\\.", "", "i"))
        );
    }

    #[test]
    fn contains_recursive_subpattern_detects_perl_only_syntax() {
        assert!(contains_recursive_subpattern(r"\[([^\[\]]|(?0))*]"));
        assert!(contains_recursive_subpattern("(?R)"));
        assert!(!contains_recursive_subpattern(r"foo(bar)baz"));
    }

    #[test]
    fn regex_substitute_mutates_in_place_without_the_r_flag() {
        // `$tag =~ s/_/ /g;` — a real line from `Metadata/ChaikaFile.pm`.
        let block = node(
            "PPI::Structure::Block",
            None,
            vec![node(
                "PPI::Statement",
                None,
                vec![
                    symbol("$tag"),
                    op("=~"),
                    node("PPI::Token::Regexp::Substitute", Some("s/_/ /g"), vec![]),
                    structure(";"),
                ],
            )],
        );
        let found = FoundSub {
            name: "f".to_string(),
            block: &block,
            params: vec![],
        };
        let mut r = Renderer::new();
        let out = r.render_sub(&found);
        assert!(
            out.contains(r#"(tag = tag.replace(/_/g, " "))"#),
            "got: {out}"
        );
    }

    #[test]
    fn regex_substitute_with_r_flag_returns_a_new_value_without_mutating() {
        // `my $namespace = $name =~ s/\d+$//r;` — a real line from `Metadata/RegexParse.pm`.
        let block = node(
            "PPI::Structure::Block",
            None,
            vec![node(
                "PPI::Statement::Variable",
                None,
                vec![
                    word("my"),
                    symbol("$namespace"),
                    op("="),
                    symbol("$name"),
                    op("=~"),
                    node("PPI::Token::Regexp::Substitute", Some(r"s/\d+$//r"), vec![]),
                    structure(";"),
                ],
            )],
        );
        let found = FoundSub {
            name: "f".to_string(),
            block: &block,
            params: vec![],
        };
        let mut r = Renderer::new();
        let out = r.render_sub(&found);
        assert!(
            out.contains(r#"name.replace(/\d+$/, "")"#) && !out.contains("(name ="),
            "expected a non-mutating replace, got: {out}"
        );
    }

    #[test]
    fn grep_with_parens_wraps_the_condition_as_an_it_callback() {
        // `grep( !m/date_added/, @tags )` — a real line from `Metadata/CopyArchiveTags.pm`. Bare
        // `$_`-implicit matches inside `grep`'s condition map to `it` (`render_magic`'s existing
        // convention for `$_`), so the whole condition needs wrapping in `(it) => (...)` for that
        // binding to be a real parameter rather than a bare, unbound boolean expression.
        let tokens = [
            word("grep"),
            node(
                "PPI::Structure::List",
                None,
                vec![node(
                    "PPI::Statement::Expression",
                    None,
                    vec![
                        node("PPI::Token::Operator", Some("!"), vec![]),
                        node("PPI::Token::Regexp::Match", Some("m/date_added/"), vec![]),
                        op(","),
                        symbol("@tags"),
                    ],
                )],
            ),
        ];
        let refs: Vec<&PpiNode> = tokens.iter().collect();
        let mut r = Renderer::new();
        let out = r.render_expr_sequence(&refs);
        assert_eq!(out, "tags.filter((it) => (! /date_added/.test(it)))");
    }

    #[test]
    fn split_with_a_bare_regex_pattern_argument_renders_the_pattern_directly() {
        // `split(/,/, $value)` — a real line from `Metadata/HDoujin.pm`/`Metadata/RegexParse.pm`.
        // The bare regex here is `split`'s own pattern *argument*, not a boolean condition, so it
        // must NOT go through the generic per-token dispatch that turns a standalone
        // `Regexp::Match` into `/pattern/.test(it)` (correct only inside `grep`/`map` conditions) —
        // that bug previously produced `value.split(/,/.test(it))`.
        let tokens = [
            word("split"),
            node(
                "PPI::Structure::List",
                None,
                vec![node(
                    "PPI::Statement::Expression",
                    None,
                    vec![
                        node("PPI::Token::Regexp::Match", Some("/,/"), vec![]),
                        op(","),
                        symbol("$value"),
                    ],
                )],
            ),
        ];
        let refs: Vec<&PpiNode> = tokens.iter().collect();
        let mut r = Renderer::new();
        let out = r.render_expr_sequence(&refs);
        assert_eq!(out, "value.split(/,/)");
    }

    #[test]
    fn bare_multi_symbol_my_declaration_declares_each_variable() {
        // `my ( $title, $trailing_tags, $other_captures );` — a real line from
        // `Metadata/RegexParse.pm`: a multi-variable declaration with no initializer. Only the
        // single-symbol case (`my $x;`) was previously handled, so this fell through to generic
        // rendering and produced the invalid `my(title, trailing_tags, other_captures);`.
        let stmt = node(
            "PPI::Statement::Variable",
            None,
            vec![
                word("my"),
                node(
                    "PPI::Structure::List",
                    None,
                    vec![node(
                        "PPI::Statement::Expression",
                        None,
                        vec![
                            symbol("$title"),
                            op(","),
                            symbol("$trailing_tags"),
                            op(","),
                            symbol("$other_captures"),
                        ],
                    )],
                ),
                structure(";"),
            ],
        );
        let mut r = Renderer::new();
        let out = r.render_variable_statement(&stmt);
        assert_eq!(
            out,
            "let title = undefined;\nlet trailing_tags = undefined;\nlet other_captures = undefined;"
        );
    }

    #[test]
    fn bare_return_with_postfix_unless_wraps_in_an_if_block() {
        // `return unless $oneshot;` — a real line from `Metadata/CopyArchiveTags.pm`. PPI parses
        // any statement starting with `return` as `PPI::Statement::Break`, not
        // `PPI::Statement`/`::Expression`, so it never reached `try_render_postfix_modifier`'s
        // check at all — previously falling straight through to plain-return rendering with no
        // idea `unless` was a modifier keyword, emitting invalid JS like `return unless (cond);`.
        let block = node(
            "PPI::Structure::Block",
            None,
            vec![node(
                "PPI::Statement::Break",
                None,
                vec![
                    word("return"),
                    word("unless"),
                    symbol("$oneshot"),
                    structure(";"),
                ],
            )],
        );
        let found = FoundSub {
            name: "f".to_string(),
            block: &block,
            params: vec![],
        };
        let mut r = Renderer::new();
        let out = r.render_sub(&found);
        assert!(out.contains("if (!(oneshot)) { return; }"), "got: {out}");
    }

    #[test]
    fn return_value_with_postfix_if_wraps_in_an_if_block() {
        // `return $1 if length($1) == 40;` — a real line from `Metadata/CopyArchiveTags.pm`.
        let block = node(
            "PPI::Structure::Block",
            None,
            vec![node(
                "PPI::Statement::Break",
                None,
                vec![
                    word("return"),
                    symbol("$oneshot"),
                    word("if"),
                    symbol("$ready"),
                    structure(";"),
                ],
            )],
        );
        let found = FoundSub {
            name: "f".to_string(),
            block: &block,
            params: vec![],
        };
        let mut r = Renderer::new();
        let out = r.render_sub(&found);
        assert!(out.contains("if (ready) { return oneshot; }"), "got: {out}");
    }
}
