//! Locating real font files on disk so [`crate::composite`] has something to draw with (T029/T034).
//!
//! [`crate::composite::FontSet`] borrows its fonts (`FontRef<'a>`), so the bytes have to outlive
//! every render. [`FontLibrary`] owns those bytes and hands out a borrowed `FontSet` per call —
//! loaded once at startup, not per page, since parsing a CJK font is far from free.
//!
//! A missing font is recoverable, never fatal: with no usable file at all the caller reports
//! translation-unavailable (FR-019) and untranslated reading is untouched.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ab_glyph::FontRef;
use lanrurugi_fontcache::FontId;

use crate::composite::FontSet;

/// Overrides the search path when set — a directory of font files.
pub const FONT_DIR_ENV: &str = "LANRURUGI_TRANSLATION_FONT_DIR";

/// Candidate files for the fallback face, best (CJK-capable) first. A translated page is usually
/// Latin text, but the source language's own characters can survive into a translation (a name
/// left untranslated, a retained honorific), so a CJK-capable face is preferred.
const FALLBACK_CANDIDATES: [&str; 8] = [
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJKjp-Regular.otf",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
];

/// Per-style preferences, keyed by the classifier's own style ids
/// (`lanrurugi_fontcache::classify::STYLE_*`). Each entry is tried in order; whatever is present
/// wins, and anything unresolved simply falls back — a style with no dedicated face still renders.
fn style_candidates(style: &str) -> &'static [&'static str] {
    match style {
        lanrurugi_fontcache::classify::STYLE_DIALOGUE => &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ],
        lanrurugi_fontcache::classify::STYLE_EMPHASIS => &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
        ],
        lanrurugi_fontcache::classify::STYLE_SFX => &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Black.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        ],
        lanrurugi_fontcache::classify::STYLE_NARRATION => &[
            "/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc",
            "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
        ],
        _ => &[],
    }
}

/// Font bytes owned for the process lifetime, so borrowed [`FontSet`]s can be handed out freely.
pub struct FontLibrary {
    fallback: Vec<u8>,
    by_style: BTreeMap<String, Vec<u8>>,
}

impl std::fmt::Debug for FontLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontLibrary")
            .field("styles", &self.by_style.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// Reads the first candidate that exists and parses as a font.
fn first_usable(candidates: &[&str], extra_dir: Option<&Path>) -> Option<Vec<u8>> {
    // A directory override is searched first and by filename, so an operator can drop in faces
    // without matching any distro's own layout.
    if let Some(dir) = extra_dir {
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut files: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| matches!(e, "ttf" | "otf" | "ttc"))
                })
                .collect();
            files.sort();
            for path in files {
                if let Ok(bytes) = std::fs::read(&path) {
                    if FontRef::try_from_slice(&bytes).is_ok() {
                        return Some(bytes);
                    }
                }
            }
        }
    }

    for path in candidates {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if FontRef::try_from_slice(&bytes).is_ok() {
            return Some(bytes);
        }
    }
    None
}

impl FontLibrary {
    /// Discovers a fallback face plus whatever per-style faces exist.
    ///
    /// Returns `None` when not even a fallback could be found — compositing is impossible then, and
    /// the caller degrades to untranslated reading rather than rendering blank boxes.
    pub fn discover() -> Option<Self> {
        let dir = std::env::var_os(FONT_DIR_ENV).map(PathBuf::from);
        let fallback = first_usable(&FALLBACK_CANDIDATES, dir.as_deref())?;

        let mut by_style = BTreeMap::new();
        for style in [
            lanrurugi_fontcache::classify::STYLE_DIALOGUE,
            lanrurugi_fontcache::classify::STYLE_EMPHASIS,
            lanrurugi_fontcache::classify::STYLE_SFX,
            lanrurugi_fontcache::classify::STYLE_NARRATION,
        ] {
            // No `extra_dir` here: the override directory supplies the fallback face, and picking
            // an arbitrary file from it for every style would make all four identical anyway.
            if let Some(bytes) = first_usable(style_candidates(style), None) {
                by_style.insert(style.to_string(), bytes);
            }
        }

        Some(Self { fallback, by_style })
    }

    /// Builds a borrowed [`FontSet`] covering `golden_set`.
    ///
    /// Styles with no dedicated face are simply absent, which `FontSet::resolve` already handles by
    /// falling back — a missing face must never block rendering the translation itself. `FontSet`
    /// takes raw bytes (not a pre-parsed `FontRef`) since it now needs to parse each face through
    /// two independent font crates (`ab_glyph` for rasterization, `harfrust` for shaping).
    pub fn font_set(&self, golden_set: &[FontId]) -> Option<FontSet<'_>> {
        if FontRef::try_from_slice(&self.fallback).is_err() {
            return None;
        }
        let mut set = FontSet::new(&self.fallback);

        for font_id in golden_set {
            if let Some(bytes) = self.by_style.get(font_id.as_str()) {
                if FontRef::try_from_slice(bytes).is_ok() {
                    set = set.with_font(font_id.as_str(), bytes);
                }
            }
        }
        Some(set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_directory_override_is_ignored_rather_than_fatal() {
        let missing = PathBuf::from("/nonexistent-font-dir-for-tests");
        assert!(first_usable(&[], Some(&missing)).is_none());
    }

    #[test]
    fn every_known_style_has_a_candidate_list() {
        for style in [
            lanrurugi_fontcache::classify::STYLE_DIALOGUE,
            lanrurugi_fontcache::classify::STYLE_EMPHASIS,
            lanrurugi_fontcache::classify::STYLE_SFX,
            lanrurugi_fontcache::classify::STYLE_NARRATION,
        ] {
            assert!(
                !style_candidates(style).is_empty(),
                "{style} has no font candidates"
            );
        }
        assert!(style_candidates("not-a-style").is_empty());
    }

    #[test]
    fn a_discovered_library_can_build_a_font_set() {
        // Skipped on a machine with no usable font at all, same policy as composite.rs's own tests.
        let Some(library) = FontLibrary::discover() else {
            return;
        };
        let golden = vec![FontId::from(
            lanrurugi_fontcache::classify::STYLE_DIALOGUE.to_string(),
        )];
        assert!(library.font_set(&golden).is_some());
    }
}
