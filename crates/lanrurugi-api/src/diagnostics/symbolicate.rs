//! Server-side symbolication by release (specs/018 FR-024/FR-025, research.md §7).
//!
//! The maps for a release live outside the served static tree (`LANRURUGI_SOURCEMAPS_DIR`), so the
//! browser never downloads them and a bundle never carries them. A frame is resolved on demand and
//! only the *result* leaves the process; when a release's map is absent, the frame stays raw and
//! the release is still named — the report is then resolvable by the maintainer instead.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use sourcemap::SourceMap;

/// One resolved frame, in the same textual shape the raw stack used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub function: Option<String>,
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// Resolves every frame it can in `stack`; frames it cannot resolve are returned unchanged.
///
/// `chunk` is the bundle-relative name (`assets/main-abc.js`), which is what a browser frame names.
pub fn resolve_stack(release: Option<&str>, stack: &str) -> String {
    let Some(release) = release else {
        return stack.to_string();
    };
    let root = maps_root();
    let mut lines = Vec::new();
    let mut changed = false;
    for line in stack.lines() {
        let resolved = resolve_line(&root, release, line);
        changed |= resolved != line;
        lines.push(resolved);
    }
    if !changed {
        return stack.to_string();
    }
    let mut out = lines.join("\n");
    if stack.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn resolve_line(root: &std::path::Path, release: &str, line: &str) -> String {
    let Some(frame) = parse_frame(line) else {
        return line.to_string();
    };
    let chunk = frame
        .file
        .rsplit('/')
        .next()
        .unwrap_or(&frame.file)
        .split('?')
        .next()
        .unwrap_or(&frame.file)
        .to_string();
    let Some(map) = load_map(&root.join(release).join(format!("{chunk}.map"))) else {
        return line.to_string();
    };
    // Source map line/column are 0-based; the stack's own numbers are 1-based.
    let line0 = frame.line.saturating_sub(1);
    let column0 = frame.column.saturating_sub(1);
    let Some(token) = map.lookup_token(line0, column0) else {
        return line.to_string();
    };
    let file = token
        .get_source()
        .map(|source| {
            source
                .rsplit('/')
                .take(3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_else(|| frame.file.clone());
    let resolved_line = token.get_src_line() + 1;
    let resolved_column = token.get_src_col() + 1;
    let function = token
        .get_name()
        .map(str::to_string)
        .or(frame.function.clone());
    match function {
        Some(function) => format!("    at {function} ({file}:{resolved_line}:{resolved_column})"),
        None => format!("    at {file}:{resolved_line}:{resolved_column}"),
    }
}

/// `at fn (file:line:col)`, `at file:line:col`, or a bare `file:line:col`.
fn parse_frame(line: &str) -> Option<Frame> {
    let trimmed = line.trim();
    let body = trimmed.strip_prefix("at ").unwrap_or(trimmed);
    let (function, location) = match body.split_once('(') {
        Some((function, rest)) => (
            Some(function.trim().to_string()).filter(|f| !f.is_empty()),
            rest.trim_end_matches(')').trim(),
        ),
        None => (None, body),
    };
    let mut parts = location.rsplitn(3, ':');
    let column = parts.next()?.parse::<u32>().ok()?;
    let line_no = parts.next()?.parse::<u32>().ok()?;
    let file = parts.next()?.to_string();
    if file.is_empty() {
        return None;
    }
    Some(Frame {
        function,
        file,
        line: line_no,
        column,
    })
}

fn maps_root() -> PathBuf {
    std::env::var("LANRURUGI_SOURCEMAPS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/usr/local/share/lanrurugi/sourcemaps"))
}

/// Parsed maps are cached: a bundle resolves many frames of the same chunk.
fn load_map(path: &std::path::Path) -> Option<Arc<SourceMap>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<Arc<SourceMap>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = cache.lock() {
        if let Some(hit) = cache.get(path) {
            return hit.clone();
        }
    }
    let loaded = std::fs::read(path)
        .ok()
        .and_then(|bytes| SourceMap::from_slice(&bytes).ok())
        .map(Arc::new);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(path.to_path_buf(), loaded.clone());
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_parsed_in_both_shapes() {
        let with_function = parse_frame("    at doThing (assets/main-abc.js:12:34)").unwrap();
        assert_eq!(with_function.function.as_deref(), Some("doThing"));
        assert_eq!(with_function.file, "assets/main-abc.js");
        assert_eq!((with_function.line, with_function.column), (12, 34));

        let bare = parse_frame("assets/main-abc.js:7:9").unwrap();
        assert_eq!(bare.function, None);
        assert_eq!((bare.line, bare.column), (7, 9));
    }

    #[test]
    fn a_stack_without_a_release_is_left_alone() {
        let stack = "at a (x.js:1:2)";
        assert_eq!(resolve_stack(None, stack), stack);
    }

    #[test]
    fn a_missing_map_keeps_the_raw_frame() {
        // No release directory exists in the test environment.
        let stack = "at a (assets/none-000.js:1:2)";
        assert_eq!(resolve_stack(Some("does-not-exist"), stack), stack);
    }

    #[test]
    fn a_real_map_resolves_to_the_source_line() {
        let dir = std::env::temp_dir().join(format!("lrrg-maps-{}", std::process::id()));
        let release = dir.join("abc123");
        std::fs::create_dir_all(&release).unwrap();
        // A one-line map: column 0 of the generated file maps to line 3, column 5 of `src/a.ts`.
        let map = r#"{"version":3,"file":"main.js","sources":["src/a.ts"],"names":["boom"],"mappings":"AAAA"}"#;
        std::fs::write(release.join("main.js.map"), map).unwrap();
        std::env::set_var("LANRURUGI_SOURCEMAPS_DIR", &dir);
        let resolved = resolve_stack(Some("abc123"), "    at main (main.js:1:1)");
        assert!(resolved.contains("src/a.ts"), "{resolved}");
        std::env::remove_var("LANRURUGI_SOURCEMAPS_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
