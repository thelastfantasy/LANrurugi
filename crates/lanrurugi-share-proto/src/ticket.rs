//! Transfer tickets signed by the sharer (research §6). A ticket names one importer and the
//! archives it may fetch, and lives 25 seconds, so revocation reaches helpers within 30 s.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::messages::{Blake3Hash, EndpointKey};

pub const TICKET_TTL_SECS: u64 = 25;
pub const RENEW_INTERVAL_SECS: u64 = 10;

const SIGNING_DOMAIN: &[u8] = b"lrrg-ticket-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    pub share_id: String,
    pub claim_id: String,
    pub importer: EndpointKey,
    pub roots: Vec<Blake3Hash>,
    /// Unix seconds.
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedTicket {
    pub ticket: Ticket,
    /// 64-byte ed25519 signature over the domain-separated encoding of `ticket`.
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TicketError {
    #[error("ticket signature does not verify")]
    BadSignature,
    #[error("ticket expired")]
    Expired,
    #[error("ticket was issued to another instance")]
    WrongImporter,
    #[error("ticket does not cover this archive")]
    NotCovered,
}

impl Ticket {
    fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = SIGNING_DOMAIN.to_vec();
        bytes.extend(postcard::to_allocvec(self).expect("a ticket always encodes"));
        bytes
    }

    pub fn sign(self, key: &SigningKey) -> SignedTicket {
        let signature = key.sign(&self.signing_bytes()).to_bytes().to_vec();
        SignedTicket {
            ticket: self,
            signature,
        }
    }
}

impl SignedTicket {
    /// Checks the sharer's signature and expiry.
    pub fn verify(&self, sharer: &EndpointKey, now: u64) -> Result<&Ticket, TicketError> {
        let key = VerifyingKey::from_bytes(sharer).map_err(|_| TicketError::BadSignature)?;
        let signature =
            Signature::from_slice(&self.signature).map_err(|_| TicketError::BadSignature)?;
        key.verify(&self.ticket.signing_bytes(), &signature)
            .map_err(|_| TicketError::BadSignature)?;
        if now >= self.ticket.expires_at {
            return Err(TicketError::Expired);
        }
        Ok(&self.ticket)
    }

    /// Everything a server checks before serving `blake3` to `importer`.
    pub fn verify_for(
        &self,
        sharer: &EndpointKey,
        importer: &EndpointKey,
        blake3: &Blake3Hash,
        now: u64,
    ) -> Result<&Ticket, TicketError> {
        let ticket = self.verify(sharer, now)?;
        if &ticket.importer != importer {
            return Err(TicketError::WrongImporter);
        }
        if !ticket.roots.contains(blake3) {
            return Err(TicketError::NotCovered);
        }
        Ok(ticket)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn ticket(expires_at: u64) -> Ticket {
        Ticket {
            share_id: "s".into(),
            claim_id: "c".into(),
            importer: key(2).verifying_key().to_bytes(),
            roots: vec![[9; 32]],
            expires_at,
        }
    }

    #[test]
    fn a_valid_ticket_verifies_for_its_importer_and_root() {
        let sharer = key(1);
        let signed = ticket(100).sign(&sharer);
        let sharer_pub = sharer.verifying_key().to_bytes();
        let importer = key(2).verifying_key().to_bytes();
        assert!(signed
            .verify_for(&sharer_pub, &importer, &[9; 32], 99)
            .is_ok());
    }

    #[test]
    fn expiry_wrong_importer_wrong_root_and_tampering_are_refused() {
        let sharer = key(1);
        let sharer_pub = sharer.verifying_key().to_bytes();
        let importer = key(2).verifying_key().to_bytes();
        let signed = ticket(100).sign(&sharer);

        assert_eq!(
            signed.verify_for(&sharer_pub, &importer, &[9; 32], 100),
            Err(TicketError::Expired)
        );
        let other = key(3).verifying_key().to_bytes();
        assert_eq!(
            signed.verify_for(&sharer_pub, &other, &[9; 32], 1),
            Err(TicketError::WrongImporter)
        );
        assert_eq!(
            signed.verify_for(&sharer_pub, &importer, &[8; 32], 1),
            Err(TicketError::NotCovered)
        );

        let mut tampered = signed.clone();
        tampered.ticket.roots.push([8; 32]);
        assert_eq!(
            tampered.verify(&sharer_pub, 1),
            Err(TicketError::BadSignature)
        );
        assert_eq!(signed.verify(&other, 1), Err(TicketError::BadSignature));
    }
}
