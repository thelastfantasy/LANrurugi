//! Helpers over the comma-separated `tags` string, currently limited to the `date_added:`
//! namespace. `date_added` is this library's own ingestion timestamp (written by the scanner when
//! an archive is catalogued), never importable metadata — so importing a legacy export has to
//! drop it, and an archive that ended up with two values needs them collapsed to one.

/// Whether `tag` is a `date_added:` tag. The namespace is compared case-insensitively (this app
/// always writes lowercase; a legacy export is tolerated either way).
pub fn is_date_added_tag(tag: &str) -> bool {
    tag.split_once(':')
        .is_some_and(|(namespace, _)| namespace.trim().eq_ignore_ascii_case("date_added"))
}

/// The newest `date_added:` tag in `tags`, exactly as spelled there (trimmed), or `None` when
/// there is none. Unparseable values rank below every parseable one; ties keep the first.
pub fn newest_date_added_tag(tags: &str) -> Option<String> {
    let mut best: Option<(&str, Option<u64>)> = None;
    for raw in tags.split(',') {
        let tag = raw.trim();
        if !is_date_added_tag(tag) {
            continue;
        }
        let parsed = tag
            .split_once(':')
            .and_then(|(_, value)| value.trim().parse::<u64>().ok());
        let keep = match best {
            None => true,
            Some((_, best_parsed)) => match (best_parsed, parsed) {
                (None, Some(_)) => true,
                (Some(best), Some(candidate)) => candidate > best,
                _ => false,
            },
        };
        if keep {
            best = Some((tag, parsed));
        }
    }
    best.map(|(tag, _)| tag.to_string())
}

/// `tags` without any `date_added:` tag. Every other segment is kept as written (a space that
/// preceded a removed segment goes with it), so a caller with nothing to strip gets its input back
/// unchanged.
pub fn strip_date_added_tags(tags: &str) -> String {
    if !tags.split(',').any(|tag| is_date_added_tag(tag.trim())) {
        return tags.to_string();
    }
    tags.split(',')
        .filter(|tag| !is_date_added_tag(tag.trim()))
        .collect::<Vec<_>>()
        .join(",")
}

/// Collapses every `date_added:` tag down to the newest one, keeping it in the position of the
/// first occurrence (so the rest of the tag order is untouched). Returns `tags` unchanged when
/// there is nothing to collapse.
pub fn collapse_duplicate_date_added(tags: &str) -> String {
    let segments: Vec<&str> = tags.split(',').collect();
    let first = segments
        .iter()
        .position(|tag| is_date_added_tag(tag.trim()));
    let Some(first) = first else {
        return tags.to_string();
    };
    if segments
        .iter()
        .filter(|tag| is_date_added_tag(tag.trim()))
        .count()
        < 2
    {
        return tags.to_string();
    }
    let newest = newest_date_added_tag(tags).unwrap_or_else(|| segments[first].trim().to_string());
    segments
        .iter()
        .enumerate()
        .filter_map(|(index, tag)| {
            if !is_date_added_tag(tag.trim()) {
                Some(*tag)
            } else if index == first {
                Some(newest.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_picks_the_largest_parsed_timestamp() {
        assert_eq!(
            newest_date_added_tag("artist:a,date_added:1742054739,date_added:1793928516"),
            Some("date_added:1793928516".to_string())
        );
    }

    #[test]
    fn newest_ignores_an_unparseable_value_when_a_parsed_one_exists() {
        assert_eq!(
            newest_date_added_tag("date_added:not-a-date,date_added:1793928516"),
            Some("date_added:1793928516".to_string())
        );
    }

    #[test]
    fn newest_keeps_an_unparseable_value_when_it_is_the_only_one() {
        assert_eq!(
            newest_date_added_tag("date_added:not-a-date"),
            Some("date_added:not-a-date".to_string())
        );
    }

    #[test]
    fn newest_matches_the_namespace_case_insensitively() {
        assert_eq!(
            newest_date_added_tag("Date_Added:1793928516"),
            Some("Date_Added:1793928516".to_string())
        );
    }

    #[test]
    fn strip_removes_every_date_added_and_keeps_the_rest_verbatim() {
        assert_eq!(
            strip_date_added_tags("artist:a, date_added:1742054739,date_added:1793928516,female:b"),
            "artist:a,female:b"
        );
    }

    #[test]
    fn strip_is_a_no_op_without_a_date_added_tag() {
        let tags = "artist:a,female:b";
        assert_eq!(strip_date_added_tags(tags), tags);
    }

    #[test]
    fn collapse_keeps_only_the_newest_at_the_first_occurrence() {
        assert_eq!(
            collapse_duplicate_date_added(
                "artist:a,date_added:1742054739,female:b,date_added:1793928516"
            ),
            "artist:a,date_added:1793928516,female:b"
        );
    }

    #[test]
    fn collapse_is_a_no_op_with_one_or_no_date_added_tag() {
        assert_eq!(
            collapse_duplicate_date_added("artist:a,date_added:1742054739"),
            "artist:a,date_added:1742054739"
        );
        assert_eq!(collapse_duplicate_date_added("artist:a"), "artist:a");
    }
}
