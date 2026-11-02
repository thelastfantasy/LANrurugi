// Hand-builds a minimal valid LHA/LZH level-1 archive containing one stored (uncompressed,
// method "-lh0-") file, for use as a `lanrurugi-scanner` test fixture (specs/003-ui-test-automation
// research.md §5.1). Neither `7z` (returns E_NOTIMPL for lzh) nor Debian's `lhasa` package (its
// `lha` command has no `a`/create subcommand — decompression only) can write this format, and no
// mature FOSS Rust crate does either, so the container bytes are constructed directly here — the
// same approach already used for the non-UTF-8-flagged zip fixture (research.md §5.2).
//
// Run with: rustc -O scripts/gen-lzh-fixture.rs -o /tmp/gen-lzh-fixture && \
//   /tmp/gen-lzh-fixture <output.lzh> <inner-filename> [content-file]
//
// Format reference: LHA/LZH level-1 header. Verified (not just derived from documentation) by
// round-tripping through `delharc` (an independent pure-Rust LHA reader) and through this
// project's own `lanrurugi_scanner::archive_format::list_pages` (the real libarchive read path).

use std::env;
use std::fs;
use std::io::Write;

fn crc16_arc(data: &[u8]) -> u16 {
    // LHA uses the CRC-16/ARC variant: reflected, polynomial 0xA001, no final XOR.
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
    }
    crc
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: gen-lzh-fixture <output.lzh> <inner-filename> [content-file]");
        std::process::exit(1);
    }
    let out_path = &args[1];
    let inner_name = &args[2];
    let content = if args.len() > 3 {
        fs::read(&args[3]).expect("read content file")
    } else {
        b"lanrurugi lzh fixture\n".to_vec()
    };

    let data_crc = crc16_arc(&content);
    let name_bytes = inner_name.as_bytes();

    // Level-1 header body (everything after the header-size and checksum bytes), in read order:
    //   "-lh0-" (5) | compressed_size u32LE (4, == original size for -lh0-) | original_size u32LE
    //   (4) | last_modified u32LE MS-DOS timestamp (4) | msdos_attrs u8 (1) | level u8 = 1 (1) |
    //   filename_len u8 (1) | filename (N) | data CRC-16 u16LE (2) | OS-ID u8 = 'U' (1) | next
    //   extended-header-size u16LE = 0 (2, terminates the extended-header chain)
    let mut header_body = Vec::new();
    header_body.extend_from_slice(b"-lh0-");
    header_body.extend_from_slice(&(content.len() as u32).to_le_bytes());
    header_body.extend_from_slice(&(content.len() as u32).to_le_bytes());
    header_body.extend_from_slice(&0x0000u16.to_le_bytes()); // time
    header_body.extend_from_slice(&0x0021u16.to_le_bytes()); // date (arbitrary valid MS-DOS date)
    header_body.push(0x20); // msdos_attrs: archive bit
    header_body.push(0x01); // level 1
    header_body.push(name_bytes.len() as u8);
    header_body.extend_from_slice(name_bytes);
    header_body.extend_from_slice(&data_crc.to_le_bytes());
    header_body.push(b'U'); // OS-ID: Unix-like/generic

    // The header-size byte does NOT count the trailing 2-byte "next extended header size" field
    // itself. But a real parser's running byte-length counter already includes the header-size and
    // checksum bytes' own 2 bytes by the time it reaches the check that uses this value (their own
    // read increments the counter, even though only the wrapping *checksum* — not the length count
    // — gets explicitly reset right after them). So header_size = 2 + len(header_body so far).
    // Verified against `delharc`'s actual parser via debug tracing, not re-derived from
    // documentation alone — a naive reading of the format spec gets this offset wrong.
    let header_size = (header_body.len() + 2) as u8;
    header_body.extend_from_slice(&0x0000u16.to_le_bytes()); // next ext. header size = 0 (end)

    let checksum: u8 = header_body.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));

    let mut out = Vec::new();
    out.push(header_size);
    out.push(checksum);
    out.extend_from_slice(&header_body);
    out.extend_from_slice(&content);

    fs::File::create(out_path)
        .and_then(|mut f| f.write_all(&out))
        .expect("write output");
    eprintln!(
        "wrote {} bytes to {} (inner file: {}, {} bytes, crc16={:#06x})",
        out.len(),
        out_path,
        inner_name,
        content.len(),
        data_crc
    );
}
