//! Share codes and their `lrrg:` text form (research §3, FR-008, FR-008a).
//!
//! A code is Crockford base32: 10 characters (50 bits) by default, or 8 (40 bits) for an opt-in
//! short code. Parsing forgives case, separators and the usual look-alikes, so a code read aloud
//! or copied from a screenshot still works; there is no check character, so a real typo surfaces
//! as a failed lookup.

use std::fmt;
use std::net::SocketAddr;

use crate::hint::RouteHint;

/// Crockford's alphabet: no I, L, O or U.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub const CODE_LEN: usize = 10;
pub const SHORT_CODE_LEN: usize = 8;

/// The URI scheme of the text form.
pub const SCHEME: &str = "lrrg";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodeError {
    #[error("a share code has {CODE_LEN} or {SHORT_CODE_LEN} characters")]
    WrongLength,
    #[error("a share code contains only letters and digits")]
    InvalidCharacter,
    #[error("no share code found in the input")]
    NotAShareReference,
}

/// A normalised share code (uppercase, no separators).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ShareCode(String);

impl ShareCode {
    /// A fresh random code: 50 bits, or 40 for a short code.
    pub fn generate(short: bool) -> Self {
        let len = if short { SHORT_CODE_LEN } else { CODE_LEN };
        let bits: u64 = rand::random();
        let code = (0..len)
            .map(|i| ALPHABET[((bits >> (5 * i)) & 31) as usize] as char)
            .collect();
        Self(code)
    }

    /// Normalises user input: case-insensitive, `O`→`0`, `I`/`L`→`1`, dashes and spaces ignored.
    pub fn parse(input: &str) -> Result<Self, CodeError> {
        let mut code = String::with_capacity(CODE_LEN);
        for c in input.chars() {
            if c == '-' || c.is_whitespace() {
                continue;
            }
            let c = match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            };
            if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
                return Err(CodeError::InvalidCharacter);
            }
            code.push(c);
        }
        match code.len() {
            CODE_LEN | SHORT_CODE_LEN => Ok(Self(code)),
            _ => Err(CodeError::WrongLength),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_short(&self) -> bool {
        self.0.len() == SHORT_CODE_LEN
    }

    /// `XXXXX-XXXXX`, or `XXXX-XXXX` for a short code.
    pub fn display(&self) -> String {
        let (a, b) = self.0.split_at(self.0.len() / 2);
        format!("{a}-{b}")
    }
}

impl fmt::Display for ShareCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

/// Never print a code in logs by accident.
impl fmt::Debug for ShareCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ShareCode(..)")
    }
}

/// Where the sharer can be dialled without any lookup: `<endpoint_id>@<addr>[,<addr>…]`, plus
/// the relays it is reachable through (`&relay=https://…[,…]`) for importers that cannot reach
/// any address directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Via {
    pub endpoint_id: String,
    pub addrs: Vec<String>,
    pub relays: Vec<String>,
}

impl Via {
    pub fn parse(value: &str) -> Option<Self> {
        let (endpoint_id, addrs) = value.split_once('@')?;
        let addrs: Vec<String> = addrs
            .split(',')
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_string)
            .collect();
        if endpoint_id.is_empty() || !endpoint_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        Some(Self {
            endpoint_id: endpoint_id.to_string(),
            addrs,
            relays: Vec::new(),
        })
    }

    /// The `relay` parameter's https URLs; anything else is dropped.
    pub fn set_relays(&mut self, param: &str) {
        self.relays = param
            .split(',')
            .map(str::trim)
            .filter(|r| r.starts_with("https://"))
            .map(str::to_string)
            .collect();
    }
}

impl fmt::Display for Via {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.endpoint_id, self.addrs.join(","))
    }
}

/// Anything a person may paste: the `lrrg:` text form, a share link or a bare code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareRef {
    pub code: ShareCode,
    pub via: Option<Via>,
    /// The sharer's LANrurugi nodes, `<endpoint_id>@<host:port>`: rendezvous to ask when the
    /// `via` routes are stale.
    pub nodes: Vec<String>,
}

impl ShareRef {
    pub fn new(code: ShareCode) -> Self {
        Self {
            code,
            via: None,
            nodes: Vec::new(),
        }
    }

    /// Accepts `lrrg:<code>[?via=…&relay=…&node=…]` (unknown parameters ignored), `…/s/<code>`,
    /// `…#<code>`, or a bare code.
    pub fn parse(input: &str) -> Result<Self, CodeError> {
        let input = input.trim();
        if let Some(rest) = strip_scheme(input) {
            let rest = rest.trim_start_matches("//");
            let (code_part, query) = rest.split_once('?').unwrap_or((rest, ""));
            let code = ShareCode::parse(code_part.trim_end_matches('/'))?;
            let param = |name: &str| {
                query
                    .split('&')
                    .filter_map(|pair| pair.split_once('='))
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| v)
            };
            // A sealed hint (`r`) is what codes carry; `via`/`relay`/`node` in the clear are
            // still read from codes handed out before it.
            if let Some(hint) = param("r").and_then(|r| RouteHint::open(&code, r)) {
                return Ok(Self::from_hint(code, hint));
            }
            let via = param("via")
                .and_then(Via::parse)
                .map(|mut via| {
                    via.set_relays(param("relay").unwrap_or(""));
                    via
                })
                .filter(|via| !via.addrs.is_empty() || !via.relays.is_empty());
            let nodes = param("node")
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .filter(|n| n.contains('@'))
                .map(str::to_string)
                .collect();
            return Ok(Self { code, via, nodes });
        }
        if input.contains("://") {
            return link_code(input).map(Self::new);
        }
        ShareCode::parse(input).map(Self::new).map_err(|e| match e {
            CodeError::InvalidCharacter if input.contains('/') => CodeError::NotAShareReference,
            e => e,
        })
    }

    /// `lrrg:XXXXX-XXXXX`, plus `?r=<sealed route hint>` when there are routes to carry. A fresh
    /// nonce each time, so the same routes never give the same text twice.
    pub fn text_form(&self) -> String {
        let code = self.code.display();
        match self.route_hint() {
            Some(hint) => format!("{SCHEME}:{code}?r={}", hint.seal(&self.code)),
            None => format!("{SCHEME}:{code}"),
        }
    }

    fn from_hint(code: ShareCode, hint: RouteHint) -> Self {
        let via = Via {
            endpoint_id: hex(&hint.endpoint_id),
            addrs: hint.addrs.iter().map(SocketAddr::to_string).collect(),
            relays: hint
                .relays
                .into_iter()
                .filter(|r| r.starts_with("https://"))
                .collect(),
        };
        let nodes = hint
            .nodes
            .iter()
            .map(|(id, host)| format!("{}@{host}", hex(id)))
            .collect();
        Self {
            code,
            via: Some(via),
            nodes,
        }
    }

    /// The routes as a hint; addresses that are not `ip:port` and malformed ids are dropped.
    fn route_hint(&self) -> Option<RouteHint> {
        let via = self.via.as_ref()?;
        let nodes = self
            .nodes
            .iter()
            .filter_map(|n| {
                let (id, host) = n.split_once('@')?;
                Some((unhex32(id)?, host.to_string()))
            })
            .collect();
        Some(RouteHint {
            endpoint_id: unhex32(&via.endpoint_id)?,
            addrs: via.addrs.iter().filter_map(|a| a.parse().ok()).collect(),
            relays: via.relays.clone(),
            nodes,
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn strip_scheme(input: &str) -> Option<&str> {
    let (scheme, rest) = input.split_once(':')?;
    scheme.eq_ignore_ascii_case(SCHEME).then_some(rest)
}

/// The code inside a share link: the segment after `/s/` (or `/embed/s/`), or a `#` fragment.
fn link_code(url: &str) -> Result<ShareCode, CodeError> {
    let (without_fragment, fragment) = url.split_once('#').unwrap_or((url, ""));
    if !fragment.is_empty() {
        if let Ok(code) = ShareCode::parse(fragment) {
            return Ok(code);
        }
    }
    let path = without_fragment
        .split_once('?')
        .map_or(without_fragment, |(p, _)| p);
    let mut segments = path.split('/').filter(|s| !s.is_empty()).rev();
    let last = segments.next().ok_or(CodeError::NotAShareReference)?;
    if segments.next() == Some("s") {
        return ShareCode::parse(last).map_err(|_| CodeError::NotAShareReference);
    }
    Err(CodeError::NotAShareReference)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_codes_have_the_right_length_and_round_trip() {
        for short in [false, true] {
            let code = ShareCode::generate(short);
            assert_eq!(code.is_short(), short);
            assert_eq!(ShareCode::parse(&code.display()).unwrap(), code);
            assert_eq!(
                ShareCode::parse(&code.as_str().to_lowercase()).unwrap(),
                code
            );
        }
    }

    #[test]
    fn look_alikes_case_and_separators_are_forgiven() {
        let code = ShareCode::parse("k7qm9-xrt2h").unwrap();
        assert_eq!(code.as_str(), "K7QM9XRT2H");
        assert_eq!(
            ShareCode::parse("O1IL2 3456 7").unwrap().as_str(),
            "0111234567"
        );
        assert_eq!(code.display(), "K7QM9-XRT2H");
        assert_eq!(ShareCode::parse("K7QM9XRT").unwrap().display(), "K7QM-9XRT");
    }

    #[test]
    fn bad_codes_are_rejected() {
        assert_eq!(ShareCode::parse("K7QM9-XRT2"), Err(CodeError::WrongLength));
        assert_eq!(
            ShareCode::parse("K7QM9-XRT2U"),
            Err(CodeError::InvalidCharacter)
        );
        assert_eq!(
            ShareCode::parse("K7QM9-XRT2!"),
            Err(CodeError::InvalidCharacter)
        );
    }

    #[test]
    fn every_input_form_yields_the_code() {
        let want = ShareCode::parse("K7QM9XRT2H").unwrap();
        for input in [
            "K7QM9-XRT2H",
            "  k7qm9 xrt2h ",
            "lrrg:K7QM9-XRT2H",
            "LRRG:k7qm9xrt2h",
            "lrrg://K7QM9-XRT2H/",
            "lrrg:K7QM9-XRT2H?future=1",
            "https://example.org/s/K7QM9-XRT2H",
            "https://example.org/s/K7QM9-XRT2H/?x=1",
            "http://192.168.1.5:3000/embed/s/k7qm9xrt2h",
            "https://example.org/#K7QM9-XRT2H",
        ] {
            assert_eq!(ShareRef::parse(input).unwrap().code, want, "{input}");
        }
    }

    #[test]
    fn unrelated_links_are_not_share_references() {
        for input in [
            "https://example.org/",
            "https://example.org/reader/K7QM9XRT2H",
            "https://example.org/s/",
            "not/a/code",
        ] {
            assert!(ShareRef::parse(input).is_err(), "{input}");
        }
    }

    const ID: &str = "0707070707070707070707070707070707070707070707070707070707070707";

    #[test]
    fn plain_route_params_from_older_codes_are_still_read() {
        let parsed = ShareRef::parse("lrrg:K7QM9-XRT2H?via=abc123@203.0.113.7:443").unwrap();
        assert_eq!(
            parsed.via,
            Some(Via {
                endpoint_id: "abc123".into(),
                addrs: vec!["203.0.113.7:443".into()],
                relays: Vec::new(),
            })
        );
        let v6 = ShareRef::parse("lrrg:K7QM9XRT2H?via=abc@[2001:db8::1]:443").unwrap();
        assert_eq!(v6.via.unwrap().addrs, ["[2001:db8::1]:443"]);
        let text = "lrrg:K7QM9-XRT2H?via=abc@203.0.113.7:443&relay=https://relay.example.org/";
        let parsed = ShareRef::parse(text).unwrap();
        assert_eq!(
            parsed.via.as_ref().unwrap().relays,
            ["https://relay.example.org/"]
        );
        let plain = ShareRef::parse("lrrg:K7QM9XRT2H?via=abc@1.2.3.4:5&relay=http://x/").unwrap();
        assert!(plain.via.unwrap().relays.is_empty());
        let noded =
            "lrrg:K7QM9-XRT2H?via=abc@&relay=https://r.example/&node=def@node.example.org:443";
        let parsed = ShareRef::parse(noded).unwrap();
        assert!(parsed.via.as_ref().unwrap().addrs.is_empty());
        assert_eq!(parsed.nodes, ["def@node.example.org:443"]);
        let bare = ShareRef::parse("lrrg:K7QM9XRT2H?via=abc@&relay=http://x/").unwrap();
        assert_eq!(bare.via, None);
    }

    #[test]
    fn routes_travel_sealed_and_round_trip() {
        let mut reference = ShareRef::new(ShareCode::parse("K7QM9XRT2H").unwrap());
        reference.via = Some(Via {
            endpoint_id: ID.into(),
            addrs: vec!["[2001:db8::1]:443".into(), "203.0.113.7:443".into()],
            relays: vec!["https://relay.example.org/".into()],
        });
        reference.nodes = vec![format!("{ID}@node.example.org:443")];
        let text = reference.text_form();
        assert!(text.starts_with("lrrg:K7QM9-XRT2H?r="), "{text}");
        assert!(!text.contains("203.0.113.7") && !text.contains("relay.example"));
        assert_eq!(ShareRef::parse(&text).unwrap(), reference);
        assert_ne!(reference.text_form(), text, "a fresh nonce each time");
        // Another code cannot open it: the hint is ignored, the code still parses.
        let swapped = text.replacen("K7QM9-XRT2H", "K7QM9-XRT2J", 1);
        assert_eq!(ShareRef::parse(&swapped).unwrap().via, None);
    }

    #[test]
    fn a_malformed_via_is_ignored_not_fatal() {
        let parsed = ShareRef::parse("lrrg:K7QM9XRT2H?via=nonsense").unwrap();
        assert_eq!(parsed.via, None);
        assert_eq!(parsed.text_form(), "lrrg:K7QM9-XRT2H");
    }

    #[test]
    fn debug_output_never_shows_the_code() {
        let code = ShareCode::parse("K7QM9XRT2H").unwrap();
        assert!(!format!("{code:?}").contains("K7QM"));
    }
}
