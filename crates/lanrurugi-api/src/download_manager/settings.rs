//! Merges a download plugin's own declared `pluginOptions()` defaults with a user's persisted
//! override (`lanrurugi_storage::plugin_options`) into one effective-settings view — the shape
//! `GET`/`PUT`/`DELETE /api/plugins/{namespace}/options` all return
//! (`specs/005-download-plugin-progress/contracts/download-settings-api.md`), and the
//! `Vec<domain_rules::DomainRule>` snapshot `download_url` resolves once per download (spec
//! FR-016).

use lanrurugi_plugin::protocol::PluginOptionsResult as PluginDeclaredOptions;
use lanrurugi_storage::plugin_options::PluginOptionsOverride;
use serde::Serialize;

use super::domain_rules::DomainRule;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    PluginDefault,
    UserOverride,
}

#[derive(Debug, Clone, Serialize)]
pub struct EffectiveDomainRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes_per_sec: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub source: Source,
}

#[derive(Debug, Clone, Serialize)]
pub struct EffectiveBundleAsArchive {
    pub value: bool,
    pub default: bool,
    pub description: String,
    pub source: Source,
}

#[derive(Debug, Clone, Serialize)]
pub struct EffectiveOverwriteOnDuplicate {
    pub value: bool,
    pub default: bool,
    pub description: String,
    pub source: Source,
}

#[derive(Debug, Clone, Serialize)]
pub struct EffectivePluginOptions {
    pub namespace: String,
    pub domain_rules: Vec<EffectiveDomainRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_as_archive: Option<EffectiveBundleAsArchive>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overwrite_on_duplicate: Option<EffectiveOverwriteOnDuplicate>,
}

/// Merges `declared` (fresh from the plugin's own `pluginOptions()` call) with `override_`
/// (the user's persisted customization, if any) into the effective-settings response shape.
///
/// A user override entirely *replaces* `domain_rules` rather than merging rule-by-rule (matching
/// `PUT`'s own contract: "a field omitted from the request body is left at its current effective
/// value" — `domain_rules` as a whole is one such field, not a per-rule merge) — whichever list
/// the user last saved is authoritative in full once they've touched it at all.
pub fn merge(
    namespace: &str,
    declared: &PluginDeclaredOptions,
    override_: Option<&PluginOptionsOverride>,
) -> EffectivePluginOptions {
    let domain_rules = match override_.and_then(|o| o.domain_rules.as_ref()) {
        Some(overridden) => overridden
            .iter()
            .map(|r| EffectiveDomainRule {
                pattern: r.pattern.clone(),
                max_concurrent: r.max_concurrent,
                max_bytes_per_sec: r.max_bytes_per_sec,
                description: None,
                source: Source::UserOverride,
            })
            .collect(),
        None => declared
            .domain_rules
            .iter()
            .map(|r| EffectiveDomainRule {
                pattern: r.pattern.clone(),
                max_concurrent: r.max_concurrent,
                max_bytes_per_sec: r.max_bytes_per_sec,
                description: r.description.clone(),
                source: Source::PluginDefault,
            })
            .collect(),
    };

    let bundle_as_archive = declared.bundle_as_archive.as_ref().map(|declared_bundle| {
        match override_.and_then(|o| o.bundle_as_archive) {
            Some(value) => EffectiveBundleAsArchive {
                value,
                default: declared_bundle.default,
                description: declared_bundle.description.clone(),
                source: Source::UserOverride,
            },
            None => EffectiveBundleAsArchive {
                value: declared_bundle.default,
                default: declared_bundle.default,
                description: declared_bundle.description.clone(),
                source: Source::PluginDefault,
            },
        }
    });

    let overwrite_on_duplicate =
        declared
            .overwrite_on_duplicate
            .as_ref()
            .map(
                |declared_overwrite| match override_.and_then(|o| o.overwrite_on_duplicate) {
                    Some(value) => EffectiveOverwriteOnDuplicate {
                        value,
                        default: declared_overwrite.default,
                        description: declared_overwrite.description.clone(),
                        source: Source::UserOverride,
                    },
                    None => EffectiveOverwriteOnDuplicate {
                        value: declared_overwrite.default,
                        default: declared_overwrite.default,
                        description: declared_overwrite.description.clone(),
                        source: Source::PluginDefault,
                    },
                },
            );

    EffectivePluginOptions {
        namespace: namespace.to_string(),
        domain_rules,
        bundle_as_archive,
        overwrite_on_duplicate,
    }
}

/// Extracts just the `Vec<domain_rules::DomainRule>` snapshot `download_url` needs to actually
/// govern a download (spec FR-016) — the same merge as [`merge`], minus the response-only
/// `source`/`description` display metadata.
pub fn resolve_domain_rules(
    declared: &PluginDeclaredOptions,
    override_: Option<&PluginOptionsOverride>,
) -> Vec<DomainRule> {
    match override_.and_then(|o| o.domain_rules.as_ref()) {
        Some(overridden) => overridden
            .iter()
            .map(|r| DomainRule {
                pattern: r.pattern.clone(),
                max_concurrent: r.max_concurrent,
                max_bytes_per_sec: r.max_bytes_per_sec,
                description: None,
            })
            .collect(),
        None => declared
            .domain_rules
            .iter()
            .map(|r| DomainRule {
                pattern: r.pattern.clone(),
                max_concurrent: r.max_concurrent,
                max_bytes_per_sec: r.max_bytes_per_sec,
                description: r.description.clone(),
            })
            .collect(),
    }
}

/// Resolves the effective `bundle_as_archive` boolean a multi-resource download should use
/// (spec FR-018) — `false` for a plugin that doesn't declare this setting at all (a
/// single-resource-only plugin never reaches this: `run_managed_downloads` only calls it when
/// `downloads.len() > 1`).
pub fn resolve_bundle_as_archive(
    declared: &PluginDeclaredOptions,
    override_: Option<&PluginOptionsOverride>,
) -> bool {
    if let Some(value) = override_.and_then(|o| o.bundle_as_archive) {
        return value;
    }
    declared
        .bundle_as_archive
        .as_ref()
        .map(|b| b.default)
        .unwrap_or(false)
}

/// Resolves whether a download for this plugin should overwrite an existing colliding archive.
/// Unlike [`resolve_bundle_as_archive`], returns `None` (not a hardcoded `false`) when neither a
/// user override nor a plugin-declared default exists — the caller (the download-queue "start"
/// handler) is expected to fall back to the global `Settings.replacedupe` value in that case,
/// since "no opinion from this specific plugin" is meaningfully different from "this plugin
/// explicitly defaults to false."
pub fn resolve_overwrite_on_duplicate(
    declared: &PluginDeclaredOptions,
    override_: Option<&PluginOptionsOverride>,
) -> Option<bool> {
    if let Some(value) = override_.and_then(|o| o.overwrite_on_duplicate) {
        return Some(value);
    }
    declared.overwrite_on_duplicate.as_ref().map(|o| o.default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_plugin::protocol::{
        BundleAsArchiveOption, DomainRule as PluginDomainRule, OverwriteOnDuplicateOption,
    };
    use lanrurugi_storage::plugin_options::DomainRuleOverride;

    fn declared_with_one_rule() -> PluginDeclaredOptions {
        PluginDeclaredOptions {
            domain_rules: vec![PluginDomainRule {
                pattern: Some("*.example.com".to_string()),
                max_concurrent: Some(2),
                max_bytes_per_sec: None,
                description: Some("plugin default".to_string()),
            }],
            bundle_as_archive: Some(BundleAsArchiveOption {
                default: true,
                description: "bundle by default".to_string(),
            }),
            overwrite_on_duplicate: Some(OverwriteOnDuplicateOption {
                default: false,
                description: "overwrite by default".to_string(),
            }),
        }
    }

    #[test]
    fn no_override_uses_plugin_declared_defaults_verbatim() {
        let effective = merge("pixivdl", &declared_with_one_rule(), None);
        assert_eq!(effective.domain_rules.len(), 1);
        assert_eq!(effective.domain_rules[0].max_concurrent, Some(2));
        assert_eq!(effective.domain_rules[0].source, Source::PluginDefault);
        let bundle = effective.bundle_as_archive.unwrap();
        assert!(bundle.value);
        assert_eq!(bundle.source, Source::PluginDefault);
    }

    #[test]
    fn override_replaces_domain_rules_and_bundle_value() {
        let override_ = PluginOptionsOverride {
            domain_rules: Some(vec![DomainRuleOverride {
                pattern: Some("*.example.com".to_string()),
                max_concurrent: Some(5),
                max_bytes_per_sec: None,
            }]),
            bundle_as_archive: Some(false),
            overwrite_on_duplicate: None,
        };
        let effective = merge("pixivdl", &declared_with_one_rule(), Some(&override_));
        assert_eq!(effective.domain_rules[0].max_concurrent, Some(5));
        assert_eq!(effective.domain_rules[0].source, Source::UserOverride);
        let bundle = effective.bundle_as_archive.unwrap();
        assert!(!bundle.value);
        assert!(bundle.default, "plugin's own default is still reported");
        assert_eq!(bundle.source, Source::UserOverride);
    }

    #[test]
    fn override_replaces_overwrite_on_duplicate_value() {
        let override_ = PluginOptionsOverride {
            domain_rules: None,
            bundle_as_archive: None,
            overwrite_on_duplicate: Some(true),
        };
        let effective = merge("ehdl", &declared_with_one_rule(), Some(&override_));
        let overwrite = effective.overwrite_on_duplicate.unwrap();
        assert!(overwrite.value);
        assert!(!overwrite.default, "plugin's own default is still reported");
        assert_eq!(overwrite.source, Source::UserOverride);
    }

    #[test]
    fn no_override_reports_overwrite_on_duplicate_plugin_default() {
        let effective = merge("ehdl", &declared_with_one_rule(), None);
        let overwrite = effective.overwrite_on_duplicate.unwrap();
        assert!(!overwrite.value);
        assert_eq!(overwrite.source, Source::PluginDefault);
    }

    #[test]
    fn overwrite_on_duplicate_absent_when_plugin_declares_nothing() {
        let declared = PluginDeclaredOptions::default();
        let effective = merge("chaikadl", &declared, None);
        assert!(effective.overwrite_on_duplicate.is_none());
    }

    #[test]
    fn resolve_domain_rules_prefers_override_over_declared() {
        let override_ = PluginOptionsOverride {
            domain_rules: Some(vec![DomainRuleOverride {
                pattern: Some("*.example.com".to_string()),
                max_concurrent: Some(9),
                max_bytes_per_sec: Some(1000),
            }]),
            bundle_as_archive: None,
            overwrite_on_duplicate: None,
        };
        let resolved = resolve_domain_rules(&declared_with_one_rule(), Some(&override_));
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].max_concurrent, Some(9));
        assert_eq!(resolved[0].max_bytes_per_sec, Some(1000));
    }

    #[test]
    fn resolve_domain_rules_falls_back_to_declared_when_no_override() {
        let resolved = resolve_domain_rules(&declared_with_one_rule(), None);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].max_concurrent, Some(2));
    }

    #[test]
    fn resolve_bundle_as_archive_uses_plugin_default_when_no_override() {
        assert!(resolve_bundle_as_archive(&declared_with_one_rule(), None));
    }

    #[test]
    fn resolve_bundle_as_archive_prefers_user_override() {
        let override_ = PluginOptionsOverride {
            domain_rules: None,
            bundle_as_archive: Some(false),
            overwrite_on_duplicate: None,
        };
        assert!(!resolve_bundle_as_archive(
            &declared_with_one_rule(),
            Some(&override_)
        ));
    }

    #[test]
    fn resolve_bundle_as_archive_is_false_when_the_plugin_declares_nothing() {
        let declared = PluginDeclaredOptions::default();
        assert!(!resolve_bundle_as_archive(&declared, None));
    }

    #[test]
    fn resolve_overwrite_on_duplicate_prefers_user_override() {
        let override_ = PluginOptionsOverride {
            domain_rules: None,
            bundle_as_archive: None,
            overwrite_on_duplicate: Some(true),
        };
        assert_eq!(
            resolve_overwrite_on_duplicate(&declared_with_one_rule(), Some(&override_)),
            Some(true)
        );
    }

    #[test]
    fn resolve_overwrite_on_duplicate_uses_plugin_default_when_no_override() {
        assert_eq!(
            resolve_overwrite_on_duplicate(&declared_with_one_rule(), None),
            Some(false)
        );
    }

    #[test]
    fn resolve_overwrite_on_duplicate_is_none_when_the_plugin_declares_nothing() {
        let declared = PluginDeclaredOptions::default();
        assert_eq!(resolve_overwrite_on_duplicate(&declared, None), None);
    }
}
