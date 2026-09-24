//! The client's side of pairing, as a state machine that cannot skip
//! mutual confirmation.
//!
//! ```text
//! ClientPairing ──begin_request──▶ (send)
//!               ──prove(begin)───▶ AwaitingServer + CompleteRequest
//! AwaitingServer ─finish(complete)─▶ Paired        only if proofS verifies
//!                                  ─▶ Untrusted    otherwise; nothing kept
//! ```
//!
//! `Paired` — the pin, the device id and the token — can be built only by
//! [`AwaitingServer::finish`], and only after `proofS` has verified in
//! constant time. A caller therefore cannot store a pin or a token from a
//! server that has not proven it holds the same secret, on the same TLS
//! connection, with the same key the client saw. The native desktop client
//! (M1G) drives this; M1E's tests drive it as a harness.

use zeroize::Zeroizing;

use crate::device::DeviceMetadata;
use crate::secret::PairingSecret;
use crate::token::DeviceToken;
use crate::transcript::{self, Profile, TranscriptInputs};
use crate::wire::{
    BeginRequest, BeginResponse, Bytes, CompleteRequest, CompleteResponse, Id, ProfileField,
};
use crate::TRANSCRIPT_BYTES;

/// A pairing attempt before `begin`.
pub struct ClientPairing {
    secret: PairingSecret,
    device: DeviceMetadata,
    server_id: [u8; 16],
    spki: [u8; 32],
    channel_binding: Zeroizing<[u8; 32]>,
    client_nonce: [u8; 32],
}

/// After `complete` was sent: holds what is needed to check `proofS`.
pub struct AwaitingServer {
    key: Zeroizing<[u8; 32]>,
    transcript: [u8; TRANSCRIPT_BYTES],
    server_id: [u8; 16],
    spki: [u8; 32],
}

/// A mutually confirmed pairing: the only value worth storing.
#[derive(Debug)]
pub struct Paired {
    /// The server's identifier.
    pub server_id: [u8; 16],
    /// The pin: `SHA-256(SPKI)` from this connection's handshake.
    pub spki: [u8; 32],
    /// This device's identifier.
    pub device_id: Id,
    /// This device's token.
    pub token: DeviceToken,
}

/// The server did not prove itself. Nothing from the exchange may be kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Untrusted;

impl ClientPairing {
    /// Starts an attempt.
    ///
    /// `server_id` is the identifier the client expects (from `pair/info`);
    /// `spki` and `channel_binding` come from **this connection's**
    /// handshake, never from a response body; `client_nonce` is 32 bytes from
    /// the OS CSPRNG.
    #[must_use]
    pub fn new(
        secret: PairingSecret,
        device: DeviceMetadata,
        server_id: [u8; 16],
        spki: [u8; 32],
        channel_binding: [u8; 32],
        client_nonce: [u8; 32],
    ) -> Self {
        Self {
            secret,
            device,
            server_id,
            spki,
            channel_binding: Zeroizing::new(channel_binding),
            client_nonce,
        }
    }

    /// The `begin` body. The profile is the only one the client computes
    /// under; it is not a choice offered to the server.
    #[must_use]
    pub fn begin_request(&self) -> BeginRequest {
        BeginRequest {
            device_name: self.device.name().clone(),
            platform: self.device.platform(),
            client_nonce: Bytes(self.client_nonce),
            binding_profile: ProfileField(Profile::NativeTlsExporterV1.id().to_owned()),
        }
    }

    /// Computes `proofC` for the server's answer to `begin`.
    #[must_use]
    pub fn prove(self, begin: &BeginResponse) -> (CompleteRequest, AwaitingServer) {
        let inputs = TranscriptInputs {
            profile_hash: Profile::NativeTlsExporterV1.hash(),
            server_id: self.server_id,
            spki: self.spki,
            channel_binding: *self.channel_binding,
            client_nonce: self.client_nonce,
            server_nonce: begin.server_nonce.0,
            device_hash: transcript::device_hash(&self.device),
        };
        let key = transcript::pairing_key(&self.secret, &begin.server_nonce.0, &self.server_id);
        let t = transcript::transcript(&inputs);
        let request = CompleteRequest {
            pairing_id: begin.pairing_id,
            proof_c: Bytes(transcript::client_proof(&key, &t)),
        };
        (
            request,
            AwaitingServer {
                key,
                transcript: t,
                server_id: self.server_id,
                spki: self.spki,
            },
        )
    }
}

impl AwaitingServer {
    /// Verifies `proofS` first, then the rest.
    ///
    /// # Errors
    ///
    /// [`Untrusted`] if the proof does not verify, or the server names a
    /// different identity, or the answer is malformed.
    pub fn finish(self, response: CompleteResponse) -> Result<Paired, Untrusted> {
        let expected = transcript::server_proof(&self.key, &self.transcript);
        if !transcript::proofs_match(&expected, &response.proof_s.0) {
            return Err(Untrusted);
        }
        if response.server.id.0 != self.server_id || response.role != "owner" {
            return Err(Untrusted);
        }
        let token = DeviceToken::parse(&response.device_token).ok_or(Untrusted)?;
        Ok(Paired {
            server_id: self.server_id,
            spki: self.spki,
            device_id: response.device_id,
            token,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{DeviceName, Platform};
    use crate::wire::ServerRef;

    const SERVER_ID: [u8; 16] = [0xaa; 16];

    fn attempt(spki: [u8; 32], cb: [u8; 32]) -> ClientPairing {
        ClientPairing::new(
            PairingSecret::from_bytes([5; 16]),
            DeviceMetadata::new(DeviceName::parse("Laptop").expect("name"), Platform::Linux),
            SERVER_ID,
            spki,
            cb,
            [3; 32],
        )
    }

    /// What an honest server holding the same secret computes.
    fn server_view(
        begin: &BeginRequest,
        server_nonce: [u8; 32],
        spki: [u8; 32],
        cb: [u8; 32],
    ) -> ([u8; 32], [u8; 32]) {
        let device = DeviceMetadata::new(begin.device_name.clone(), begin.platform);
        let inputs = TranscriptInputs {
            profile_hash: Profile::NativeTlsExporterV1.hash(),
            server_id: SERVER_ID,
            spki,
            channel_binding: cb,
            client_nonce: begin.client_nonce.0,
            server_nonce,
            device_hash: transcript::device_hash(&device),
        };
        let key = transcript::pairing_key(
            &PairingSecret::from_bytes([5; 16]),
            &server_nonce,
            &SERVER_ID,
        );
        let t = transcript::transcript(&inputs);
        (
            transcript::client_proof(&key, &t),
            transcript::server_proof(&key, &t),
        )
    }

    fn response(proof_s: [u8; 32], server: [u8; 16]) -> CompleteResponse {
        CompleteResponse {
            proof_s: Bytes(proof_s),
            device_id: Id([1; 16]),
            device_token: DeviceToken::from_bytes([9; 32]).encode().to_string(),
            role: "owner".to_owned(),
            server: ServerRef {
                id: Id(server),
                name: "atrium".to_owned(),
            },
        }
    }

    fn begin_response() -> BeginResponse {
        BeginResponse {
            pairing_id: Id([7; 16]),
            server_nonce: Bytes([4; 32]),
            expires_at: "2026-09-24T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn an_honest_server_is_confirmed() {
        let client = attempt([1; 32], [2; 32]);
        let begin = client.begin_request();
        let (expected_c, proof_s) = server_view(&begin, [4; 32], [1; 32], [2; 32]);
        let (complete, awaiting) = client.prove(&begin_response());
        assert_eq!(
            complete.proof_c.0, expected_c,
            "both sides compute the same proofC"
        );
        let paired = awaiting
            .finish(response(proof_s, SERVER_ID))
            .expect("confirmed");
        assert_eq!(paired.spki, [1; 32]);
        assert_eq!(paired.server_id, SERVER_ID);
    }

    #[test]
    fn a_wrong_proof_s_is_untrusted() {
        let client = attempt([1; 32], [2; 32]);
        let (_, awaiting) = client.prove(&begin_response());
        assert_eq!(
            awaiting.finish(response([0x5a; 32], SERVER_ID)).map(|_| ()),
            Err(Untrusted)
        );
    }

    #[test]
    fn a_server_claiming_another_identity_is_untrusted() {
        let client = attempt([1; 32], [2; 32]);
        let begin = client.begin_request();
        let (_, proof_s) = server_view(&begin, [4; 32], [1; 32], [2; 32]);
        let (_, awaiting) = client.prove(&begin_response());
        assert_eq!(
            awaiting.finish(response(proof_s, [0xbb; 16])).map(|_| ()),
            Err(Untrusted)
        );
    }

    #[test]
    fn a_relay_sees_different_proofs() {
        // The client pinned a MITM's key and exporter; the real server
        // computes over its own. Neither proof crosses.
        let client = attempt([0xee; 32], [0xdd; 32]);
        let begin = client.begin_request();
        let (real_c, real_s) = server_view(&begin, [4; 32], [1; 32], [2; 32]);
        let (complete, awaiting) = client.prove(&begin_response());
        assert_ne!(complete.proof_c.0, real_c);
        assert!(awaiting.finish(response(real_s, SERVER_ID)).is_err());
    }
}
