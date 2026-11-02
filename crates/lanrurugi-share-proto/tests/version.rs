//! The engine version and the transport it speaks must be one number and one string, and the
//! transport must match the `iroh` the workspace actually builds against — a silent drift there
//! would let two instances believe they are compatible while their QUIC handshakes fail.

use lanrurugi_share_proto::messages::{SHARE_ENGINE_VERSION, SHARE_TRANSPORT};

#[test]
fn the_transport_string_matches_the_pinned_iroh_version() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
            .expect("the workspace manifest is readable from the source tree");
    let line = manifest
        .lines()
        .find(|l| l.trim_start().starts_with("iroh = { version ="))
        .expect("the workspace pins iroh");
    assert!(
        line.contains(&format!(
            "\"{}\"",
            SHARE_TRANSPORT.trim_start_matches("iroh ")
        )),
        "SHARE_TRANSPORT is {SHARE_TRANSPORT} but the workspace pins: {line}"
    );
}

#[test]
fn the_engine_version_is_one_positive_number() {
    const {
        assert!(SHARE_ENGINE_VERSION >= 1);
    }
}
