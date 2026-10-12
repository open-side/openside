//! Reach an openside daemon that has no inbound port: the daemon keeps an outbound
//! WebSocket to a relay, devices connect to the same relay, and the relay forwards opaque
//! frames between them.
//!
//! CLI/MCP clients and the daemon run a Noise `NK` handshake end to end ([`noise`]),
//! with the daemon's static key pinned from the pairing link. The relay cannot read or
//! forge those clients' traffic. A relay-served PWA uses the same encrypted transport
//! against passive observers, but trusts the operator to serve its code: modified
//! JavaScript can steal pairing codes, device tokens and plaintext. Prefer self-hosting
//! the relay, or loading and connecting the PWA directly to the daemon/Tailscale origin.
//! Inside the session, [`mux`] carries one byte stream per HTTP exchange (WebSocket
//! upgrades included), so the daemon's ordinary HTTP API, SSE and WebSockets work
//! unchanged. [`server`] is the relay, [`host`] the daemon side, [`client`] the device side
//! (the CLI; the web app has its own TypeScript client in `web/src/api/relay/`).

pub mod client;
pub mod host;
pub mod mux;
pub mod net;
pub mod noise;
pub mod server;

/// Relay, handshake and stream failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The daemon is not registered with the relay right now.
    #[error("the daemon is not connected to the relay")]
    DaemonOffline,
    /// The relay refused the registration token.
    #[error("the relay rejected the registration token")]
    Unauthorized,
    /// The relay is at its per-daemon connection limit.
    #[error("the relay has too many connections to this daemon")]
    TooManyClients,
    /// The Noise handshake failed: wrong daemon key, tampering or a timeout.
    #[error("could not verify the daemon (encrypted handshake failed)")]
    Handshake,
    /// The session or stream ended.
    #[error("the relay session closed")]
    Closed,
    /// The URL is not an `https://` origin (or `http://` on loopback).
    #[error("relay URL must be an https:// origin (http:// only on loopback)")]
    InvalidUrl,
    /// Network failure.
    #[error("{0}")]
    Io(String),
    /// The peer broke the protocol.
    #[error("relay protocol error: {0}")]
    Protocol(&'static str),
}
