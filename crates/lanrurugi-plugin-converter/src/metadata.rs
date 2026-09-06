//! Converts a parsed `plugin_info` hash ([`crate::perl_value::PerlValue`]) into the TS
//! `pluginInfo()` shape our dispatcher/host actually expect — verified against
//! `crates/lanrurugi-plugin/src/protocol.rs::PluginInfo` (the Rust-side deserialization target)
//! and `apps/frontend/src/api/types.ts::PluginInfo` (what the Settings/Plugins UI reads):
//! `{ namespace, type, parameters: [{name, description, required}], declared_permissions,
//! name, author, description, version, icon?, oneshot_arg?, login_from? }`.
//!
//! Legacy's `parameters` has no per-parameter `name` (plugins receive them positionally via
//! `@_`) — this module synthesizes `param1`, `param2`, ... in array form, or uses the real key
//! as `name` when `parameters` is hash-shaped (the newer, better-documented convention some
//! plugins already use, e.g. `CopyArchiveTags.pm`).

use crate::perl_value::{hash_get, PerlValue};

pub struct ConvertedParameter {
    pub name: String,
    pub description: String,
}

pub struct ConvertedMetadata {
    pub namespace: String,
    pub kind: String,
    pub name: String,
    pub author: String,
    pub description: String,
    pub version: String,
    pub icon: Option<String>,
    pub oneshot_arg: Option<String>,
    /// Namespace of a `login`-type plugin to run before this one (e.g. `"ehlogin"`) — verified
    /// against real corpus usage (`~/LANraragi/lib/LANraragi/Plugin/{Metadata,Download}/*.pm`'s
    /// `login_from => "..."` field). `None` for the (large majority of) plugins that need no
    /// login at all.
    pub login_from: Option<String>,
    pub parameters: Vec<ConvertedParameter>,
    /// Heuristic hint, not a hard fact — see [`crate::permissions::guess_network_usage`].
    pub likely_makes_network_requests: bool,
    /// Heuristic hint, not a hard fact — see [`crate::permissions::guess_filesystem_write`].
    pub likely_writes_files: bool,
    /// Sidecar metadata filenames this plugin reads out of the archive it's processing —
    /// auto-detected from every `is_file_in_archive($archive, "name.ext")` call found in the
    /// source (`LANraragi::Utils::Archive`'s real-file-open idiom; see
    /// `render.rs::try_render_open_file_statement`/`render_named_call`'s `is_file_in_archive`
    /// case for how the *body* of such a call gets rewritten). The host resolves each declared
    /// name against the current archive before invoking `exec_metadata` — see
    /// `lanrurugi-plugin::protocol::PluginInfo::sidecar_files`'s own docs for the full mechanism.
    pub sidecar_files: Vec<String>,
}

/// Scans `source` for every `is_file_in_archive($archive_expr, "literal.ext")` call and collects
/// the literal filename arguments, in first-appearance order, deduplicated. Deliberately a plain
/// text scan rather than an AST-level one (this runs once per file on the *original* Perl source,
/// before any PPI dump is even requested).
///
/// The filename argument is a plain string literal in most of the real corpus, but
/// `Metadata/EHDLInfo.pm` instead passes a package-level `my $metadata_file = "info.txt";`
/// constant by name — so a second pass resolves any bare-variable argument back to its own
/// simple string-literal assignment earlier in the same file. Genuinely dynamic filenames (built
/// from string interpolation, concatenation, etc.) aren't resolvable this way and are silently
/// skipped — no real plugin in the corpus does that, and one that did would just get no
/// sidecar-file pre-fetch at all (falling back to whatever `hostArgs.sidecar_files?.[...]`
/// evaluates to, i.e. always `undefined`) rather than this converter guessing wrong.
fn detect_sidecar_files(source: &str) -> Vec<String> {
    static LITERAL_ARG_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"is_file_in_archive\s*\([^,]+,\s*["']([^"']+)["']"#).unwrap()
    });
    static VAR_ARG_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"is_file_in_archive\s*\([^,]+,\s*\$(\w+)\s*\)").unwrap()
    });

    let mut names = Vec::new();
    for cap in LITERAL_ARG_RE.captures_iter(source) {
        let name = cap[1].to_string();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    for cap in VAR_ARG_RE.captures_iter(source) {
        let var_name = &cap[1];
        let assign_re =
            regex::Regex::new(&format!(r#"\bmy\s+\${}\s*=\s*["']([^"']+)["']"#, var_name))
                .expect("var_name is \\w+, always a valid regex fragment");
        if let Some(assign_cap) = assign_re.captures(source) {
            let name = assign_cap[1].to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

pub fn convert(fields: &[(String, PerlValue)], source: &str) -> ConvertedMetadata {
    let get_str = |key: &str| -> String {
        hash_get(fields, key)
            .and_then(PerlValue::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let get_opt_str = |key: &str| -> Option<String> {
        hash_get(fields, key)
            .and_then(PerlValue::as_str)
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    };

    let parameters = match hash_get(fields, "parameters") {
        Some(PerlValue::Array(items)) => items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                let hash = item.as_hash()?;
                let desc = hash_get(hash, "desc")
                    .or_else(|| hash_get(hash, "description"))
                    .and_then(PerlValue::as_str)
                    .unwrap_or_default()
                    .to_string();
                Some(ConvertedParameter {
                    name: format!("param{}", i + 1),
                    description: desc,
                })
            })
            .collect(),
        Some(PerlValue::Hash(entries)) => entries
            .iter()
            .filter_map(|(key, value)| {
                let hash = value.as_hash()?;
                let desc = hash_get(hash, "desc")
                    .or_else(|| hash_get(hash, "description"))
                    .and_then(PerlValue::as_str)
                    .unwrap_or_default()
                    .to_string();
                Some(ConvertedParameter {
                    name: key.clone(),
                    description: desc,
                })
            })
            .collect(),
        _ => Vec::new(),
    };

    ConvertedMetadata {
        namespace: get_str("namespace"),
        kind: get_str("type"),
        name: get_str("name"),
        author: get_str("author"),
        description: get_str("description"),
        version: get_str("version"),
        icon: get_opt_str("icon"),
        oneshot_arg: get_opt_str("oneshot_arg"),
        login_from: get_opt_str("login_from"),
        parameters,
        likely_makes_network_requests: crate::permissions::guess_network_usage(source),
        likely_writes_files: crate::permissions::guess_filesystem_write(source),
        sidecar_files: detect_sidecar_files(source),
    }
}

/// Renders the converted metadata as a `pluginInfo()` TS function, in the same shape as
/// `crates/lanrurugi-plugin/samples/regex-filename-plugin.ts`'s own `pluginInfo()`.
pub fn render(meta: &ConvertedMetadata) -> String {
    let mut out = String::new();
    out.push_str("export function pluginInfo() {\n");
    out.push_str("  return {\n");
    out.push_str(&format!("    namespace: {:?},\n", meta.namespace));
    out.push_str(&format!("    type: {:?} as const,\n", meta.kind));
    out.push_str("    parameters: [\n");
    for param in &meta.parameters {
        out.push_str(&format!(
            "      {{ name: {:?}, description: {:?}, required: false }},\n",
            param.name, param.description
        ));
    }
    out.push_str("    ],\n");
    let write_field = if meta.likely_writes_files {
        "true"
    } else {
        "false"
    };
    if meta.likely_makes_network_requests {
        out.push_str(
            "    // TODO(perl-convert): source used an HTTP client (Mojo::UserAgent/LWP/etc.) — \n\
             \x20   // fill in the actual host(s) this plugin needs so Deno's --allow-net grant \n\
             \x20   // stays as narrow as possible (constitution Principle IV).\n",
        );
        out.push_str(&format!(
            "    declared_permissions: {{ net: [/* TODO: host(s) */], read: false, write: {write_field} }},\n"
        ));
    } else {
        out.push_str(&format!(
            "    declared_permissions: {{ net: [], read: false, write: {write_field} }},\n"
        ));
    }
    out.push_str(&format!("    name: {:?},\n", meta.name));
    out.push_str(&format!(
        "    author: {:?},\n",
        format!("{} (ported)", meta.author)
    ));
    out.push_str(&format!("    description: {:?},\n", meta.description));
    out.push_str(&format!("    version: {:?},\n", meta.version));
    if let Some(icon) = &meta.icon {
        out.push_str(&format!("    icon: {:?},\n", icon));
    }
    if let Some(oneshot) = &meta.oneshot_arg {
        out.push_str(&format!("    oneshot_arg: {:?},\n", oneshot));
    }
    if let Some(login_from) = &meta.login_from {
        out.push_str(&format!("    login_from: {:?},\n", login_from));
    }
    if !meta.sidecar_files.is_empty() {
        let quoted: Vec<String> = meta
            .sidecar_files
            .iter()
            .map(|f| format!("{f:?}"))
            .collect();
        out.push_str(&format!("    sidecar_files: [{}],\n", quoted.join(", ")));
    }
    out.push_str("  };\n");
    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perl_value::parse_plugin_info;

    #[test]
    fn converts_array_shaped_parameters_with_synthesized_names() {
        let src = r#"
            sub plugin_info {
                return (
                    name => "Ksk",
                    type => "metadata",
                    namespace => "kskyamlmeta",
                    author => "someone",
                    version => "0.1",
                    description => "desc",
                    parameters => [ { type => "bool", desc => "Assume english" } ]
                );
            }
        "#;
        let fields = parse_plugin_info(src).unwrap();
        let converted = convert(&fields, src);
        assert_eq!(converted.parameters.len(), 1);
        assert_eq!(converted.parameters[0].name, "param1");
        assert_eq!(converted.parameters[0].description, "Assume english");
        assert!(!converted.likely_makes_network_requests);
    }

    #[test]
    fn converts_hash_shaped_parameters_preserving_real_names() {
        let src = r#"
            sub plugin_info {
                return (
                    name => "X",
                    parameters => {
                        'copy_date_added' => { type => "bool", desc => "Enable to also copy" }
                    }
                );
            }
        "#;
        let fields = parse_plugin_info(src).unwrap();
        let converted = convert(&fields, src);
        assert_eq!(converted.parameters[0].name, "copy_date_added");
    }

    #[test]
    fn detects_network_usage_from_module_imports() {
        let src = "use Mojo::UserAgent;\nsub plugin_info { return (name => \"X\"); }";
        let fields = parse_plugin_info(src).unwrap();
        let converted = convert(&fields, src);
        assert!(converted.likely_makes_network_requests);
    }

    #[test]
    fn parses_and_renders_login_from() {
        // Real corpus shape, e.g. `~/LANraragi/lib/LANraragi/Plugin/Metadata/EHentai.pm`.
        let src = r#"
            sub plugin_info {
                return (
                    name => "E-Hentai",
                    type => "metadata",
                    namespace => "ehplugin",
                    login_from => "ehlogin",
                );
            }
        "#;
        let fields = parse_plugin_info(src).unwrap();
        let converted = convert(&fields, src);
        assert_eq!(converted.login_from.as_deref(), Some("ehlogin"));
        assert!(render(&converted).contains("login_from: \"ehlogin\","));
    }

    #[test]
    fn omits_login_from_when_the_plugin_declares_none() {
        let src = r#"
            sub plugin_info {
                return ( name => "X", type => "metadata", namespace => "x" );
            }
        "#;
        let fields = parse_plugin_info(src).unwrap();
        let converted = convert(&fields, src);
        assert_eq!(converted.login_from, None);
        assert!(!render(&converted).contains("login_from"));
    }

    #[test]
    fn render_produces_valid_looking_ts_object_literal() {
        let meta = ConvertedMetadata {
            namespace: "test".into(),
            kind: "metadata".into(),
            name: "Test".into(),
            author: "Someone".into(),
            description: "A test plugin".into(),
            version: "1.0".into(),
            icon: None,
            oneshot_arg: None,
            login_from: None,
            parameters: vec![],
            likely_makes_network_requests: false,
            likely_writes_files: false,
            sidecar_files: vec![],
        };
        let rendered = render(&meta);
        assert!(rendered.contains("export function pluginInfo()"));
        assert!(rendered.contains("namespace: \"test\""));
        assert!(rendered.contains("declared_permissions: { net: [], read: false, write: false }"));
    }

    #[test]
    fn detects_sidecar_file_from_a_string_literal_argument() {
        // `Metadata/ChaikaFile.pm`'s real shape.
        let src = r#"
            sub plugin_info { return ( name => "X", type => "metadata", namespace => "x" ); }
            sub get_tags {
                my $path_in_archive = is_file_in_archive( $lrr_info->{file_path}, "api.json" );
            }
        "#;
        let fields = parse_plugin_info(src).unwrap();
        let converted = convert(&fields, src);
        assert_eq!(converted.sidecar_files, vec!["api.json".to_string()]);
        assert!(render(&converted).contains(r#"sidecar_files: ["api.json"],"#));
    }

    #[test]
    fn detects_sidecar_file_from_a_package_constant_argument() {
        // `Metadata/EHDLInfo.pm`'s real shape: the filename is a `my $var = "literal";` constant,
        // not passed as a literal directly at the call site.
        let src = r#"
            my $metadata_file = "info.txt";
            sub plugin_info { return ( name => "X", type => "metadata", namespace => "x" ); }
            sub get_tags {
                if ( is_file_in_archive( $archive, $metadata_file ) ) { }
            }
        "#;
        let fields = parse_plugin_info(src).unwrap();
        let converted = convert(&fields, src);
        assert_eq!(converted.sidecar_files, vec!["info.txt".to_string()]);
    }

    #[test]
    fn no_sidecar_files_field_when_the_plugin_never_reads_one() {
        let src = r#"
            sub plugin_info { return ( name => "X", type => "metadata", namespace => "x" ); }
        "#;
        let fields = parse_plugin_info(src).unwrap();
        let converted = convert(&fields, src);
        assert!(converted.sidecar_files.is_empty());
        assert!(!render(&converted).contains("sidecar_files"));
    }
}
