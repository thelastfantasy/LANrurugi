//! Length-prefixed postcard frames: a 4-byte big-endian body length, then the sender's
//! [`ShareEngine`] behind its own 2-byte length, then the encoded message. Keeping the engine in the
//! prefix is what makes the version check independent of the message: a peer whose message schema is
//! unknown can still be told its version is unsupported. The async reading and writing lives with
//! the transport; this module only encodes, splits and bounds.

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::messages::ShareEngine;

/// Bytes of the engine-length prefix that follows the body length.
pub const ENGINE_LEN_BYTES: usize = 2;

/// Large enough for a manifest of thousands of archives, small enough to refuse garbage early.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame of {0} bytes exceeds the limit")]
    TooLarge(usize),
    #[error("malformed frame: {0}")]
    Malformed(#[from] postcard::Error),
}

/// The length prefix, the sender's engine and then the encoded message.
pub fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>, FrameError> {
    encode_from(&ShareEngine::current(), message)
}

/// `encode` with an explicit engine, for tests and for answering with the engine that was asked for.
pub fn encode_from<T: Serialize>(engine: &ShareEngine, message: &T) -> Result<Vec<u8>, FrameError> {
    let engine = postcard::to_allocvec(engine)?;
    let payload = postcard::to_allocvec(message)?;
    let body = ENGINE_LEN_BYTES + engine.len() + payload.len();
    if body > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(body));
    }
    let mut frame = Vec::with_capacity(4 + body);
    frame.extend_from_slice(&(body as u32).to_be_bytes());
    frame.extend_from_slice(&(engine.len() as u16).to_be_bytes());
    frame.extend_from_slice(&engine);
    frame.extend_from_slice(&payload);
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

/// Splits a body into the sender's engine bytes and the message bytes.
pub fn split(body: &[u8]) -> Result<(&[u8], &[u8]), FrameError> {
    let (head, rest) =
        body.split_first_chunk::<ENGINE_LEN_BYTES>()
            .ok_or(FrameError::Malformed(
                postcard::Error::DeserializeUnexpectedEnd,
            ))?;
    let engine_len = u16::from_be_bytes(*head) as usize;
    if engine_len > rest.len() {
        return Err(FrameError::Malformed(
            postcard::Error::DeserializeUnexpectedEnd,
        ));
    }
    Ok(rest.split_at(engine_len))
}

/// Decodes just the engine bytes `split` returned.
pub fn decode_engine(engine: &[u8]) -> Result<ShareEngine, FrameError> {
    Ok(postcard::from_bytes(engine)?)
}

/// Decodes just the message bytes `split` returned.
pub fn decode_payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T, FrameError> {
    Ok(postcard::from_bytes(payload)?)
}

/// The sender's engine and the message it carries.
pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<(ShareEngine, T), FrameError> {
    let (engine, payload) = split(body)?;
    Ok((decode_engine(engine)?, decode_payload(payload)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::Request;

    #[test]
    fn a_frame_round_trips_with_its_engine() {
        let message = Request::Renew {
            claim_id: "c".into(),
        };
        let frame = encode(&message).unwrap();
        let len = body_len(frame[..4].try_into().unwrap()).unwrap();
        assert_eq!(len, frame.len() - 4);
        let (engine, decoded) = decode::<Request>(&frame[4..]).unwrap();
        assert_eq!(engine, ShareEngine::current());
        assert_eq!(decoded, message);
    }

    #[test]
    fn the_engine_is_readable_before_the_payload_is_decoded() {
        // A payload this build cannot decode (an unknown request variant, encoded by hand) still
        // yields the sender's engine, which is what a version refusal needs.
        let older = ShareEngine {
            protocol: 1,
            transport: "iroh 0.0.1".into(),
        };
        let frame = encode_from(
            &older,
            &Request::Renew {
                claim_id: "x".into(),
            },
        )
        .unwrap();
        let (engine, _) = split(&frame[4..]).unwrap();
        assert_eq!(postcard::from_bytes::<ShareEngine>(engine).unwrap(), older);
    }

    #[test]
    fn a_truncated_body_is_malformed() {
        assert!(split(&[0u8]).is_err());
        assert!(split(&[0, 9, 1]).is_err());
    }

    #[test]
    fn oversized_prefixes_are_refused() {
        assert!(matches!(
            body_len((MAX_FRAME_LEN as u32 + 1).to_be_bytes()),
            Err(FrameError::TooLarge(_))
        ));
    }
}
