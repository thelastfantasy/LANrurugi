//! The sharing protocol (specs/017-archive-sharing, contracts/share-protocol.md): share codes and
//! their `lrrg:` text form with its sealed route hint, the keys derived from a code, the encrypted
//! knock record, signed transfer tickets and the request/response messages exchanged between
//! instances.

pub mod code;
pub mod derive;
pub mod framing;
pub mod hint;
pub mod messages;
pub mod record;
pub mod ticket;
