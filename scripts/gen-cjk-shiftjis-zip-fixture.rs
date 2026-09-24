// Hand-builds a single-entry zip with a raw Shift-JIS-encoded filename and the ZIP general-purpose
// UTF-8 flag (bit 11) deliberately left unset — reproducing exactly what an old Shift-JIS-locale
// zip tool actually wrote on disk, decades before the UTF-8 flag convention existed. No current
// tool (including `7z`) can produce this shape; every modern zip tool sets the UTF-8 flag for
// non-ASCII names. Mirrors the byte-construction helper already proven in
// `crates/lanrurugi-scanner/src/archive_format.rs`'s own
// `make_test_zip_with_raw_name_bytes`/`list_pages_recovers_shift_jis_filename_without_utf8_flag`
// test (specs/003-ui-test-automation research.md §5.2) — this script produces the same shape as a
// standalone, permanent fixture file instead of an in-test temp file.
//
// Run with: rustc --edition 2021 -O scripts/gen-cjk-shiftjis-zip-fixture.rs -o /tmp/gen-cjk-zip \
//   --extern encoding_rs=<path-to-libencoding_rs.rlib> && /tmp/gen-cjk-zip <output.zip> <content-file>
//
// Simpler in practice: run as a `cargo script`/scratch binary with `encoding_rs` as a dependency
// (see this project's own Cargo.lock for the exact version already used by lanrurugi-scanner).

use std::env;
use std::fs;
use std::io::Write;

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn build_zip(name_bytes: &[u8], content: &[u8]) -> Vec<u8> {
    let crc = crc32(content);
    let name_len = name_bytes.len() as u16;
    let content_len = content.len() as u32;

    let mut local = Vec::new();
    local.extend_from_slice(&0x04034b50u32.to_le_bytes());
    local.extend_from_slice(&20u16.to_le_bytes());
    local.extend_from_slice(&0u16.to_le_bytes()); // flags: no UTF-8 bit (0x0800) set
    local.extend_from_slice(&0u16.to_le_bytes()); // stored, no compression
    local.extend_from_slice(&0u16.to_le_bytes());
    local.extend_from_slice(&0u16.to_le_bytes());
    local.extend_from_slice(&crc.to_le_bytes());
    local.extend_from_slice(&content_len.to_le_bytes());
    local.extend_from_slice(&content_len.to_le_bytes());
    local.extend_from_slice(&name_len.to_le_bytes());
    local.extend_from_slice(&0u16.to_le_bytes());
    local.extend_from_slice(name_bytes);
    local.extend_from_slice(content);

    let mut central = Vec::new();
    central.extend_from_slice(&0x02014b50u32.to_le_bytes());
    central.extend_from_slice(&20u16.to_le_bytes());
    central.extend_from_slice(&20u16.to_le_bytes());
    central.extend_from_slice(&0u16.to_le_bytes());
    central.extend_from_slice(&0u16.to_le_bytes());
    central.extend_from_slice(&0u16.to_le_bytes());
    central.extend_from_slice(&0u16.to_le_bytes());
    central.extend_from_slice(&crc.to_le_bytes());
    central.extend_from_slice(&content_len.to_le_bytes());
    central.extend_from_slice(&content_len.to_le_bytes());
    central.extend_from_slice(&name_len.to_le_bytes());
    central.extend_from_slice(&0u16.to_le_bytes());
    central.extend_from_slice(&0u16.to_le_bytes());
    central.extend_from_slice(&0u16.to_le_bytes());
    central.extend_from_slice(&0u16.to_le_bytes());
    central.extend_from_slice(&0u32.to_le_bytes());
    central.extend_from_slice(&0u32.to_le_bytes()); // local header offset
    central.extend_from_slice(name_bytes);

    let mut eocd = Vec::new();
    eocd.extend_from_slice(&0x06054b50u32.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());
    eocd.extend_from_slice(&1u16.to_le_bytes());
    eocd.extend_from_slice(&1u16.to_le_bytes());
    eocd.extend_from_slice(&(central.len() as u32).to_le_bytes());
    eocd.extend_from_slice(&(local.len() as u32).to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());

    let mut out = Vec::new();
    out.extend_from_slice(&local);
    out.extend_from_slice(&central);
    out.extend_from_slice(&eocd);
    out
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let out_path = &args[1];
    let content = if args.len() > 2 {
        fs::read(&args[2]).expect("read content file")
    } else {
        b"placeholder page content".to_vec()
    };

    let (name_bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode("空白 テスト.jpg");
    assert!(!had_errors, "Shift-JIS encoding should not error for this test string");

    let zip_bytes = build_zip(&name_bytes, &content);
    fs::File::create(out_path)
        .and_then(|mut f| f.write_all(&zip_bytes))
        .expect("write output");
    eprintln!(
        "wrote {} bytes to {} (entry: Shift-JIS \"空白 テスト.jpg\", {} raw name bytes, {} content bytes)",
        zip_bytes.len(),
        out_path,
        name_bytes.len(),
        content.len()
    );
}
