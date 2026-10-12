//! One WebAuthn ceremony on an agent tab through a temporary CDP virtual authenticator.
//!
//! The authenticator's whole life runs in a task of its own that the caller cannot cancel:
//! if the tool call is dropped, the task notices, tears the authenticator down and, when
//! teardown cannot be confirmed, closes the tab. While the authenticator exists, the tab
//! may only load documents of the authorized origin, so a page cannot carry the installed
//! credentials to another origin that shares the RP ID.
//!
//! Private keys cross only the daemon's CDP socket. Chromium necessarily holds them while
//! the authenticator exists, and the transport keeps ordinary buffers the daemon cannot
//! wipe; the daemon's own `Value` copies of listed credentials are zeroized.

use std::{sync::Arc, time::Duration};

use futures_util::future::BoxFuture;
use openside_browser::{Action as BrowserAction, Browser};
use openside_vault::{PasskeyCredential, PasskeyRelease, Secret, SignCount};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use zeroize::Zeroize;

#[derive(Clone, Copy)]
pub(super) struct Limits {
    /// Upper bound for the whole ceremony: install, page trigger, polling.
    pub ceremony: Duration,
    /// Upper bound for each teardown step.
    pub cleanup: Duration,
    pub poll: Duration,
    /// Pause between attempts to destroy a tab whose authenticator could not be confirmed gone.
    pub retry: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            ceremony: Duration::from_secs(15),
            cleanup: Duration::from_secs(5),
            poll: Duration::from_millis(100),
            retry: Duration::from_secs(2),
        }
    }
}

pub(super) enum Job {
    Register,
    Login(Vec<PasskeyRelease>),
}

pub(super) enum Outcome {
    Registered(Vec<PasskeyCredential>),
    SignedIn(Vec<SignCount>),
}

/// The browser operations a ceremony needs; the daemon implements it over [`Browser`].
pub(super) trait Port: Send + Sync + 'static {
    fn command<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, ()>>;
    /// Security origin of every frame of the tab, top frame first; opaque origins are not
    /// tuple origins (see [`is_opaque`]).
    fn frames(&self) -> BoxFuture<'_, Result<Vec<String>, ()>>;
    /// Create the ceremony's own tab, blank, in the same browser profile as the agent's tab.
    fn open(&self) -> BoxFuture<'_, Result<(), ()>>;
    /// Load the page the ceremony runs on into that tab; the guard is already installed.
    fn load(&self) -> BoxFuture<'_, Result<(), ()>>;
    /// Whether the browser was started with the isolation the ceremony relies on.
    fn documents_isolated(&self) -> bool;
    /// Click the page control that starts the ceremony.
    fn click(&self) -> BoxFuture<'_, Result<(), ()>>;
    /// Refuse document requests of the tab's frames outside `origin` until the guard is dropped.
    fn guard<'a>(&'a self, origin: &'a str) -> BoxFuture<'a, Result<Box<dyn Send>, ()>>;
    /// Close the tab; `Ok` only when the browser confirmed it.
    fn close_tab(&self) -> BoxFuture<'_, Result<(), ()>>;
    /// Kill the whole browser; `Ok` only once its connection is gone.
    fn terminate(&self) -> BoxFuture<'_, Result<(), ()>>;
}

/// Runs the ceremony in a tab of its own: the agent's tab is never given the authenticator,
/// so nothing that tab is loading or later commits can reach it.
pub(super) struct BrowserPort {
    browser: Arc<Browser>,
    owner: String,
    /// The page the agent's tab shows; the ceremony's tab loads it again.
    url: String,
    /// The control to press, found again by role and name in the page of the ceremony's tab.
    submit_role: String,
    submit_name: String,
    tab: Mutex<Option<String>>,
}

impl BrowserPort {
    pub fn new(
        browser: Arc<Browser>,
        owner: String,
        url: String,
        submit_role: String,
        submit_name: String,
    ) -> Self {
        Self {
            browser,
            owner,
            url,
            submit_role,
            submit_name,
            tab: Mutex::new(None),
        }
    }

    fn tab(&self) -> Result<String, ()> {
        self.tab.lock().clone().ok_or(())
    }
}

/// Origin of a document URL in the form of a frame's `securityOrigin`.
fn url_origin(url: &str) -> String {
    url::Url::parse(url)
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_default()
}

impl Port for BrowserPort {
    fn command<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, ()>> {
        Box::pin(async move {
            self.browser
                .webauthn_command(&self.owner, &self.tab()?, method, params)
                .await
                .map_err(|_| ())
        })
    }
    fn frames(&self) -> BoxFuture<'_, Result<Vec<String>, ()>> {
        Box::pin(async move {
            let tree = self
                .browser
                .page_command(&self.owner, &self.tab()?, "Page.getFrameTree", json!({}))
                .await
                .map_err(|_| ())?;
            let mut origins = Vec::new();
            collect_origins(&tree["frameTree"], &mut origins);
            Ok(origins)
        })
    }
    fn open(&self) -> BoxFuture<'_, Result<(), ()>> {
        Box::pin(async move {
            let tab = self
                .browser
                .create_agent_tab(&self.owner, "about:blank")
                .await
                .map_err(|_| ())?;
            *self.tab.lock() = Some(tab.id);
            Ok(())
        })
    }
    fn load(&self) -> BoxFuture<'_, Result<(), ()>> {
        Box::pin(async move {
            self.browser
                .navigate(&self.owner, &self.tab()?, &self.url)
                .await
                .map_err(|_| ())
        })
    }
    fn click(&self) -> BoxFuture<'_, Result<(), ()>> {
        Box::pin(async move {
            let tab = self.tab()?;
            // The page may still be building its controls; exactly one match is required
            // because the agent's reference cannot be told apart from a lookalike.
            for _ in 0..30 {
                let snapshot = self.browser.snapshot(&tab).await.map_err(|_| ())?;
                let mut found = snapshot.nodes.iter().filter(|node| {
                    node.role == self.submit_role
                        && node.name == self.submit_name
                        && node.reference.is_some()
                });
                match (found.next(), found.next()) {
                    (Some(node), None) => {
                        let reference = node.reference.clone().ok_or(())?;
                        return self
                            .browser
                            .act(&self.owner, &tab, &reference, BrowserAction::Click)
                            .await
                            .map(|_| ())
                            .map_err(|_| ());
                    }
                    (Some(_), Some(_)) => return Err(()),
                    (None, _) => tokio::time::sleep(Duration::from_millis(100)).await,
                }
            }
            Err(())
        })
    }
    fn guard<'a>(&'a self, origin: &'a str) -> BoxFuture<'a, Result<Box<dyn Send>, ()>> {
        Box::pin(async move {
            let origin = origin.to_owned();
            self.browser
                .guard_documents(
                    &self.owner,
                    &self.tab()?,
                    Arc::new(move |url| url_origin(url) == origin),
                )
                .await
                .map(|guard| Box::new(guard) as Box<dyn Send>)
                .map_err(|_| ())
        })
    }
    fn close_tab(&self) -> BoxFuture<'_, Result<(), ()>> {
        Box::pin(async move {
            // No tab was ever created: nothing holds an authenticator.
            let Ok(tab) = self.tab() else {
                return Ok(());
            };
            self.browser
                .close_agent_tab(&self.owner, &tab)
                .await
                .map_err(|_| ())
        })
    }
    fn terminate(&self) -> BoxFuture<'_, Result<(), ()>> {
        Box::pin(async move { self.browser.terminate().await.map_err(|_| ()) })
    }
    fn documents_isolated(&self) -> bool {
        self.browser.documents_isolated()
    }
}

/// Pre-order security origins of a `Page.getFrameTree` node and its children.
fn collect_origins(node: &Value, origins: &mut Vec<String>) {
    origins.push(
        node["frame"]["securityOrigin"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    );
    for child in node["childFrames"].as_array().into_iter().flatten() {
        collect_origins(child, origins);
    }
}

/// WebAuthn refuses an opaque origin (sandboxed or `data:` frame), so such a frame cannot use
/// the authenticator.
fn is_opaque(origin: &str) -> bool {
    url::Url::parse(origin).map_or(true, |url| !url.has_host())
}

/// Whether the tab shows nothing but the authorized origin: the top frame exactly, and every
/// other frame the same origin (or opaque). A document of any other origin could call the
/// installed authenticator with the same RP ID.
async fn exclusive<P: Port>(port: &P, origin: &str) -> Result<(), ()> {
    let frames = port.frames().await?;
    let (top, rest) = frames.split_first().ok_or(())?;
    if top == origin && rest.iter().all(|frame| frame == origin || is_opaque(frame)) {
        Ok(())
    } else {
        Err(())
    }
}

/// What was asked of the browser, so teardown knows what may exist even when the future
/// that asked was cancelled mid-request.
#[derive(Default)]
struct Installed {
    enabled: bool,
    add_sent: bool,
    authenticator: Option<String>,
}

/// Run one ceremony to completion or failure; the authenticator never outlives this call,
/// not even when the caller drops the returned future.
pub(super) async fn run<P: Port>(
    port: Arc<P>,
    origin: String,
    limits: Limits,
    job: Job,
) -> Result<Outcome, ()> {
    let (alive, gone) = oneshot::channel::<()>();
    let task = tokio::spawn(lifecycle(port, origin, limits, job, gone));
    let result = task.await.map_err(|_| ())?;
    drop(alive);
    result
}

async fn lifecycle<P: Port>(
    port: Arc<P>,
    origin: String,
    limits: Limits,
    job: Job,
    gone: oneshot::Receiver<()>,
) -> Result<Outcome, ()> {
    // A browser re-adopted from an older build can still activate prerendered documents into
    // a tab without a request, which nothing here can see or stop.
    if !port.documents_isolated() {
        return Err(());
    }
    tokio::time::timeout(limits.cleanup, port.open())
        .await
        .map_err(|_| ())??;
    let guard = match tokio::time::timeout(limits.cleanup, port.guard(&origin)).await {
        Ok(Ok(guard)) => guard,
        _ => {
            // No authenticator exists yet; the blank tab is only tidied up.
            let _ = tokio::time::timeout(limits.cleanup, port.close_tab()).await;
            return Err(());
        }
    };
    let mut installed = Installed::default();
    let outcome = {
        let work = tokio::time::timeout(
            limits.ceremony,
            work(&*port, &origin, &job, limits, &mut installed),
        );
        tokio::select! {
            result = work => result.unwrap_or(Err(())),
            // The sender lives exactly as long as the caller's future.
            _ = gone => Err(()),
        }
    };
    let removed = tokio::time::timeout(limits.cleanup, release(&*port, &installed))
        .await
        .unwrap_or(false);
    // The ceremony's tab is always closed. An authenticator that was not confirmed removed
    // is destroyed along with the tab, or the browser, before success is possible.
    let closed = matches!(
        tokio::time::timeout(limits.cleanup, port.close_tab()).await,
        Ok(Ok(()))
    );
    if closed || removed || terminated(&*port, limits).await {
        drop(guard);
        return outcome;
    }
    // The authenticator may still answer the page and the tab is still there: no success, and
    // the guard stays until a later attempt really destroys the tab or the browser.
    tokio::spawn(keep_guard_until_gone(port, guard, limits));
    Err(())
}

/// Close the tab; when that is not confirmed either, kill the browser, which takes every tab
/// with it and is relaunched by the controller on next use.
async fn destroyed<P: Port>(port: &P, limits: Limits) -> bool {
    matches!(
        tokio::time::timeout(limits.cleanup, port.close_tab()).await,
        Ok(Ok(()))
    ) || terminated(port, limits).await
}

async fn terminated<P: Port>(port: &P, limits: Limits) -> bool {
    matches!(
        tokio::time::timeout(limits.cleanup, port.terminate()).await,
        Ok(Ok(()))
    )
}

async fn keep_guard_until_gone<P: Port>(port: Arc<P>, guard: Box<dyn Send>, limits: Limits) {
    loop {
        tokio::time::sleep(limits.retry).await;
        if destroyed(&*port, limits).await {
            drop(guard);
            return;
        }
    }
}

/// Whether the virtual authenticator is certainly gone.
async fn release<P: Port>(port: &P, installed: &Installed) -> bool {
    let confirmed = match &installed.authenticator {
        Some(id) => port
            .command(
                "WebAuthn.removeVirtualAuthenticator",
                json!({"authenticatorId": id}),
            )
            .await
            .is_ok(),
        None => !installed.add_sent,
    };
    if installed.enabled {
        let _ = port.command("WebAuthn.disable", json!({})).await;
    }
    confirmed
}

/// Zeroize every string of a CDP value that may carry private-key material.
fn wipe(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(items) => items.iter_mut().for_each(wipe),
        Value::Object(map) => map.values_mut().for_each(wipe),
        _ => {}
    }
}

async fn credentials<P: Port>(port: &P, authenticator: &str) -> Result<Vec<Value>, ()> {
    let mut listed = port
        .command(
            "WebAuthn.getCredentials",
            json!({"authenticatorId": authenticator}),
        )
        .await?;
    let credentials = match listed["credentials"].take() {
        Value::Array(credentials) => Ok(credentials),
        mut other => {
            wipe(&mut other);
            Err(())
        }
    };
    wipe(&mut listed);
    credentials
}

async fn work<P: Port>(
    port: &P,
    origin: &str,
    job: &Job,
    limits: Limits,
    installed: &mut Installed,
) -> Result<Outcome, ()> {
    // The guard was installed on the blank tab before its first navigation, so every document
    // this tab shows was judged by it: nothing was in flight, restored or prerendered earlier.
    // The frames are still read, since a request the guard cannot judge may exist.
    port.load().await?;
    exclusive(port, origin).await?;
    installed.enabled = true;
    port.command("WebAuthn.enable", json!({"enableUI": false}))
        .await?;
    installed.add_sent = true;
    let created = port
        .command(
            "WebAuthn.addVirtualAuthenticator",
            json!({"options": {
                "protocol": "ctap2",
                "transport": "internal",
                "hasResidentKey": true,
                "hasUserVerification": true,
                "isUserVerified": true,
                "automaticPresenceSimulation": true,
            }}),
        )
        .await?;
    let authenticator = created["authenticatorId"].as_str().ok_or(())?.to_owned();
    installed.authenticator = Some(authenticator.clone());
    // Whatever committed while the authenticator was being added is found here.
    exclusive(port, origin).await?;
    match job {
        Job::Register => register(port, origin, limits, &authenticator).await,
        Job::Login(keys) => login(port, origin, limits, &authenticator, keys).await,
    }
}

async fn register<P: Port>(
    port: &P,
    origin: &str,
    limits: Limits,
    authenticator: &str,
) -> Result<Outcome, ()> {
    port.click().await?;
    loop {
        exclusive(port, origin).await?;
        let mut listed = credentials(port, authenticator).await?;
        if !listed.is_empty() {
            let captured: Result<Vec<_>, ()> = listed.drain(..).map(captured).collect();
            return captured.map(Outcome::Registered);
        }
        tokio::time::sleep(limits.poll).await;
    }
}

async fn login<P: Port>(
    port: &P,
    origin: &str,
    limits: Limits,
    authenticator: &str,
    keys: &[PasskeyRelease],
) -> Result<Outcome, ()> {
    for key in keys {
        let mut credential = json!({
            "credentialId": key.credential_id,
            "isResidentCredential": key.user_handle.is_some(),
            "rpId": key.rp_id,
            "privateKey": key.private_key.as_str(),
            "signCount": key.sign_count,
        });
        if let Some(handle) = &key.user_handle {
            credential["userHandle"] = json!(handle);
        }
        port.command(
            "WebAuthn.addCredential",
            json!({"authenticatorId": authenticator, "credential": credential}),
        )
        .await?;
    }
    exclusive(port, origin).await?;
    port.click().await?;
    loop {
        exclusive(port, origin).await?;
        let mut listed = credentials(port, authenticator).await?;
        let counts: Vec<SignCount> = listed
            .iter()
            .filter_map(|credential| {
                Some(SignCount {
                    credential_id: credential["credentialId"].as_str()?.to_owned(),
                    sign_count: u32::try_from(credential["signCount"].as_u64()?).ok()?,
                })
            })
            .collect();
        listed.iter_mut().for_each(wipe);
        let used = counts.iter().any(|count| {
            keys.iter().any(|key| {
                key.credential_id == count.credential_id && count.sign_count > key.sign_count
            })
        });
        if used {
            return Ok(Outcome::SignedIn(counts));
        }
        tokio::time::sleep(limits.poll).await;
    }
}

fn captured(mut credential: Value) -> Result<PasskeyCredential, ()> {
    let text = |value: &Value| value.as_str().map(str::to_owned).ok_or(());
    let private_key = match credential["privateKey"].take() {
        Value::String(key) => Secret::new(key),
        _ => return Err(()),
    };
    PasskeyCredential::new(
        text(&credential["credentialId"])?,
        text(&credential["rpId"])?,
        credential["userHandle"].as_str().map(str::to_owned),
        private_key,
        u32::try_from(credential["signCount"].as_u64().unwrap_or(0)).map_err(|_| ())?,
    )
    .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use parking_lot::Mutex;

    use super::*;

    const ORIGIN: &str = "https://example.com";

    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// Scripted browser: records every call, and can fail or hang chosen methods.
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
        fail: Vec<&'static str>,
        hang: Vec<&'static str>,
        origin: Mutex<Option<String>>,
        /// Credentials the authenticator reports after the page was clicked.
        listed: Mutex<Vec<Value>>,
        guard_dropped: Arc<AtomicBool>,
        /// Origins of the frames below the top frame.
        nested: Mutex<Vec<String>>,
        /// Once set, closing the tab and killing the browser succeed.
        healed: AtomicBool,
        /// Top-frame origin the page ends up at once it was loaded, as a redirect would.
        after_load: Mutex<Option<String>>,
        /// A browser re-adopted without the isolation marker.
        unisolated: bool,
    }

    impl Fake {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().clone()
        }
        async fn step(&self, name: &str) -> Result<(), ()> {
            self.calls.lock().push(name.to_owned());
            if self.healed.load(Ordering::SeqCst) && matches!(name, "close_tab" | "terminate") {
                return Ok(());
            }
            if self.hang.contains(&name) {
                std::future::pending::<()>().await;
            }
            if self.fail.contains(&name) {
                return Err(());
            }
            Ok(())
        }
    }

    impl Port for Fake {
        fn command<'a>(
            &'a self,
            method: &'a str,
            _params: Value,
        ) -> BoxFuture<'a, Result<Value, ()>> {
            Box::pin(async move {
                self.step(method).await?;
                Ok(match method {
                    "WebAuthn.addVirtualAuthenticator" => json!({"authenticatorId": "auth-1"}),
                    "WebAuthn.getCredentials" => {
                        json!({"credentials": self.listed.lock().clone()})
                    }
                    _ => json!({}),
                })
            })
        }
        fn frames(&self) -> BoxFuture<'_, Result<Vec<String>, ()>> {
            Box::pin(async move {
                self.step("frames").await?;
                let top = self
                    .origin
                    .lock()
                    .clone()
                    .unwrap_or_else(|| ORIGIN.to_owned());
                Ok([vec![top], self.nested.lock().clone()].concat())
            })
        }
        fn open(&self) -> BoxFuture<'_, Result<(), ()>> {
            Box::pin(async move { self.step("open").await })
        }
        fn load(&self) -> BoxFuture<'_, Result<(), ()>> {
            Box::pin(async move {
                self.step("load").await?;
                if let Some(origin) = self.after_load.lock().clone() {
                    *self.origin.lock() = Some(origin);
                }
                Ok(())
            })
        }
        fn documents_isolated(&self) -> bool {
            !self.unisolated
        }
        fn click(&self) -> BoxFuture<'_, Result<(), ()>> {
            Box::pin(async move { self.step("click").await })
        }
        fn guard<'a>(&'a self, origin: &'a str) -> BoxFuture<'a, Result<Box<dyn Send>, ()>> {
            Box::pin(async move {
                self.step(&format!("guard {origin}")).await?;
                Ok(Box::new(Dropped(self.guard_dropped.clone())) as Box<dyn Send>)
            })
        }
        fn close_tab(&self) -> BoxFuture<'_, Result<(), ()>> {
            Box::pin(async move { self.step("close_tab").await })
        }
        fn terminate(&self) -> BoxFuture<'_, Result<(), ()>> {
            Box::pin(async move { self.step("terminate").await })
        }
    }

    fn quick() -> Limits {
        Limits {
            ceremony: Duration::from_millis(400),
            cleanup: Duration::from_millis(200),
            poll: Duration::from_millis(5),
            retry: Duration::from_millis(20),
        }
    }

    fn release_key() -> PasskeyRelease {
        PasskeyRelease {
            credential_id: "cred-1".into(),
            rp_id: "example.com".into(),
            user_handle: Some("handle".into()),
            private_key: zeroize::Zeroizing::new("private-key-bytes".into()),
            sign_count: 4,
        }
    }

    fn registered() -> Value {
        json!({
            "credentialId": "cred-new",
            "rpId": "example.com",
            "userHandle": "handle",
            "privateKey": "private-key-material",
            "signCount": 0,
        })
    }

    async fn eventually(what: &str, condition: impl Fn() -> bool) {
        for _ in 0..200 {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{what}");
    }

    #[tokio::test]
    async fn register_installs_under_a_guard_and_always_removes_the_authenticator() {
        let fake = Arc::new(Fake::default());
        *fake.listed.lock() = vec![registered()];
        let outcome = run(fake.clone(), ORIGIN.into(), quick(), Job::Register).await;
        assert!(matches!(outcome, Ok(Outcome::Registered(keys)) if keys.len() == 1));
        let calls = fake.calls();
        let position = |name: &str| calls.iter().position(|call| call == name).unwrap();
        assert!(
            position(&format!("guard {ORIGIN}")) < position("WebAuthn.addVirtualAuthenticator")
        );
        assert!(position("click") < position("WebAuthn.getCredentials"));
        assert!(
            position("WebAuthn.getCredentials") < position("WebAuthn.removeVirtualAuthenticator")
        );
        assert!(!calls.contains(&"terminate".to_owned()));
        assert!(
            fake.guard_dropped.load(Ordering::SeqCst),
            "the guard outlived the ceremony"
        );
    }

    #[tokio::test]
    async fn dropping_the_caller_after_add_credential_still_removes_the_authenticator() {
        let fake = Arc::new(Fake {
            hang: vec!["click"],
            ..Fake::default()
        });
        let limits = Limits {
            ceremony: Duration::from_secs(60),
            ..quick()
        };
        // The caller gives up while the ceremony waits for the page.
        let caller = tokio::time::timeout(
            Duration::from_millis(50),
            run(
                fake.clone(),
                ORIGIN.into(),
                limits,
                Job::Login(vec![release_key()]),
            ),
        )
        .await;
        assert!(caller.is_err());
        let probe = fake.clone();
        eventually(
            "authenticator was not removed after the caller was dropped",
            move || {
                probe
                    .calls()
                    .contains(&"WebAuthn.removeVirtualAuthenticator".to_owned())
            },
        )
        .await;
        assert!(fake.calls().contains(&"WebAuthn.addCredential".to_owned()));
        let probe = fake.guard_dropped.clone();
        eventually("guard still active", move || probe.load(Ordering::SeqCst)).await;
        assert!(fake.calls().contains(&"close_tab".to_owned()));
        assert!(!fake.calls().contains(&"terminate".to_owned()));
    }

    #[tokio::test]
    async fn a_hanging_cdp_call_hits_the_overall_timeout_and_cleans_up() {
        let fake = Arc::new(Fake {
            hang: vec!["WebAuthn.addCredential"],
            ..Fake::default()
        });
        let started = std::time::Instant::now();
        let outcome = run(
            fake.clone(),
            ORIGIN.into(),
            quick(),
            Job::Login(vec![release_key()]),
        )
        .await;
        assert!(outcome.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(fake
            .calls()
            .contains(&"WebAuthn.removeVirtualAuthenticator".to_owned()));
        assert!(fake.guard_dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn failed_login_commands_still_clean_up() {
        let fake = Arc::new(Fake {
            fail: vec!["WebAuthn.addCredential"],
            ..Fake::default()
        });
        let outcome = run(
            fake.clone(),
            ORIGIN.into(),
            quick(),
            Job::Login(vec![release_key()]),
        )
        .await;
        assert!(outcome.is_err());
        let calls = fake.calls();
        assert!(calls.contains(&"WebAuthn.removeVirtualAuthenticator".to_owned()));
        assert!(calls.contains(&"WebAuthn.disable".to_owned()));
        assert!(!calls.contains(&"click".to_owned()));
    }

    #[tokio::test]
    async fn unconfirmed_removal_closes_the_tab() {
        let fake = Arc::new(Fake {
            fail: vec!["WebAuthn.removeVirtualAuthenticator"],
            ..Fake::default()
        });
        *fake.listed.lock() = vec![registered()];
        let outcome = run(fake.clone(), ORIGIN.into(), quick(), Job::Register).await;
        // The captured key is not thrown away because teardown failed.
        assert!(matches!(outcome, Ok(Outcome::Registered(_))));
        assert!(fake.calls().contains(&"close_tab".to_owned()));

        let hanging = Arc::new(Fake {
            hang: vec!["WebAuthn.removeVirtualAuthenticator"],
            ..Fake::default()
        });
        *hanging.listed.lock() = vec![registered()];
        let outcome = run(hanging.clone(), ORIGIN.into(), quick(), Job::Register).await;
        assert!(outcome.is_ok());
        assert!(hanging.calls().contains(&"close_tab".to_owned()));
    }

    #[tokio::test]
    async fn an_add_request_without_an_answer_cannot_be_confirmed_gone() {
        let fake = Arc::new(Fake {
            fail: vec!["WebAuthn.addVirtualAuthenticator"],
            ..Fake::default()
        });
        assert!(run(fake.clone(), ORIGIN.into(), quick(), Job::Register)
            .await
            .is_err());
        let calls = fake.calls();
        assert!(!calls.contains(&"WebAuthn.removeVirtualAuthenticator".to_owned()));
        assert!(calls.contains(&"close_tab".to_owned()));

        // Nothing was installed when the domain could not even be enabled.
        let fake = Arc::new(Fake {
            fail: vec!["WebAuthn.enable"],
            ..Fake::default()
        });
        assert!(run(fake.clone(), ORIGIN.into(), quick(), Job::Register)
            .await
            .is_err());
        assert!(!fake.calls().contains(&"terminate".to_owned()));
    }

    #[tokio::test]
    async fn leaving_the_authorized_origin_ends_the_ceremony_without_a_result() {
        let fake = Arc::new(Fake::default());
        let moving = fake.clone();
        let mover = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            *moving.origin.lock() = Some("https://other.example.com".into());
        });
        let outcome = run(
            fake.clone(),
            ORIGIN.into(),
            quick(),
            Job::Login(vec![release_key()]),
        )
        .await;
        mover.await.unwrap();
        assert!(outcome.is_err());
        assert!(fake
            .calls()
            .contains(&"WebAuthn.removeVirtualAuthenticator".to_owned()));
    }

    #[tokio::test]
    async fn login_reports_counters_only_after_the_credential_was_used() {
        let fake = Arc::new(Fake::default());
        let credentials = fake.clone();
        let user = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            *credentials.listed.lock() = vec![json!({
                "credentialId": "cred-1",
                "rpId": "example.com",
                "privateKey": "private-key-bytes",
                "signCount": 5,
            })];
        });
        let outcome = run(
            fake.clone(),
            ORIGIN.into(),
            quick(),
            Job::Login(vec![release_key()]),
        )
        .await;
        user.await.unwrap();
        let Ok(Outcome::SignedIn(counts)) = outcome else {
            panic!("login should succeed");
        };
        assert_eq!(counts.len(), 1);
        assert_eq!(counts[0].sign_count, 5);
    }

    #[test]
    fn the_navigation_rule_compares_exact_origins() {
        assert_eq!(
            url_origin("https://example.com/a?b#c"),
            "https://example.com"
        );
        assert_eq!(
            url_origin("http://localhost:8080/x"),
            "http://localhost:8080"
        );
        assert_ne!(
            url_origin("http://localhost:8081/x"),
            "http://localhost:8080"
        );
        assert_ne!(url_origin("https://a.example.com/"), "https://example.com");
        assert_eq!(url_origin("data:text/html,x"), "null");
    }

    #[test]
    fn wipe_clears_every_string_in_a_credentials_listing() {
        let mut listed =
            json!({"credentials": [registered()], "nested": {"k": "private-key-material"}});
        wipe(&mut listed);
        assert!(!listed.to_string().contains("private-key-material"));
    }

    #[test]
    fn frame_origins_are_collected_top_first_and_opaque_ones_recognised() {
        let tree = json!({
            "frame": {"securityOrigin": "https://example.com"},
            "childFrames": [
                {"frame": {"securityOrigin": "https://ads.example.net"}, "childFrames": [
                    {"frame": {"securityOrigin": "://"}}
                ]},
                {"frame": {"securityOrigin": "https://example.com"}},
            ],
        });
        let mut origins = Vec::new();
        collect_origins(&tree, &mut origins);
        assert_eq!(
            origins,
            [
                "https://example.com",
                "https://ads.example.net",
                "://",
                "https://example.com"
            ]
        );
        assert!(is_opaque("://") && is_opaque("null") && is_opaque(""));
        assert!(!is_opaque("http://localhost:8080"));
    }

    #[tokio::test]
    async fn a_frame_of_another_origin_refuses_the_ceremony_before_anything_is_installed() {
        let fake = Arc::new(Fake::default());
        *fake.nested.lock() = vec!["https://login.example.com".into()];
        assert!(run(
            fake.clone(),
            ORIGIN.into(),
            quick(),
            Job::Login(vec![release_key()])
        )
        .await
        .is_err());
        let calls = fake.calls();
        assert!(!calls.contains(&"WebAuthn.addVirtualAuthenticator".to_owned()));
        assert!(!calls.contains(&"WebAuthn.addCredential".to_owned()));
        assert!(!calls.contains(&"click".to_owned()));
        assert!(fake.guard_dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn the_ceremony_runs_in_its_own_tab_that_is_guarded_before_it_loads_anything() {
        let fake = Arc::new(Fake::default());
        *fake.listed.lock() = vec![registered()];
        run(fake.clone(), ORIGIN.into(), quick(), Job::Register)
            .await
            .ok();
        let calls = fake.calls();
        let at = |name: &str| calls.iter().position(|call| call == name).unwrap();
        assert!(at("open") < at(&format!("guard {ORIGIN}")));
        assert!(at(&format!("guard {ORIGIN}")) < at("load"));
        assert!(
            calls[at("load")..at("WebAuthn.addVirtualAuthenticator")]
                .iter()
                .any(|call| call == "frames"),
            "the frames must be read after loading and before the authenticator exists: {calls:?}"
        );
        assert!(at("WebAuthn.removeVirtualAuthenticator") < at("close_tab"));
        assert!(!calls.contains(&"terminate".to_owned()));
    }

    #[tokio::test]
    async fn a_page_that_ends_up_on_another_origin_refuses_before_anything_is_installed() {
        let fake = Arc::new(Fake::default());
        *fake.after_load.lock() = Some("https://login.example.com".into());
        assert!(run(
            fake.clone(),
            ORIGIN.into(),
            quick(),
            Job::Login(vec![release_key()])
        )
        .await
        .is_err());
        let calls = fake.calls();
        assert!(!calls.contains(&"WebAuthn.addVirtualAuthenticator".to_owned()));
        assert!(!calls.contains(&"WebAuthn.addCredential".to_owned()));
        assert!(!calls.contains(&"click".to_owned()));
        assert!(calls.contains(&"close_tab".to_owned()));
        assert!(fake.guard_dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_tab_that_cannot_be_opened_guarded_or_loaded_refuses_the_ceremony() {
        for failing in ["open", "guard https://example.com", "load"] {
            let fake = Arc::new(Fake {
                fail: vec![failing],
                ..Fake::default()
            });
            assert!(run(
                fake.clone(),
                ORIGIN.into(),
                quick(),
                Job::Login(vec![release_key()])
            )
            .await
            .is_err());
            let calls = fake.calls();
            for forbidden in [
                "WebAuthn.enable",
                "WebAuthn.addVirtualAuthenticator",
                "WebAuthn.addCredential",
                "click",
            ] {
                assert!(
                    !calls.contains(&forbidden.to_owned()),
                    "{forbidden} ran after {failing} failed: {calls:?}"
                );
            }
            // A tab that was opened is closed again even though it never got an authenticator.
            assert_eq!(
                calls.contains(&"close_tab".to_owned()),
                failing != "open",
                "{failing}: {calls:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_browser_not_started_with_the_isolation_refuses_the_ceremony() {
        let fake = Arc::new(Fake {
            unisolated: true,
            ..Fake::default()
        });
        assert!(run(fake.clone(), ORIGIN.into(), quick(), Job::Register)
            .await
            .is_err());
        let calls = fake.calls();
        for forbidden in [
            "open",
            "load",
            "WebAuthn.enable",
            "WebAuthn.addVirtualAuthenticator",
            "click",
        ] {
            assert!(
                !calls.contains(&forbidden.to_owned()),
                "{forbidden} ran: {calls:?}"
            );
        }
    }

    #[tokio::test]
    async fn frames_of_the_same_or_an_opaque_origin_do_not_get_in_the_way() {
        let fake = Arc::new(Fake::default());
        *fake.nested.lock() = vec![ORIGIN.into(), "://".into()];
        *fake.listed.lock() = vec![registered()];
        let outcome = run(fake.clone(), ORIGIN.into(), quick(), Job::Register).await;
        assert!(matches!(outcome, Ok(Outcome::Registered(_))));
    }

    #[tokio::test]
    async fn a_frame_committing_during_the_ceremony_removes_the_authenticator() {
        let fake = Arc::new(Fake::default());
        let adding = fake.clone();
        let frame = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            *adding.nested.lock() = vec!["https://other.example.com".into()];
        });
        let outcome = run(
            fake.clone(),
            ORIGIN.into(),
            quick(),
            Job::Login(vec![release_key()]),
        )
        .await;
        frame.await.unwrap();
        assert!(outcome.is_err());
        assert!(fake
            .calls()
            .contains(&"WebAuthn.removeVirtualAuthenticator".to_owned()));
    }

    #[tokio::test]
    async fn failed_removal_and_failed_close_escalate_to_killing_the_browser() {
        let fake = Arc::new(Fake {
            fail: vec!["WebAuthn.removeVirtualAuthenticator", "close_tab"],
            ..Fake::default()
        });
        *fake.listed.lock() = vec![registered()];
        let outcome = run(fake.clone(), ORIGIN.into(), quick(), Job::Register).await;
        let calls = fake.calls();
        let position = |name: &str| calls.iter().position(|call| call == name).unwrap();
        assert!(position("close_tab") < position("terminate"));
        // The browser is gone, so nothing can answer the page any more.
        assert!(matches!(outcome, Ok(Outcome::Registered(_))));
        assert!(fake.guard_dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn nothing_confirmed_is_a_failure_and_the_guard_stays_until_the_tab_is_gone() {
        let fake = Arc::new(Fake {
            fail: vec![
                "WebAuthn.removeVirtualAuthenticator",
                "WebAuthn.disable",
                "close_tab",
                "terminate",
            ],
            ..Fake::default()
        });
        *fake.listed.lock() = vec![registered()];
        let outcome = run(fake.clone(), ORIGIN.into(), quick(), Job::Register).await;
        // A captured key is not reported as success while the authenticator may still be live.
        assert!(outcome.is_err());
        assert!(fake.calls().contains(&"terminate".to_owned()));
        assert!(
            !fake.guard_dropped.load(Ordering::SeqCst),
            "the guard was released with the authenticator unconfirmed"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!fake.guard_dropped.load(Ordering::SeqCst));

        // Once a retry destroys the tab or the browser, the restriction ends.
        fake.healed.store(true, Ordering::SeqCst);
        let probe = fake.guard_dropped.clone();
        eventually(
            "the guard was never released after the tab was destroyed",
            move || probe.load(Ordering::SeqCst),
        )
        .await;
    }

    #[tokio::test]
    async fn hanging_removal_close_and_kill_are_bounded_and_fail_closed() {
        let fake = Arc::new(Fake {
            hang: vec![
                "WebAuthn.removeVirtualAuthenticator",
                "WebAuthn.disable",
                "close_tab",
                "terminate",
            ],
            ..Fake::default()
        });
        *fake.listed.lock() = vec![registered()];
        let started = std::time::Instant::now();
        let outcome = run(fake.clone(), ORIGIN.into(), quick(), Job::Register).await;
        assert!(outcome.is_err());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "teardown must be bounded"
        );
        let calls = fake.calls();
        assert!(calls.contains(&"close_tab".to_owned()) && calls.contains(&"terminate".to_owned()));
        assert!(!fake.guard_dropped.load(Ordering::SeqCst));
    }
}
