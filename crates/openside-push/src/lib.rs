//! Web Push sender for openside notifications.
//!
//! A message is encrypted for one subscription with the `aes128gcm` content coding of
//! RFC 8188 keyed as RFC 8291 describes (ECDH P-256 between a fresh application-server key
//! and the subscription's `p256dh` key, mixed with its 16-octet `auth` secret), and is
//! authorised towards the push service with an RFC 8292 VAPID JWT (ES256) signed by the
//! daemon's long-lived VAPID key. The push service never sees the plaintext.
//!
//! [`fake_push`] is a loopback push service for tests: it verifies the VAPID token and
//! decrypts each message with the subscriber's private key, exactly as a browser would.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod fake_push;

use std::{fmt, net::SocketAddr, path::Path, time::Duration};

use aes_gcm::{aead::Aead, Aes128Gcm, KeyInit, Nonce};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use hkdf::Hkdf;
use p256::{
    ecdh::diffie_hellman,
    ecdsa::{
        signature::{Signer, Verifier},
        Signature, SigningKey, VerifyingKey,
    },
    elliptic_curve::sec1::ToEncodedPoint,
    PublicKey, SecretKey,
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

/// Record size written into the content-coding header; one record carries the message.
const RECORD_SIZE: u32 = 4096;
/// Largest plaintext that fits one record: record size minus delimiter and GCM tag.
pub const MAX_PAYLOAD: usize = RECORD_SIZE as usize - 1 - 16;
/// VAPID tokens are valid for this long; RFC 8292 caps it at 24 hours.
const TOKEN_LIFETIME_SECS: u64 = 12 * 60 * 60;

/// Push failure; never carries key material.
#[derive(Debug)]
pub enum Error {
    /// A key, secret or header is malformed.
    Key(&'static str),
    /// Encryption, decryption or signature verification failed.
    Crypto(&'static str),
    /// Payload larger than [`MAX_PAYLOAD`].
    TooLarge,
    /// Key file I/O failed.
    Io(std::io::Error),
    /// Transport failure talking to the push service.
    Http(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(what) => write!(f, "invalid {what}"),
            Self::Crypto(what) => write!(f, "{what}"),
            Self::TooLarge => write!(f, "push payload exceeds {MAX_PAYLOAD} bytes"),
            Self::Io(error) => write!(f, "push key file: {error}"),
            Self::Http(error) => write!(f, "push service request failed: {error}"),
        }
    }
}

impl std::error::Error for Error {}

/// Result of push operations.
pub type Result<T, E = Error> = std::result::Result<T, E>;

fn decode(value: &str, what: &'static str) -> Result<Vec<u8>> {
    B64.decode(value.trim_end_matches('='))
        .map_err(|_| Error::Key(what))
}

fn uncompressed(key: &PublicKey) -> Vec<u8> {
    key.to_encoded_point(false).as_bytes().to_vec()
}

/// The daemon's VAPID signing key (application-server identity, RFC 8292).
pub struct VapidKey {
    secret: SecretKey,
}

impl VapidKey {
    /// Fresh random key.
    pub fn generate() -> Self {
        Self {
            secret: SecretKey::random(&mut OsRng),
        }
    }

    /// Load the key stored at `path` (base64url of the 32-byte scalar), creating it with
    /// mode 0600 on first use. An existing file is never rewritten.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                #[cfg(unix)]
                let private = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o077 == 0
                };
                #[cfg(not(unix))]
                let private = true;
                // A symlink or group/world-readable key may be shared with someone else.
                if !metadata.is_file() || !private {
                    return Err(Error::Key(
                        "VAPID key file must be a private (0600) regular file",
                    ));
                }
                let text = Zeroizing::new(std::fs::read_to_string(path).map_err(Error::Io)?);
                let bytes = Zeroizing::new(decode(text.trim(), "VAPID key file")?);
                let secret =
                    SecretKey::from_slice(&bytes).map_err(|_| Error::Key("VAPID key file"))?;
                Ok(Self { secret })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let key = Self::generate();
                let encoded = Zeroizing::new(format!("{}\n", B64.encode(key.secret.to_bytes())));
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(Error::Io)?;
                }
                match options.open(path) {
                    Ok(mut file) => {
                        use std::io::Write;
                        file.write_all(encoded.as_bytes()).map_err(Error::Io)?;
                        file.sync_all().map_err(Error::Io)?;
                        Ok(key)
                    }
                    // Another caller created it first; use theirs so both agree.
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        Self::load_or_create(path)
                    }
                    Err(error) => Err(Error::Io(error)),
                }
            }
            Err(error) => Err(Error::Io(error)),
        }
    }

    /// Uncompressed public key, base64url: the PWA's `applicationServerKey` and the `k=`
    /// parameter of every request.
    pub fn public_key(&self) -> String {
        B64.encode(uncompressed(&self.secret.public_key()))
    }

    /// `Authorization` header value for a request to `endpoint` (RFC 8292 §3).
    pub fn authorization(&self, endpoint: &str, subject: &str, now_unix: u64) -> Result<String> {
        let claims = VapidClaims {
            aud: audience(endpoint)?,
            exp: now_unix + TOKEN_LIFETIME_SECS,
            sub: subject.to_owned(),
        };
        let header = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let body = B64.encode(serde_json::to_vec(&claims).map_err(|_| Error::Key("claims"))?);
        let input = format!("{header}.{body}");
        let signature: Signature = SigningKey::from(&self.secret).sign(input.as_bytes());
        Ok(format!(
            "vapid t={input}.{}, k={}",
            B64.encode(signature.to_bytes()),
            self.public_key()
        ))
    }
}

/// Origin of a push endpoint, the JWT audience.
fn audience(endpoint: &str) -> Result<String> {
    let url = url::Url::parse(endpoint).map_err(|_| Error::Key("push endpoint"))?;
    Ok(url.origin().ascii_serialization())
}

/// VAPID JWT claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VapidClaims {
    /// Origin of the push service.
    pub aud: String,
    /// Expiry, seconds since the Unix epoch.
    pub exp: u64,
    /// Contact URI of the application server operator.
    pub sub: String,
}

/// Verify a `vapid t=…, k=…` header: the ES256 signature must match `k`. Returns `k`
/// (base64url public key) and the claims; callers check `aud` and `exp`.
pub fn verify_vapid(header: &str) -> Result<(String, VapidClaims)> {
    let rest = header
        .strip_prefix("vapid ")
        .ok_or(Error::Key("authorization scheme"))?;
    let mut token = None;
    let mut key = None;
    for part in rest.split(',') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("t=") {
            token = Some(value);
        } else if let Some(value) = part.strip_prefix("k=") {
            key = Some(value);
        }
    }
    let (token, key) = token.zip(key).ok_or(Error::Key("vapid parameters"))?;
    let (input, signature) = token.rsplit_once('.').ok_or(Error::Key("JWT"))?;
    let (header, claims) = input.split_once('.').ok_or(Error::Key("JWT"))?;
    let header: serde_json::Value = serde_json::from_slice(&decode(header, "JWT header")?)
        .map_err(|_| Error::Key("JWT header"))?;
    if header["alg"] != "ES256" {
        return Err(Error::Key("JWT algorithm"));
    }
    let verifying = VerifyingKey::from_sec1_bytes(&decode(key, "VAPID public key")?)
        .map_err(|_| Error::Key("VAPID public key"))?;
    let signature = Signature::from_slice(&decode(signature, "JWT signature")?)
        .map_err(|_| Error::Key("JWT signature"))?;
    verifying
        .verify(input.as_bytes(), &signature)
        .map_err(|_| Error::Crypto("VAPID signature does not verify"))?;
    let claims = serde_json::from_slice(&decode(claims, "JWT claims")?)
        .map_err(|_| Error::Key("JWT claims"))?;
    Ok((key.to_owned(), claims))
}

/// Check subscription keys as the browser reported them (base64url).
pub fn validate_keys(p256dh: &str, auth: &str) -> Result<()> {
    PublicKey::from_sec1_bytes(&decode(p256dh, "p256dh key")?)
        .map_err(|_| Error::Key("p256dh key"))?;
    if decode(auth, "auth secret")?.len() != 16 {
        return Err(Error::Key("auth secret"));
    }
    Ok(())
}

struct Derived {
    cek: Zeroizing<[u8; 16]>,
    nonce: [u8; 12],
}

/// RFC 8291 §3.3–3.4 key schedule.
fn derive(
    ecdh: &[u8],
    auth: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8],
) -> Result<Derived> {
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);
    let mut ikm = Zeroizing::new([0_u8; 32]);
    Hkdf::<Sha256>::new(Some(auth), ecdh)
        .expand(&key_info, ikm.as_mut())
        .map_err(|_| Error::Crypto("key derivation failed"))?;
    let prk = Hkdf::<Sha256>::new(Some(salt), ikm.as_ref());
    let mut cek = Zeroizing::new([0_u8; 16]);
    let mut nonce = [0_u8; 12];
    prk.expand(b"Content-Encoding: aes128gcm\0", cek.as_mut())
        .map_err(|_| Error::Crypto("key derivation failed"))?;
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .map_err(|_| Error::Crypto("key derivation failed"))?;
    Ok(Derived { cek, nonce })
}

/// Encrypt `plaintext` for a subscription (`p256dh`, `auth` as base64url) into an
/// `aes128gcm` body: salt ‖ rs ‖ idlen ‖ keyid (application-server public key) ‖ record.
pub fn encrypt(p256dh: &str, auth: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
    let mut salt = [0_u8; 16];
    OsRng.fill_bytes(&mut salt);
    encrypt_with(
        &SecretKey::random(&mut OsRng),
        salt,
        p256dh,
        auth,
        plaintext,
    )
}

fn encrypt_with(
    as_secret: &SecretKey,
    salt: [u8; 16],
    p256dh: &str,
    auth: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    if plaintext.len() > MAX_PAYLOAD {
        return Err(Error::TooLarge);
    }
    let ua_public_bytes = decode(p256dh, "p256dh key")?;
    let ua_public =
        PublicKey::from_sec1_bytes(&ua_public_bytes).map_err(|_| Error::Key("p256dh key"))?;
    let auth = Zeroizing::new(decode(auth, "auth secret")?);
    if auth.len() != 16 {
        return Err(Error::Key("auth secret"));
    }
    let as_public = uncompressed(&as_secret.public_key());
    let shared = diffie_hellman(as_secret.to_nonzero_scalar(), ua_public.as_affine());
    let keys = derive(
        shared.raw_secret_bytes(),
        &auth,
        &uncompressed(&ua_public),
        &as_public,
        &salt,
    )?;
    let mut record = Zeroizing::new(Vec::with_capacity(plaintext.len() + 1));
    record.extend_from_slice(plaintext);
    // Padding delimiter of the last (only) record, RFC 8188 §2.
    record.push(2);
    let ciphertext = Aes128Gcm::new_from_slice(keys.cek.as_ref())
        .map_err(|_| Error::Crypto("cipher setup failed"))?
        .encrypt(Nonce::from_slice(&keys.nonce), record.as_slice())
        .map_err(|_| Error::Crypto("encryption failed"))?;
    let mut body = Vec::with_capacity(16 + 4 + 1 + as_public.len() + ciphertext.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(as_public.len() as u8);
    body.extend_from_slice(&as_public);
    body.extend_from_slice(&ciphertext);
    Ok(body)
}

/// Decrypt an `aes128gcm` push body as the user agent does, with its subscription
/// private key and auth secret.
pub fn decrypt(ua_secret: &SecretKey, auth: &[u8], body: &[u8]) -> Result<Vec<u8>> {
    if body.len() < 21 {
        return Err(Error::Key("content-coding header"));
    }
    let salt = &body[..16];
    let id_len = usize::from(body[20]);
    let header_len = 21 + id_len;
    if body.len() < header_len + 17 {
        return Err(Error::Key("content-coding header"));
    }
    let as_public_bytes = &body[21..header_len];
    let as_public = PublicKey::from_sec1_bytes(as_public_bytes).map_err(|_| Error::Key("keyid"))?;
    let shared = diffie_hellman(ua_secret.to_nonzero_scalar(), as_public.as_affine());
    let keys = derive(
        shared.raw_secret_bytes(),
        auth,
        &uncompressed(&ua_secret.public_key()),
        as_public_bytes,
        salt,
    )?;
    let mut record = Zeroizing::new(
        Aes128Gcm::new_from_slice(keys.cek.as_ref())
            .map_err(|_| Error::Crypto("cipher setup failed"))?
            .decrypt(Nonce::from_slice(&keys.nonce), &body[header_len..])
            .map_err(|_| Error::Crypto("decryption failed"))?,
    );
    while record.last() == Some(&0) {
        record.pop();
    }
    if record.pop() != Some(2) {
        return Err(Error::Crypto("missing last-record delimiter"));
    }
    Ok(record.to_vec())
}

/// A subscription as stored by the daemon.
pub struct Subscription<'a> {
    /// Push service endpoint URL.
    pub endpoint: &'a str,
    /// Subscription public key, base64url.
    pub p256dh: &'a str,
    /// Subscription auth secret, base64url.
    pub auth: &'a str,
}

/// HTTP sender to push services: no proxies, no redirects, bounded time.
#[derive(Clone)]
pub struct Sender {
    client: reqwest::Client,
}

impl Sender {
    /// Build the HTTP client.
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Self::client(None)?,
        })
    }

    fn client(pinned: Option<(&str, SocketAddr)>) -> Result<reqwest::Client> {
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10));
        if let Some((host, address)) = pinned {
            builder = builder.resolve(host, address);
        }
        builder
            .build()
            .map_err(|error| Error::Http(error.to_string()))
    }

    /// Encrypt and post one message; returns the push service's HTTP status
    /// (201 accepted; 404/410 mean the subscription is gone). With `pinned`, the endpoint's
    /// host connects to exactly that address (the one the caller vetted), so a second DNS
    /// answer cannot redirect the request. Redirects are never followed.
    pub async fn send(
        &self,
        key: &VapidKey,
        subscription: &Subscription<'_>,
        payload: &[u8],
        ttl_secs: u32,
        subject: &str,
        pinned: Option<SocketAddr>,
    ) -> Result<u16> {
        let body = encrypt(subscription.p256dh, subscription.auth, payload)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let authorization = key.authorization(subscription.endpoint, subject, now)?;
        let pinned_client;
        let client = match pinned {
            Some(address) => {
                let url = url::Url::parse(subscription.endpoint)
                    .map_err(|_| Error::Http("invalid endpoint".into()))?;
                let host = url.host_str().unwrap_or_default().to_owned();
                pinned_client = Self::client(Some((&host, address)))?;
                &pinned_client
            }
            None => &self.client,
        };
        let response = client
            .post(subscription.endpoint)
            .header("TTL", ttl_secs.to_string())
            .header("Urgency", "high")
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .header("Authorization", authorization)
            .body(body)
            .send()
            .await
            .map_err(|error| Error::Http(error.without_url().to_string()))?;
        Ok(response.status().as_u16())
    }
}

#[cfg(test)]
mod tests;
