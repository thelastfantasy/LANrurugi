//! Invokes `dump_ast.pl` (embedded via `include_str!`, written to a temp file at runtime — same
//! pattern `lanrurugi_plugin::DISPATCHER_SCRIPT` already uses for the Deno dispatcher script) to
//! parse a `.pm` file with PPI (Perl's own AST library) and deserialize the resulting JSON tree.
//!
//! Requires `perl` + the `PPI`/`JSON::PP` CPAN modules on `PATH` — genuinely parsing Perl needs a
//! real Perl, which is why this shells out rather than reimplementing a Perl parser in Rust. See
//! `Dockerfile.build` at the repo root for a ready-made environment with both the Rust toolchain
//! and this Perl/PPI setup; the host itself never needs Perl installed.

use std::path::Path;
use std::process::Command;

use serde::Deserialize;

pub const DUMP_AST_SCRIPT: &str = include_str!("../ppi/dump_ast.pl");

#[derive(Debug, Clone, Deserialize)]
pub struct PpiNode {
    pub class: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub children: Option<Vec<PpiNode>>,
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub finish: Option<String>,
}

impl PpiNode {
    pub fn children(&self) -> &[PpiNode] {
        self.children.as_deref().unwrap_or(&[])
    }

    /// Reconstructs this node's exact original source text by concatenating every descendant
    /// token's `content` in order — used both for the "original Perl as a comment" fallback and
    /// for the token-source-text needed by anything the renderer doesn't specially recognize.
    pub fn source_text(&self) -> String {
        if let Some(content) = &self.content {
            return content.clone();
        }
        let mut out = String::new();
        if let Some(start) = &self.start {
            out.push_str(start);
        }
        for child in self.children() {
            out.push_str(&child.source_text());
        }
        if let Some(finish) = &self.finish {
            out.push_str(finish);
        }
        out
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PpiError {
    #[error("failed to write temporary dump_ast.pl: {0}")]
    WriteScript(#[source] std::io::Error),
    #[error("failed to invoke perl (is it installed? see Dockerfile.build): {0}")]
    Spawn(#[source] std::io::Error),
    #[error("perl/PPI failed to parse the file:\n{0}")]
    ParseFailed(String),
    #[error("failed to deserialize PPI's JSON output: {0}")]
    InvalidJson(#[from] serde_json::Error),
}

/// Parses `path` with PPI, returning the top-level document children.
pub fn parse_pm_file(path: &Path) -> Result<Vec<PpiNode>, PpiError> {
    let script_path = std::env::temp_dir().join("lanrurugi-plugin-converter-dump_ast.pl");
    std::fs::write(&script_path, DUMP_AST_SCRIPT).map_err(PpiError::WriteScript)?;

    let output = Command::new("perl")
        .arg(&script_path)
        .arg(path)
        .output()
        .map_err(PpiError::Spawn)?;

    if !output.status.success() {
        return Err(PpiError::ParseFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let nodes: Vec<PpiNode> = serde_json::from_slice(&output.stdout)?;
    Ok(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_a_leaf_token() {
        let json = r#"{"class": "PPI::Token::Word", "content": "sub"}"#;
        let node: PpiNode = serde_json::from_str(json).unwrap();
        assert_eq!(node.class, "PPI::Token::Word");
        assert_eq!(node.content.as_deref(), Some("sub"));
        assert!(node.children().is_empty());
    }

    #[test]
    fn deserializes_a_structure_with_start_and_finish() {
        let json =
            r#"{"class": "PPI::Structure::List", "start": "(", "finish": ")", "children": []}"#;
        let node: PpiNode = serde_json::from_str(json).unwrap();
        assert_eq!(node.start.as_deref(), Some("("));
        assert_eq!(node.finish.as_deref(), Some(")"));
    }

    #[test]
    fn source_text_reconstructs_original_from_leaf_tokens() {
        let json = r#"{
            "class": "PPI::Statement",
            "children": [
                {"class": "PPI::Token::Word", "content": "my"},
                {"class": "PPI::Token::Whitespace", "content": " "},
                {"class": "PPI::Token::Symbol", "content": "$x"}
            ]
        }"#;
        let node: PpiNode = serde_json::from_str(json).unwrap();
        assert_eq!(node.source_text(), "my $x");
    }

    #[test]
    fn source_text_includes_structure_delimiters() {
        let json = r#"{
            "class": "PPI::Structure::List",
            "start": "(",
            "finish": ")",
            "children": [{"class": "PPI::Token::Symbol", "content": "$x"}]
        }"#;
        let node: PpiNode = serde_json::from_str(json).unwrap();
        assert_eq!(node.source_text(), "($x)");
    }
}
