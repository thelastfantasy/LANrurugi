//! Length-prefixed postcard frames: a 4-byte big-endian length, then the encoded message. The
//! async reading and writing lives with the transport; this module only encodes and bounds.

use serde::de::DeserializeOwned;
use serde::Serialize;

/// Large enough for a manifest of thousands of archives, small enough to refuse garbage early.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame of {0} bytes exceeds the limit")]
    TooLarge(usize),
    #[error("malformed frame: {0}")]
    Malformed(#[from] postcard::Error),
}

/// The length prefix followed by the encoded message.
pub fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>, FrameError> {
    let body = postcard::to_allocvec(message)?;
    if body.len() > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(body.len()));
    }
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// The body length announced by a prefix, refused when it exceeds `MAX_FRAME_LEN`.
pub fn body_len(prefix: [u8; 4]) -> Result<usize, FrameError> {
    let len = u32::from_be_bytes(prefix) as usize;
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(len));
    }
    Ok(len)
}

pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, FrameError> {
    Ok(postcard::from_bytes(body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{Envelope, Request};

    #[test]
    fn a_frame_round_trips() {
        let message = Envelope::new(Request::Renew {
            claim_id: "c".into(),
        });
        let frame = encode(&message).unwrap();
        let len = body_len(frame[..4].try_into().unwrap()).unwrap();
        assert_eq!(len, frame.len() - 4);
        assert_eq!(decode::<Envelope>(&frame[4..]).unwrap(), message);
    }

    #[test]
    fn oversized_prefixes_are_refused() {
        assert!(matches!(
            body_len((MAX_FRAME_LEN as u32 + 1).to_be_bytes()),
            Err(FrameError::TooLarge(_))
        ));
    }
}
