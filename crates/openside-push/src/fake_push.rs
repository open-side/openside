//! Loopback push service for tests. It accepts `POST /push/{id}`, verifies the RFC 8292
//! VAPID token (signature, audience, expiry) and decrypts the RFC 8291 body with the
//! subscriber's private key, recording what a browser would have received. It never
//! talks to a real push service.

use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use p256::{elliptic_curve::sec1::ToEncodedPoint, SecretKey};
use parking_lot::Mutex;
use rand_core::{OsRng, RngCore};
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::VapidClaims;

/// One message the fake service accepted and decrypted.
#[derive(Debug, Clone)]
pub struct Delivery {
    /// Subscriber id from the endpoint path.
    pub subscriber: String,
    /// `k=` key of the VAPID header (base64url).
    pub vapid_key: String,
    /// Verified JWT claims.
    pub claims: VapidClaims,
    /// `TTL` header.
    pub ttl: u32,
    /// Decrypted payload.
    pub plaintext: Vec<u8>,
}

impl Delivery {
    /// Payload parsed as JSON (`null` when it is not JSON).
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.plaintext).unwrap_or(Value::Null)
    }
}

struct Subscriber {
    secret: SecretKey,
    auth: [u8; 16],
    gone: bool,
}

#[derive(Default)]
struct Inner {
    subscribers: HashMap<String, Subscriber>,
    deliveries: Vec<Delivery>,
    rejections: Vec<String>,
}

struct Shared {
    origin: String,
    inner: Mutex<Inner>,
    notify: Notify,
}

/// A running fake push service bound to 127.0.0.1.
pub struct FakePushService {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

/// Browser-side subscription material for one fake subscriber.
#[derive(Debug, Clone)]
pub struct FakeSubscription {
    /// Subscriber id.
    pub id: String,
    /// Endpoint URL on the fake service.
    pub endpoint: String,
    /// Public key (base64url, uncompressed P-256).
    pub p256dh: String,
    /// Auth secret (base64url).
    pub auth: String,
}

impl FakeSubscription {
    /// `PushSubscription.toJSON()` shape.
    pub fn to_json(&self) -> Value {
        json!({"endpoint": self.endpoint, "keys": {"p256dh": self.p256dh, "auth": self.auth}})
    }
}

impl FakePushService {
    /// Bind an ephemeral loopback port and serve.
    pub async fn spawn() -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            origin: format!("http://{addr}"),
            inner: Mutex::default(),
            notify: Notify::new(),
        });
        let app = Router::new()
            .route("/push/{id}", post(receive))
            .with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self { addr, shared, task })
    }

    /// Bound address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Create a subscriber with fresh keys, as `pushManager.subscribe()` would.
    pub fn subscribe(&self) -> FakeSubscription {
        let secret = SecretKey::random(&mut OsRng);
        let mut auth = [0_u8; 16];
        OsRng.fill_bytes(&mut auth);
        let mut id = [0_u8; 8];
        OsRng.fill_bytes(&mut id);
        let id = B64.encode(id);
        let subscription = FakeSubscription {
            endpoint: format!("{}/push/{id}", self.shared.origin),
            p256dh: B64.encode(secret.public_key().to_encoded_point(false).as_bytes()),
            auth: B64.encode(auth),
            id: id.clone(),
        };
        self.shared.inner.lock().subscribers.insert(
            id,
            Subscriber {
                secret,
                auth,
                gone: false,
            },
        );
        subscription
    }

    /// Make the subscriber's endpoint answer 410 Gone from now on.
    pub fn expire(&self, id: &str) {
        if let Some(subscriber) = self.shared.inner.lock().subscribers.get_mut(id) {
            subscriber.gone = true;
        }
    }

    /// Every accepted delivery so far.
    pub fn deliveries(&self) -> Vec<Delivery> {
        self.shared.inner.lock().deliveries.clone()
    }

    /// Reasons for every refused request so far.
    pub fn rejections(&self) -> Vec<String> {
        self.shared.inner.lock().rejections.clone()
    }

    /// Wait until at least `count` deliveries were accepted.
    pub async fn wait_for(&self, count: usize, limit: Duration) -> Vec<Delivery> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let notified = self.shared.notify.notified();
            let deliveries = self.deliveries();
            if deliveries.len() >= count {
                return deliveries;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return self.deliveries();
            }
        }
    }
}

impl Drop for FakePushService {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn receive(
    State(shared): State<Arc<Shared>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, &'static str) {
    let reject = |status: StatusCode, reason: String| {
        let mut inner = shared.inner.lock();
        inner.rejections.push(reason);
        drop(inner);
        shared.notify.notify_waiters();
        (status, "rejected")
    };
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let Some(authorization) = header("authorization") else {
        return reject(StatusCode::UNAUTHORIZED, "missing authorization".into());
    };
    let (vapid_key, claims) = match crate::verify_vapid(authorization) {
        Ok(verified) => verified,
        Err(error) => return reject(StatusCode::UNAUTHORIZED, error.to_string()),
    };
    if claims.aud != shared.origin {
        return reject(StatusCode::FORBIDDEN, format!("audience {}", claims.aud));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    if claims.exp <= now || claims.exp > now + 24 * 60 * 60 {
        return reject(StatusCode::FORBIDDEN, "token expiry out of range".into());
    }
    if header("content-encoding") != Some("aes128gcm") {
        return reject(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "content encoding".into(),
        );
    }
    let Some(ttl) = header("ttl").and_then(|value| value.parse().ok()) else {
        return reject(StatusCode::BAD_REQUEST, "missing TTL".into());
    };
    let decrypted = {
        let inner = shared.inner.lock();
        let Some(subscriber) = inner.subscribers.get(&id) else {
            drop(inner);
            return reject(StatusCode::NOT_FOUND, format!("unknown subscriber {id}"));
        };
        if subscriber.gone {
            drop(inner);
            return reject(StatusCode::GONE, format!("expired subscriber {id}"));
        }
        crate::decrypt(&subscriber.secret, &subscriber.auth, &body)
    };
    match decrypted {
        Ok(plaintext) => {
            shared.inner.lock().deliveries.push(Delivery {
                subscriber: id,
                vapid_key,
                claims,
                ttl,
                plaintext,
            });
            shared.notify.notify_waiters();
            (StatusCode::CREATED, "")
        }
        Err(error) => reject(StatusCode::BAD_REQUEST, error.to_string()),
    }
}
