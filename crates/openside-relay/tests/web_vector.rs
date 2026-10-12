//! Fixed Noise handshake vector shared with the web client (`web/src/api/relay/noise.test.ts`),
//! so the WebCrypto initiator and the `snow` responder are checked against the same bytes.
//! Regenerate with `OPENSIDE_WRITE_VECTOR=1 cargo test -p openside-relay --test web_vector`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use openside_relay::noise::{self, StaticKeypair, PATTERN, PROLOGUE};

const VECTOR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../web/src/api/relay/noise-vector.json"
);

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn web_client_vector_matches_snow() {
    let daemon = StaticKeypair::from_secret([0x11; 32]);
    let initiator_ephemeral = StaticKeypair::from_secret([0x22; 32]);
    let responder_ephemeral = [0x33; 32];
    let daemon_id = noise::daemon_id(&daemon.public);
    let mut prologue = PROLOGUE.to_vec();
    prologue.extend_from_slice(daemon_id.as_bytes());

    let mut initiator = snow::Builder::new(PATTERN.parse().unwrap())
        .prologue(&prologue)
        .unwrap()
        .remote_public_key(&daemon.public)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&initiator_ephemeral.secret)
        .build_initiator()
        .unwrap();
    let mut responder = snow::Builder::new(PATTERN.parse().unwrap())
        .prologue(&prologue)
        .unwrap()
        .local_private_key(&daemon.secret)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&responder_ephemeral)
        .build_responder()
        .unwrap();
    let mut buf = vec![0_u8; 1024];
    let mut scratch = vec![0_u8; 1024];
    let n = initiator.write_message(&[], &mut buf).unwrap();
    let message1 = buf[..n].to_vec();
    responder.read_message(&message1, &mut scratch).unwrap();
    let n = responder.write_message(&[], &mut buf).unwrap();
    let message2 = buf[..n].to_vec();
    initiator.read_message(&message2, &mut scratch).unwrap();
    let mut initiator = initiator.into_transport_mode().unwrap();
    let mut responder = responder.into_transport_mode().unwrap();
    // Two messages each way, so the counter nonce is exercised past zero.
    let mut from_daemon = Vec::new();
    for text in ["hello from the daemon", "second"] {
        let n = responder.write_message(text.as_bytes(), &mut buf).unwrap();
        from_daemon.push(hex(&buf[..n]));
    }
    let mut from_device = Vec::new();
    for text in ["ping", "pong"] {
        let n = initiator.write_message(text.as_bytes(), &mut buf).unwrap();
        from_device.push(hex(&buf[..n]));
    }

    let vector = serde_json::json!({
        "daemon_id": daemon_id,
        "daemon_key": URL_SAFE_NO_PAD.encode(daemon.public),
        "ephemeral_secret": URL_SAFE_NO_PAD.encode(initiator_ephemeral.secret),
        "ephemeral_public": URL_SAFE_NO_PAD.encode(initiator_ephemeral.public),
        "message1": hex(&message1),
        "message2": hex(&message2),
        "from_daemon": from_daemon,
        "from_daemon_plain": ["hello from the daemon", "second"],
        "from_device": from_device,
        "from_device_plain": ["ping", "pong"],
    });
    let text = format!("{}\n", serde_json::to_string_pretty(&vector).unwrap());
    if std::env::var_os("OPENSIDE_WRITE_VECTOR").is_some() {
        std::fs::write(VECTOR, &text).unwrap();
    }
    let stored = std::fs::read_to_string(VECTOR)
        .expect("run with OPENSIDE_WRITE_VECTOR=1 to create the vector");
    assert_eq!(stored, text, "web/src/api/relay/noise-vector.json is stale");
}
