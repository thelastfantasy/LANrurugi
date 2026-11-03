//! `lanrurugi formats` — a read-only census of which page formats a library actually contains, and
//! how each fares: formats every browser renders, formats only *some* clients render (the reader's
//! own `lrr_img_support` capability signal decides per client, and the server converts the rest),
//! and formats **no** mainstream browser renders while this server cannot decode them either — for
//! those a page is simply unviewable, so they are the point of the report.
//!
//! The runtime counterpart of this table is the reader's capability model
//! (`lanrurugi_api::archives::ClientImageSupport`); this tool is the "do I even have such files?"
//! half, and is deliberately dependency-free (extensions first, magic bytes on a sample).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lanrurugi_scanner::archive_format;

#[derive(clap::Args)]
pub struct FormatsArgs {
    /// Archive library folder, same default as `serve`.
    #[arg(long, env = "LANRURUGI_LIBRARY_PATH", default_value = "./library")]
    library_path: PathBuf,

    /// How many entries per archive to open and identify by content, to catch pages whose
    /// extension lies (0 disables; each sample reads a whole entry).
    #[arg(long, default_value_t = 2)]
    magic_samples_per_archive: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Browser {
    /// Every mainstream browser renders it.
    Universal,
    /// Only clients that advertise the capability render it; the rest need a conversion.
    Conditional,
    /// No mainstream browser renders it.
    Never,
}

impl Browser {
    fn label(self) -> &'static str {
        match self {
            Browser::Universal => "all browsers",
            Browser::Conditional => "some browsers (capability-dependent)",
            Browser::Never => "no browser",
        }
    }
}

struct FormatFacts {
    label: &'static str,
    browser: Browser,
    /// Whether this server can decode the format and therefore convert it on demand.
    server_can_convert: bool,
    /// Anything a reader should know beyond the two flags above.
    caveat: Option<&'static str>,
}

const fn facts(
    label: &'static str,
    browser: Browser,
    server_can_convert: bool,
    caveat: Option<&'static str>,
) -> FormatFacts {
    FormatFacts {
        label,
        browser,
        server_can_convert,
        caveat,
    }
}

/// Extension-level knowledge. Content that lies about its extension is caught separately by the
/// magic-byte sample pass.
fn facts_for_extension(ext: &str) -> Option<FormatFacts> {
    Some(match ext {
        "jpg" | "jpeg" | "jpe" | "jfif" => facts("JPEG", Browser::Universal, true, None),
        "png" | "apng" => facts("PNG/APNG", Browser::Universal, true, None),
        "gif" => facts("GIF", Browser::Universal, true, None),
        "webp" => facts("WebP", Browser::Universal, true, None),
        "bmp" => facts("BMP", Browser::Universal, true, None),
        "svg" => facts("SVG", Browser::Universal, false, Some("served as-is")),
        "avif" => facts(
            "AVIF (still + sequences)",
            Browser::Conditional,
            true,
            Some("Chrome 85+/Edge 121+/Firefox 113+/Safari 16.4+; older Firefox/Safari are still-only. Still images are converted; sequences are passed through (no frame decoder)"),
        ),
        "jxl" => facts(
            "JPEG XL (still + sequences)",
            Browser::Conditional,
            true,
            Some("Chrome 155+/Firefox 158+; Safari decodes stills only; Edge none"),
        ),
        "heic" | "heif" | "hif" => facts(
            "HEIC/HEIF",
            Browser::Conditional,
            false,
            Some("Safari 17+/iOS 17+ only, and no decoder exists here (bookworm's libheif is older than libheif-rs requires)"),
        ),
        "jp2" | "j2k" | "jpf" | "jpx" | "jpm" => facts(
            "JPEG 2000",
            Browser::Never,
            true,
            Some("no browser renders it; decoded here by jpeg2k's pure-Rust openjp2 backend"),
        ),
        "mng" | "jng" => facts(
            "MNG/JNG",
            Browser::Never,
            false,
            Some("dead format; no decoder here"),
        ),
        "wp2" => facts(
            "WebP2",
            Browser::Never,
            false,
            Some("never shipped in a browser; no decoder here"),
        ),
        "tif" | "tiff" => facts(
            "TIFF",
            Browser::Never,
            true,
            Some("multi-page TIFF is served as its first frame only"),
        ),
        "mpo" => facts(
            "MPO (multi-picture JPEG)",
            Browser::Universal,
            true,
            Some("renders as its first frame, like a plain JPEG"),
        ),
        _ => return None,
    })
}

/// Magic-byte knowledge, for entries whose extension is missing or unrecognized.
fn facts_for_magic(bytes: &[u8]) -> Option<FormatFacts> {
    let ext = if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        match &bytes[8..12] {
            b"avif" | b"avis" => "avif",
            b"heic" | b"heix" | b"hevc" | b"mif1" | b"msf1" => "heic",
            b"jxl " => "jxl",
            _ => return None,
        }
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "jpg"
    } else if bytes.starts_with(b"\x89PNG") {
        "png"
    } else if bytes.starts_with(b"GIF8") {
        "gif"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp"
    } else if bytes.starts_with(b"BM") {
        "bmp"
    } else if bytes.starts_with(&[0x49, 0x49, 0x2a, 0x00])
        || bytes.starts_with(&[0x4d, 0x4d, 0x00, 0x2a])
    {
        "tiff"
    } else if bytes.starts_with(&[0x00, 0x00, 0x00, 0x0c, b'J', b'X', b'L', b' '])
        || bytes.starts_with(&[0xff, 0x0a])
    {
        // Both the container (`JXL ` box) and the bare codestream signature.
        "jxl"
    } else {
        return None;
    };
    facts_for_extension(ext)
}

struct Counts {
    pages: u64,
    archives: u64,
    examples: Vec<String>,
}

pub fn run(args: FormatsArgs) -> anyhow::Result<()> {
    let mut by_format: BTreeMap<&'static str, Counts> = BTreeMap::new();
    let mut unknown_ext: BTreeMap<String, Counts> = BTreeMap::new();
    let mut mismatches: BTreeMap<String, Counts> = BTreeMap::new();
    let mut archives = 0u64;
    let mut unreadable = 0u64;

    let mut entries: Vec<PathBuf> = std::fs::read_dir(&args.library_path)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .collect();
    entries.sort();

    for path in &entries {
        let listed = match archive_format::list_all_entries(path) {
            Ok(listed) => listed,
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        archives += 1;
        let archive_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        let pages: Vec<&archive_format::ArchiveEntryInfo> =
            listed.iter().filter(|entry| entry.is_page).collect();

        // Count this archive's pages locally first: "in how many archives" is a per-archive
        // question, and deriving it from the example list (which is capped at three names)
        // over-counted wildly — it reported more archives than the library has.
        let mut local: BTreeMap<&'static str, u64> = BTreeMap::new();
        let mut local_unknown: BTreeMap<String, u64> = BTreeMap::new();
        for entry in &pages {
            let ext = Path::new(&entry.name)
                .extension()
                .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            match facts_for_extension(&ext) {
                Some(facts) => *local.entry(facts.label).or_default() += 1,
                None => *local_unknown.entry(ext).or_default() += 1,
            }
        }
        for (label, count) in local {
            let slot = by_format.entry(label).or_insert_with(|| Counts {
                pages: 0,
                archives: 0,
                examples: Vec::new(),
            });
            slot.pages += count;
            slot.archives += 1;
            if slot.examples.len() < 3 {
                slot.examples.push(archive_name.clone());
            }
        }
        for (ext, count) in local_unknown {
            let slot = unknown_ext.entry(ext).or_insert_with(|| Counts {
                pages: 0,
                archives: 0,
                examples: Vec::new(),
            });
            slot.pages += count;
            slot.archives += 1;
            if slot.examples.len() < 3 {
                slot.examples.push(archive_name.clone());
            }
        }

        // Content check on a bounded sample: catches pages whose extension lies (a renamed HEIC
        // inside a zip is a real case) without reading a whole library.
        if args.magic_samples_per_archive > 0 && !pages.is_empty() {
            let step = (pages.len() / args.magic_samples_per_archive).max(1);
            for entry in pages
                .iter()
                .step_by(step)
                .take(args.magic_samples_per_archive)
            {
                let Ok(bytes) = archive_format::read_entry(path, &entry.name) else {
                    continue;
                };
                let Some(facts) = facts_for_magic(&bytes) else {
                    continue;
                };
                let ext = Path::new(&entry.name)
                    .extension()
                    .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                let declared = facts_for_extension(&ext).map(|facts| facts.label);
                if declared != Some(facts.label) {
                    let key = format!(
                        "declared {} / actually {}",
                        declared.unwrap_or(ext.as_str()),
                        facts.label
                    );
                    let slot = mismatches.entry(key).or_insert_with(|| Counts {
                        pages: 0,
                        archives: 0,
                        examples: Vec::new(),
                    });
                    slot.pages += 1;
                    slot.archives += 1;
                    if slot.examples.len() < 3 {
                        slot.examples
                            .push(format!("{archive_name} :: {}", entry.name));
                    }
                }
            }
        }
    }

    println!("Library: {}", args.library_path.display());
    println!(
        "Archives read: {archives} (unreadable: {unreadable})  Pages: {}",
        by_format.values().map(|c| c.pages).sum::<u64>()
            + unknown_ext.values().map(|c| c.pages).sum::<u64>()
    );
    println!("\nFormats found:");
    for (label, counts) in &by_format {
        let facts = all_facts()
            .into_iter()
            .find(|(name, _)| name == label)
            .map(|(_, facts)| facts);
        let (browser, converts, caveat) = match facts {
            Some(facts) => (
                facts.browser.label(),
                if facts.server_can_convert {
                    "convertible"
                } else {
                    "NOT convertible"
                },
                facts.caveat,
            ),
            None => ("?", "?", None),
        };
        println!(
            "  {label:<28} {:>7} pages in {:>4} archives   [{browser}; {converts}]",
            counts.pages, counts.archives
        );
        if counts.pages > 0 {
            println!("      e.g. {}", counts.examples.join(", "));
        }
        if let Some(caveat) = caveat {
            println!("      note: {caveat}");
        }
    }
    for (ext, counts) in &unknown_ext {
        println!(
            "  unrecognized extension .{ext:<14} {:>7} pages in {:>4} archives   [unknown format]",
            counts.pages, counts.archives
        );
        if counts.pages > 0 {
            println!("      e.g. {}", counts.examples.join(", "));
        }
    }

    // The point of the report: pages nothing can display.
    let mut gaps: Vec<(&'static str, &Counts, FormatFacts)> = all_facts()
        .into_iter()
        .filter(|(_, facts)| facts.browser != Browser::Universal && !facts.server_can_convert)
        .filter_map(|(label, facts)| {
            by_format
                .get(label)
                .filter(|counts| counts.pages > 0)
                .map(|counts| (label, counts, facts))
        })
        .collect();
    gaps.sort_by_key(|(_, counts, _)| std::cmp::Reverse(counts.pages));

    println!("\nUnviewable pages (no mainstream browser renders the format AND this server cannot convert it):");
    if gaps.is_empty() {
        println!("  none found — every page in this library is convertible or natively renderable");
    } else {
        for (label, counts, _) in &gaps {
            println!(
                "  {label:<28} {:>7} pages in {:>4} archives   e.g. {}",
                counts.pages,
                counts.archives,
                counts.examples.join(", ")
            );
        }
        println!(
            "  → these need a decoder for the format (or an external conversion) before any client can display them"
        );
    }

    if !mismatches.is_empty() {
        println!("\nExtension/content mismatches (sampled):");
        for (key, counts) in &mismatches {
            println!(
                "  {key:<46} {:>4} sampled pages   e.g. {}",
                counts.pages,
                counts.examples.join(", ")
            );
        }
    }

    Ok(())
}

/// Every known format, used to translate a report label back into its facts.
fn all_facts() -> Vec<(&'static str, FormatFacts)> {
    [
        "jpg", "png", "gif", "webp", "bmp", "svg", "avif", "jxl", "heic", "jp2", "mng", "wp2",
        "tiff", "mpo",
    ]
    .into_iter()
    .filter_map(|ext| facts_for_extension(ext).map(|facts| (facts.label, facts)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_extensions_by_browser_and_decoder_support() {
        let jpeg = facts_for_extension("jpg").expect("jpeg known");
        assert!(jpeg.browser == Browser::Universal && jpeg.server_can_convert);

        let heic = facts_for_extension("heic").expect("heic known");
        assert!(heic.browser == Browser::Conditional && !heic.server_can_convert);
        assert_eq!(heic.label, "HEIC/HEIF");

        // Unviewable in every browser, but decoded here (pure-Rust openjp2), so a client that can
        // display WebP still gets it. The split that matters is not "viewable vs not" — it is
        // "viewable, or convertible into something viewable".
        let jp2 = facts_for_extension("jp2").expect("jpeg 2000 known");
        assert!(jp2.browser == Browser::Never && jp2.server_can_convert);

        // AVIF is client-dependent *and* convertible here: the browser split is stills versus
        // sequences (Firefox 113+/Safari 16.4+ for the latter), and stills are decoded/relayed.
        let avif = facts_for_extension("avif").expect("avif known");
        assert!(avif.browser == Browser::Conditional && avif.server_can_convert);

        // The case that is genuinely unviewable for *every* client and unconvertible here.
        let mng = facts_for_extension("mng").expect("mng known");
        assert!(mng.browser == Browser::Never && !mng.server_can_convert);

        assert!(facts_for_extension("xyz").is_none());
    }

    #[test]
    fn identifies_formats_by_magic_bytes_when_the_extension_lies() {
        // A renamed HEIC inside a zip is the case this exists for.
        let heic = b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00heicmif1";
        assert_eq!(facts_for_magic(heic).map(|f| f.label), Some("HEIC/HEIF"));
        // Animated AVIF (`avis`) and JXL container/codestream signatures.
        assert_eq!(
            facts_for_magic(b"\x00\x00\x00\x1cftypavis\x00\x00\x00\x00avifmsf1").map(|f| f.label),
            Some("AVIF (still + sequences)")
        );
        assert_eq!(
            facts_for_magic(b"\x00\x00\x00\x0cJXL \r\n\x87\n").map(|f| f.label),
            Some("JPEG XL (still + sequences)")
        );
        assert_eq!(
            facts_for_magic(b"\xff\x0a\x00\x00").map(|f| f.label),
            Some("JPEG XL (still + sequences)")
        );
        assert_eq!(
            facts_for_magic(b"\xff\xd8\xff\xe0").map(|f| f.label),
            Some("JPEG")
        );
        assert!(facts_for_magic(b"not an image").is_none());
    }
}
