//! Hex and clock helpers shared by node strings, rendezvous packets and share codes.

/// Lowercase hex, the spelling every wire string in this crate uses.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parses lowercase hex back into bytes. Odd length or a non-hex digit returns `None`.
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// Seconds since the Unix epoch; 0 if the clock is before it.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        assert_eq!(unhex(&hex(&[0, 15, 255])).unwrap(), vec![0, 15, 255]);
        assert!(unhex("abc").is_none());
    }
}
