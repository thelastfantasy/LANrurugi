//! Archive metadata the message schemas do not name, carried *after* the postcard message.
//!
//! postcard is positional: a new struct field shifts every byte after it, so adding one to
//! `ArchiveEntry` would make an older peer misread the rest of the frame. Trailing bytes are a
//! different story — `postcard::from_bytes` stops after the message and never looks at what
//! follows — so the additive fields ride a trailer inside the same frame body. An older peer
//! decodes exactly the message it always did and simply never sees them; a newer peer reads the
//! remainder and applies the names it knows. This is what keeps `SHARE_ENGINE_VERSION` at 4: an
//! older build refuses nothing, it just imports without the extras.

use serde::{Deserialize, Serialize};

/// [`ArchiveExtras::fields`] key for the archive's `primary_source` — the URL its metadata came
/// from, which nothing in the bytes can give back.
pub const EXTRA_PRIMARY_SOURCE: &str = "primary_source";

/// One archive's unnamed fields, keyed by the sharer's archive id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveExtras {
    pub archive_id: String,
    /// `(name, value)` pairs. Both strings so any later field fits without touching this shape.
    pub fields: Vec<(String, String)>,
}

impl ArchiveExtras {
    /// One archive's `primary_source`, if the sharer sent one.
    pub fn primary_source(&self) -> Option<&str> {
        self.fields
            .iter()
            .find(|(name, _)| name == EXTRA_PRIMARY_SOURCE)
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
    }

    /// The pairs this build does not know, kept verbatim.
    pub fn unknown(&self) -> Vec<(String, String)> {
        self.fields
            .iter()
            .filter(|(name, _)| name != EXTRA_PRIMARY_SOURCE)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extras() -> ArchiveExtras {
        ArchiveExtras {
            archive_id: "a".into(),
            fields: vec![
                (EXTRA_PRIMARY_SOURCE.into(), "nhentai.net/g/1".into()),
                ("something_newer".into(), "kept".into()),
            ],
        }
    }

    #[test]
    fn the_source_is_read_and_the_rest_kept() {
        let extras = extras();
        assert_eq!(extras.primary_source(), Some("nhentai.net/g/1"));
        assert_eq!(
            extras.unknown(),
            vec![("something_newer".to_string(), "kept".to_string())]
        );
    }

    #[test]
    fn an_empty_source_is_not_a_source() {
        let extras = ArchiveExtras {
            archive_id: "a".into(),
            fields: vec![(EXTRA_PRIMARY_SOURCE.into(), "  ".into())],
        };
        assert!(extras.primary_source().is_none());
        assert!(extras.unknown().is_empty());
    }
}
