//! A whole tankoubon as one streamed zip (specs/017-archive-sharing US3, FR-036–FR-040).
//!
//! Every member archive goes in as its original bytes, stored rather than re-compressed, under a
//! position-prefixed name so the entries list in tankoubon order. The zip is written by a blocking
//! task into a bounded duplex pipe whose read half is the response body, so the first bytes leave
//! immediately and no temporary copy of the tankoubon ever exists.

use std::collections::HashSet;
use std::io::{self, Write};
use std::path::PathBuf;

use tokio_util::io::{ReaderStream, SyncIoBridge};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Back-pressure window between the zip writer and the client.
const PIPE_CAPACITY: usize = 256 * 1024;

const TITLE_MAX_CHARS: usize = 150;

/// One member of the tankoubon, in reading order.
#[derive(Debug, Clone)]
pub enum MemberSource {
    /// The member's archive file. A sidecar `.patch.zip` is merged in, as `download_archive` does.
    File { title: String, path: PathBuf },
    /// The member's archive record is gone; only its id or title is known.
    Missing { title: String },
}

impl MemberSource {
    fn title(&self) -> &str {
        match self {
            Self::File { title, .. } | Self::Missing { title } => title,
        }
    }
}

/// Starts writing the zip on a blocking thread and returns its byte stream. Must be called from
/// inside the Tokio runtime.
pub fn stream_tankoubon_zip(members: Vec<MemberSource>) -> ReaderStream<tokio::io::DuplexStream> {
    let (writer, reader) = tokio::io::duplex(PIPE_CAPACITY);
    let bridge = SyncIoBridge::new(writer);
    tokio::task::spawn_blocking(move || {
        // An error here is almost always the client going away mid-download; the truncated body
        // is the only signal left to give.
        if let Err(e) = write_tankoubon_zip(bridge, &members) {
            tracing::debug!(error = %e, "tankoubon zip stream ended early");
        }
    });
    ReaderStream::new(reader)
}

/// Writes the whole zip to `out`. Members whose file cannot be opened are skipped and listed in a
/// final `MISSING.txt`.
pub fn write_tankoubon_zip<W: Write>(out: W, members: &[MemberSource]) -> io::Result<()> {
    let mut zip = ZipWriter::new_stream(out);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(true);
    let width = members.len().to_string().len().max(2);
    let mut used_names = HashSet::new();
    let mut missing = Vec::new();

    for (index, member) in members.iter().enumerate() {
        let position = index + 1;
        let MemberSource::File { path, .. } = member else {
            missing.push(missing_line(position, width, member.title()));
            continue;
        };
        let merged = lanrurugi_scanner::patch::build_merged_zip(path)
            .ok()
            .flatten();
        let extension = if merged.is_some() {
            "zip".to_string()
        } else {
            path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("zip")
                .to_string()
        };
        let name = unique_name(
            &mut used_names,
            member_entry_name(position, width, member.title(), &extension),
        );
        match merged {
            Some(bytes) => {
                zip.start_file(name, options).map_err(io::Error::other)?;
                zip.write_all(&bytes)?;
            }
            None => {
                let mut file = match std::fs::File::open(path) {
                    Ok(f) => f,
                    Err(_) => {
                        missing.push(missing_line(position, width, member.title()));
                        continue;
                    }
                };
                zip.start_file(name, options).map_err(io::Error::other)?;
                io::copy(&mut file, &mut zip)?;
            }
        }
    }

    if !missing.is_empty() {
        zip.start_file("MISSING.txt", options)
            .map_err(io::Error::other)?;
        let mut note = String::from("These members could not be read and are not included:\n");
        for line in &missing {
            note.push_str(line);
            note.push('\n');
        }
        zip.write_all(note.as_bytes())?;
    }
    let mut out = zip.finish().map_err(io::Error::other)?.into_inner();
    out.flush()
}

/// `NN - <title>.<ext>`, with `NN` zero-padded to `width`.
fn member_entry_name(position: usize, width: usize, title: &str, extension: &str) -> String {
    format!(
        "{position:0width$} - {}.{extension}",
        sanitize_title(title),
        width = width
    )
}

fn missing_line(position: usize, width: usize, title: &str) -> String {
    format!(
        "{position:0width$} - {}",
        sanitize_title(title),
        width = width
    )
}

/// Keeps a title usable as a file name on every common file system.
fn sanitize_title(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .take(TITLE_MAX_CHARS)
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Position prefixes already make names unique; this only guards against a future naming change.
fn unique_name(used: &mut HashSet<String>, name: String) -> String {
    if used.insert(name.to_lowercase()) {
        return name;
    }
    let (stem, ext) = name
        .rsplit_once('.')
        .map_or((name.as_str(), ""), |(s, e)| (s, e));
    (2..)
        .map(|n| format!("{stem}-{n}.{ext}"))
        .find(|candidate| used.insert(candidate.to_lowercase()))
        .expect("an unused suffix always exists")
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};

    use super::*;

    fn temp_member(dir: &std::path::Path, file: &str, bytes: &[u8], title: &str) -> MemberSource {
        let path = dir.join(file);
        std::fs::write(&path, bytes).unwrap();
        MemberSource::File {
            title: title.to_string(),
            path,
        }
    }

    fn unzip(bytes: Vec<u8>) -> zip::ZipArchive<Cursor<Vec<u8>>> {
        zip::ZipArchive::new(Cursor::new(bytes)).unwrap()
    }

    #[test]
    fn members_are_stored_in_order_with_padded_prefixes_and_identical_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let members = vec![
            temp_member(dir.path(), "a.cbz", b"first archive", "Vol: 1"),
            temp_member(dir.path(), "b.zip", b"second", "Vol 2"),
            temp_member(dir.path(), "c.rar", b"third!", "Vol 2"),
        ];
        let mut out = Vec::new();
        write_tankoubon_zip(&mut out, &members).unwrap();
        let mut archive = unzip(out);

        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert_eq!(
            names,
            ["01 - Vol_ 1.cbz", "02 - Vol 2.zip", "03 - Vol 2.rar"]
        );
        let expected: [&[u8]; 3] = [b"first archive", b"second", b"third!"];
        for (i, want) in expected.iter().enumerate() {
            let mut entry = archive.by_index(i).unwrap();
            assert_eq!(entry.compression(), CompressionMethod::Stored);
            let mut got = Vec::new();
            entry.read_to_end(&mut got).unwrap();
            assert_eq!(&got, want);
        }
    }

    #[test]
    fn missing_members_are_skipped_and_listed_last() {
        let dir = tempfile::tempdir().unwrap();
        let members = vec![
            temp_member(dir.path(), "a.zip", b"present", "Present"),
            MemberSource::File {
                title: "Gone from disk".into(),
                path: dir.path().join("nope.zip"),
            },
            MemberSource::Missing {
                title: "Deleted record".into(),
            },
        ];
        let mut out = Vec::new();
        write_tankoubon_zip(&mut out, &members).unwrap();
        let mut archive = unzip(out);

        assert_eq!(archive.len(), 2);
        assert_eq!(archive.by_index(0).unwrap().name(), "01 - Present.zip");
        let mut note = String::new();
        archive
            .by_name("MISSING.txt")
            .unwrap()
            .read_to_string(&mut note)
            .unwrap();
        assert!(note.contains("02 - Gone from disk"));
        assert!(note.contains("03 - Deleted record"));
    }

    #[test]
    fn padding_follows_the_member_count() {
        assert_eq!(member_entry_name(7, 3, "x", "zip"), "007 - x.zip");
        assert_eq!(member_entry_name(7, 2, "x", "zip"), "07 - x.zip");
    }

    #[test]
    fn titles_are_made_safe_for_file_names() {
        assert_eq!(sanitize_title("a/b\\c:d*e?"), "a_b_c_d_e_");
        assert_eq!(sanitize_title("  ...  "), "untitled");
        assert_eq!(sanitize_title("name."), "name");
        assert_eq!(
            sanitize_title(&"長".repeat(300)).chars().count(),
            TITLE_MAX_CHARS
        );
    }

    #[test]
    fn a_repeated_name_gets_a_numbered_suffix() {
        let mut used = HashSet::new();
        assert_eq!(unique_name(&mut used, "01 - a.zip".into()), "01 - a.zip");
        assert_eq!(unique_name(&mut used, "01 - A.zip".into()), "01 - A-2.zip");
    }
}
