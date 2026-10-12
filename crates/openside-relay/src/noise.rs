//! Noise `NK` handshake between a device (initiator) and the daemon (responder).
//!
//! The device learns the daemon's static X25519 public key from the pairing QR, so the
//! relay in between cannot impersonate the daemon: without the matching private key the
//! handshake fails. The prologue binds the routing id, so a frame replayed to another
//! daemon id does not decrypt either.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::Error;

/// Noise protocol name; the browser client (`web/src/api/relay/noise.ts`) uses the same.
pub const PATTERN: &str = "Noise_NK_25519_AESGCM_SHA256";
/// Prologue prefix; the daemon id follows.
pub const PROLOGUE: &[u8] = b"openside-relay/1\n";
/// Largest Noise message (and so the largest relay WebSocket message).
pub const MAX_MESSAGE: usize = 65535;
/// AEAD tag appended to every transport message.
pub const TAG_LEN: usize = 16;

/// The daemon's long-term X25519 key pair.
#[derive(Clone)]
pub struct StaticKeypair {
    /// Private scalar; never leaves `<data_dir>/relay-key`.
    pub secret: [u8; 32],
    /// Public key carried in pairing QR codes.
    pub public: [u8; 32],
}

impl std::fmt::Debug for StaticKeypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticKeypair")
            .field("public", &encode_key(&self.public))
            .finish_non_exhaustive()
    }
}

impl Drop for StaticKeypair {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl StaticKeypair {
    /// A fresh random key pair.
    pub fn generate() -> Self {
        let mut secret = [0_u8; 32];
        rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut secret);
        Self::from_secret(secret)
    }

    /// Rebuild the pair from a stored private key.
    pub fn from_secret(secret: [u8; 32]) -> Self {
        let public = curve25519_dalek::MontgomeryPoint::mul_base_clamped(secret).to_bytes();
        Self { secret, public }
    }
}

/// Stable routing id derived from the public key. Registration separately proves possession
/// of the corresponding secret; the public hash alone is not an authorization credential.
pub fn daemon_id(public: &[u8; 32]) -> String {
    let digest = Sha256::digest(public);
    URL_SAFE_NO_PAD.encode(&digest[..16])
}

/// base64url without padding.
pub fn encode_key(key: &[u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(key)
}

/// Inverse of [`encode_key`]; `None` unless exactly 32 bytes.
pub fn decode_key(text: &str) -> Option<[u8; 32]> {
    URL_SAFE_NO_PAD.decode(text.trim()).ok()?.try_into().ok()
}

fn prologue(daemon_id: &str, registration: bool) -> Vec<u8> {
    let mut prologue = if registration {
        b"openside-relay-registration/1\n".to_vec()
    } else {
        PROLOGUE.to_vec()
    };
    prologue.extend_from_slice(daemon_id.as_bytes());
    prologue
}

fn params() -> Result<snow::params::NoiseParams, Error> {
    PATTERN.parse().map_err(|_| Error::Handshake)
}

/// Initiator side, step one: returns the state and the first message to send.
pub fn initiate(
    daemon_id: &str,
    daemon_key: &[u8; 32],
) -> Result<(snow::HandshakeState, Vec<u8>), Error> {
    initiate_with_prologue(daemon_id, daemon_key, false)
}

pub(crate) fn initiate_registration(
    daemon_id: &str,
    daemon_key: &[u8; 32],
) -> Result<(snow::HandshakeState, Vec<u8>), Error> {
    initiate_with_prologue(daemon_id, daemon_key, true)
}

fn initiate_with_prologue(
    daemon_id: &str,
    daemon_key: &[u8; 32],
    registration: bool,
) -> Result<(snow::HandshakeState, Vec<u8>), Error> {
    let prologue = prologue(daemon_id, registration);
    let mut state = snow::Builder::new(params()?)
        .prologue(&prologue)
        .and_then(|b| b.remote_public_key(daemon_key))
        .and_then(|b| b.build_initiator())
        .map_err(|_| Error::Handshake)?;
    let mut message = vec![0_u8; MAX_MESSAGE];
    let len = state
        .write_message(&[], &mut message)
        .map_err(|_| Error::Handshake)?;
    message.truncate(len);
    Ok((state, message))
}

/// Initiator side, step two: consume the responder's reply.
pub fn finish(
    mut state: snow::HandshakeState,
    reply: &[u8],
) -> Result<snow::TransportState, Error> {
    let mut payload = vec![0_u8; MAX_MESSAGE];
    state
        .read_message(reply, &mut payload)
        .map_err(|_| Error::Handshake)?;
    state.into_transport_mode().map_err(|_| Error::Handshake)
}

/// Responder side: consume the first message, return the reply and the transport state.
pub fn respond(
    keypair: &StaticKeypair,
    daemon_id: &str,
    first: &[u8],
) -> Result<(snow::TransportState, Vec<u8>), Error> {
    respond_with_prologue(keypair, daemon_id, first, false)
}

pub(crate) fn respond_registration(
    keypair: &StaticKeypair,
    daemon_id: &str,
    first: &[u8],
) -> Result<(snow::TransportState, Vec<u8>), Error> {
    respond_with_prologue(keypair, daemon_id, first, true)
}

fn respond_with_prologue(
    keypair: &StaticKeypair,
    daemon_id: &str,
    first: &[u8],
    registration: bool,
) -> Result<(snow::TransportState, Vec<u8>), Error> {
    let prologue = prologue(daemon_id, registration);
    let mut state = snow::Builder::new(params()?)
        .prologue(&prologue)
        .and_then(|b| b.local_private_key(&keypair.secret))
        .and_then(|b| b.build_responder())
        .map_err(|_| Error::Handshake)?;
    let mut payload = vec![0_u8; MAX_MESSAGE];
    state
        .read_message(first, &mut payload)
        .map_err(|_| Error::Handshake)?;
    let mut reply = vec![0_u8; MAX_MESSAGE];
    let len = state
        .write_message(&[], &mut reply)
        .map_err(|_| Error::Handshake)?;
    reply.truncate(len);
    Ok((
        state.into_transport_mode().map_err(|_| Error::Handshake)?,
        reply,
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn handshake_round_trips_and_binds_the_daemon_key() {
        let daemon = StaticKeypair::generate();
        let id = daemon_id(&daemon.public);
        assert_eq!(decode_key(&encode_key(&daemon.public)), Some(daemon.public));
        let (state, first) = initiate(&id, &daemon.public).unwrap();
        let (mut responder, reply) = respond(&daemon, &id, &first).unwrap();
        let mut initiator = finish(state, &reply).unwrap();
        let mut wire = [0_u8; 64];
        let len = initiator.write_message(b"hello", &mut wire).unwrap();
        assert!(!wire[..len].windows(5).any(|w| w == b"hello"));
        let mut plain = [0_u8; 64];
        let n = responder.read_message(&wire[..len], &mut plain).unwrap();
        assert_eq!(&plain[..n], b"hello");

        let impostor = StaticKeypair::generate();
        let (_, first) = initiate(&id, &daemon.public).unwrap();
        assert!(matches!(
            respond(&impostor, &id, &first),
            Err(Error::Handshake)
        ));
        let (_, first) = initiate("other-id", &daemon.public).unwrap();
        assert!(matches!(
            respond(&daemon, &id, &first),
            Err(Error::Handshake)
        ));
    }

    #[test]
    fn public_key_matches_snow_derivation() {
        let builder = snow::Builder::new(params().unwrap());
        let generated = builder.generate_keypair().unwrap();
        let secret: [u8; 32] = generated.private.as_slice().try_into().unwrap();
        assert_eq!(
            StaticKeypair::from_secret(secret).public.as_slice(),
            generated.public.as_slice()
        );
    }

    #[test]
    fn registration_proves_the_static_key_and_is_domain_separated() {
        let daemon = StaticKeypair::generate();
        let id = daemon_id(&daemon.public);
        let (state, first) = initiate_registration(&id, &daemon.public).unwrap();
        assert!(respond(&daemon, &id, &first).is_err());
        assert!(respond_registration(&StaticKeypair::generate(), &id, &first).is_err());
        let (_, reply) = respond_registration(&daemon, &id, &first).unwrap();
        assert!(finish(state, &reply).is_ok());
    }

    #[test]
    fn terminal_transport_nonce_is_rejected_without_wrapping() {
        let daemon = StaticKeypair::generate();
        let id = daemon_id(&daemon.public);
        let (state, first) = initiate(&id, &daemon.public).unwrap();
        let (_, reply) = respond(&daemon, &id, &first).unwrap();
        let mut transport = finish(state, &reply).unwrap();
        transport.set_receiving_nonce(u64::MAX);
        assert!(transport.read_message(&[0; TAG_LEN], &mut [0; 1]).is_err());
        assert_eq!(transport.receiving_nonce(), u64::MAX);
    }
}
