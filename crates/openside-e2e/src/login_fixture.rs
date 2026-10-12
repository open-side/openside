//! Loopback sign-in sites for the login scenarios (`docs/specs/e2e-scenarios.md`, requirement
//! C-10): a relying party ("Northwind") with email/password, two-step username, phone-push,
//! SMS and authenticator codes, authenticator enrollment, mid-login choices and wrong-password
//! errors, plus an identity provider with an account chooser and a consent page. Everything is
//! deterministic and held in memory; `/__fixture/*` exposes the state for assertions and lets
//! a test approve or deny a phone push the way the person's phone would.

use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{
    extract::{Form, Query, State},
    http::header,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use parking_lot::Mutex;
use serde_json::{json, Value};

/// Email + password account (S1, S7, S10).
pub const ALICE: (&str, &str) = ("alice@northwind.test", "correct-horse-7781");
/// Username of the two-step sign-in (S2).
pub const BOB: (&str, &str) = ("bob", "bob-pass-5521");
/// Phone-push account (S4).
pub const PAT: (&str, &str) = ("pat@northwind.test", "push-pass-3317");
/// SMS-code account (S5).
pub const SAM: (&str, &str) = ("sam@northwind.test", "sms-pass-6604");
/// Account with an authenticator already set up in the person's own app (S5).
pub const TOM: (&str, &str) = ("tom@northwind.test", "totp-pass-4410");
/// Account offered authenticator enrollment on its first sign-in (S8, S9).
pub const TESS: (&str, &str) = ("tess@northwind.test", "enroll-pass-2208");
/// Account whose sign-in walks through method, profile, trust and terms choices (S6).
pub const CARA: (&str, &str) = ("cara@northwind.test", "choice-pass-9001");
/// The code the site "texts" to SAM and CARA.
pub const SMS_CODE: &str = "482913";
/// Accounts the identity provider already knows in this browser (S3).
pub const IDP_ACCOUNTS: [&str; 2] = ["alice@gmail.test", "alice.work@corp.test"];
/// RFC 6238's SHA-1 test key: the authenticator TOM's app holds and the one TESS's enrollment
/// page shows.
const TOTP_KEY: &[u8] = b"12345678901234567890";
/// [`TOTP_KEY`] in base32, as the enrollment page shows it.
pub const TOTP_SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

/// The fixture's own TOTP for `unix`, computed independently of the vault.
pub fn totp_at(unix: u64) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, TOTP_KEY);
    let tag = ring::hmac::sign(&key, &(unix / 30).to_be_bytes());
    let digest = tag.as_ref();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let value = u32::from_be_bytes([
        digest[offset],
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]) & 0x7fff_ffff;
    format!("{:06}", value % 1_000_000)
}

/// The current TOTP, as the person's authenticator app would show it.
pub fn totp_now() -> String {
    totp_at(now())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn totp_ok(code: &str) -> bool {
    let now = now();
    // One step of clock drift either way, as real verifiers allow.
    [now.saturating_sub(30), now, now + 30]
        .iter()
        .any(|t| totp_at(*t) == code)
}

#[derive(Default)]
struct Site {
    /// Who is signed in, and how ("password", "oauth:<account>", "push" …).
    signed_in: Option<(String, String)>,
    /// Sign-in step the browser is in, for the account that passed its password.
    pending: Option<String>,
    /// Every form post, path and fields, in order.
    posts: Vec<(String, HashMap<String, String>)>,
    push: PushState,
    enrolled: bool,
    method: Option<String>,
    profile: Option<String>,
    trusted: Option<bool>,
    terms: Option<bool>,
    oauth_consent: Option<(String, bool)>,
    /// Codes the site accepted or rejected: (kind, ok).
    codes: Vec<(String, bool)>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum PushState {
    #[default]
    Idle,
    Pending,
    Approved,
    Denied,
    Expired,
}

impl PushState {
    fn wire(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Expired => "expired",
        }
    }
}

type Shared = Arc<Mutex<Site>>;

/// A running fixture: the relying party at `origin`, the identity provider at `idp_origin`.
pub struct LoginFixture {
    /// `http://localhost:<port>`, the Northwind site.
    pub origin: String,
    /// `http://127.0.0.1:<port>`, the identity provider.
    pub idp_origin: String,
    site: Shared,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for LoginFixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

const STYLE: &str = "<style>
:root{color-scheme:light;font-family:system-ui,-apple-system,'Segoe UI',sans-serif}
body{margin:0;background:#f3f5f9;color:#14182b}
header{display:flex;align-items:center;gap:10px;padding:14px 24px;background:#fff;border-bottom:1px solid #e3e7ef;font-weight:650}
.logo{display:inline-grid;place-items:center;width:28px;height:28px;border-radius:8px;background:#1f4fd8;color:#fff;font-size:14px}
main{max-width:400px;margin:48px auto;padding:32px;background:#fff;border:1px solid #e3e7ef;border-radius:16px}
h1{font-size:22px;margin:0 0 6px}p{color:#4a5168;line-height:1.5}
label{display:block;font-size:14px;margin:14px 0 6px;color:#2a2f45}
input[type=text],input[type=email],input[type=password],input[inputmode=numeric]{box-sizing:border-box;width:100%;padding:11px 12px;border:1px solid #c9cfdc;border-radius:10px;font-size:15px}
.check{display:flex;gap:8px;align-items:center;margin-top:12px}.check input{width:auto}
button,.button{display:block;box-sizing:border-box;width:100%;margin-top:16px;padding:11px;border:0;border-radius:10px;background:#1f4fd8;color:#fff;font-size:15px;font-weight:600;text-align:center;text-decoration:none;cursor:pointer}
button.secondary{background:#eef1f7;color:#14182b}
.or{display:flex;align-items:center;gap:10px;margin:18px 0 0;color:#8a90a5;font-size:13px}.or:before,.or:after{content:'';flex:1;border-top:1px solid #e3e7ef}
[role=alert]{margin:14px 0 0;padding:10px 12px;border-radius:10px;background:#fdecea;color:#9b1c1c;font-size:14px}
.number{font-size:48px;font-weight:700;text-align:center;margin:18px 0}
.key{font-family:ui-monospace,monospace;letter-spacing:.08em;background:#f3f5f9;padding:8px 10px;border-radius:8px}
ul.choices{list-style:none;padding:0;margin:16px 0 0}ul.choices li{margin:8px 0}ul.choices button{margin:0;background:#fff;color:#14182b;border:1px solid #c9cfdc;text-align:left}
.radio{display:flex;gap:10px;align-items:flex-start;padding:12px;border:1px solid #c9cfdc;border-radius:10px;margin:10px 0}.radio input{margin-top:3px}
small{color:#6b7189}
</style>";

fn page(site: &str, title: &str, body: &str) -> Html<String> {
    Html(format!(
        "<!doctype html><html lang='en'><head><meta charset='utf-8'><title>{title}</title>\
         <meta name='viewport' content='width=device-width,initial-scale=1'>\
         <meta property='og:site_name' content='{site}'>{STYLE}</head><body>\
         <header><span class='logo'>{}</span>{site}</header><main>{body}</main></body></html>",
        &site[..1]
    ))
}

fn rp(title: &str, body: &str) -> Html<String> {
    page("Northwind", title, body)
}

fn alert(error: Option<&str>) -> String {
    error
        .map(|text| format!("<div role='alert'>{text}</div>"))
        .unwrap_or_default()
}

fn login_page(idp: &str, error: Option<&str>, email: &str) -> Html<String> {
    rp(
        "Sign in · Northwind",
        &format!(
            "<h1>Sign in to Northwind</h1><p>Use your Northwind account.</p>\
             <form method='post' action='/login'>\
             <label for='email'>Email address</label>\
             <input id='email' name='email' type='email' autocomplete='username' value='{email}' required>\
             <label for='password'>Password</label>\
             <input id='password' name='password' type='password' autocomplete='current-password' required>\
             <div class='check'><input type='checkbox' id='keep' name='keep' value='on'><label for='keep' style='margin:0'>Keep me signed in</label></div>\
             {}<button type='submit'>Sign in</button></form>\
             <div class='or'>or</div>\
             <button type='button' class='secondary' onclick=\"location.href='{idp}/authorize?client=northwind'\">Continue with Google</button>\
             <p><small>Have a username instead? <a href='/signin'>Sign in with your username</a></small></p>",
            alert(error)
        ),
    )
}

const HOME_TITLE: &str = "Your accounts · Northwind";

fn home(name: &str) -> Html<String> {
    rp(
        HOME_TITLE,
        &format!("<h1>Welcome back, {name}</h1><p>Checking ···· 2231 — balance $4,812.06</p>"),
    )
}

fn otp_page(title: &str, lede: &str, action: &str, error: Option<&str>) -> Html<String> {
    rp(
        title,
        &format!(
            "<h1>{title}</h1><p>{lede}</p><form method='post' action='{action}'>\
             <label for='code'>Verification code</label>\
             <input id='code' name='code' inputmode='numeric' autocomplete='one-time-code' maxlength='6'>\
             {}<button type='submit'>Verify</button></form>",
            alert(error)
        ),
    )
}

fn sms_page(error: Option<&str>) -> Html<String> {
    otp_page(
        "Check your phone for a code",
        "We texted a 6-digit code to the phone number ending in 1234.",
        "/verify/sms",
        error,
    )
}

fn totp_page(error: Option<&str>) -> Html<String> {
    otp_page(
        "Two-step verification",
        "Enter the 6-digit code from your authenticator app.",
        "/verify/totp",
        error,
    )
}

/// The QR code for `text` as inline SVG, one rect per dark module.
fn qr_svg(text: &str) -> String {
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else {
        return String::new();
    };
    let width = code.width();
    let mut rects = String::new();
    for (index, color) in code.to_colors().iter().enumerate() {
        if *color == qrcode::Color::Dark {
            rects.push_str(&format!(
                "<rect x='{}' y='{}' width='1' height='1'/>",
                index % width + 4,
                index / width + 4
            ));
        }
    }
    let size = width + 8;
    format!(
        "<svg role='img' aria-label='QR code' width='176' height='176' viewBox='0 0 {size} {size}' style='display:block;margin:16px auto;background:#fff' shape-rendering='crispEdges'><g fill='#000'>{rects}</g></svg>"
    )
}

fn enroll_page(error: Option<&str>) -> Html<String> {
    let uri = format!(
        "otpauth://totp/Northwind:{}?secret={TOTP_SEED}&issuer=Northwind&digits=6&period=30",
        TESS.0.replace('@', "%40")
    );
    let grouped: Vec<&str> = TOTP_SEED
        .as_bytes()
        .chunks(4)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
        .collect();
    rp(
        "Set up two-step verification · Northwind",
        &format!(
            "<h1>Protect your account with an authenticator app</h1>\
             <p>Scan this QR code with your authenticator app, or enter the setup key manually.</p>\
             <a href='{uri}' aria-label='Open in authenticator app'>{}</a>\
             <p>Setup key: <span class='key'>{}</span></p>\
             <form method='post' action='/security/authenticator'>\
             <label for='code'>Enter the 6-digit code from the app to confirm</label>\
             <input id='code' name='code' inputmode='numeric' autocomplete='one-time-code' maxlength='6'>\
             {}<button type='submit'>Confirm</button></form>\
             <p><small><a href='/security/skip'>Skip for now</a></small></p>",
            qr_svg(&uri),
            grouped.join(" "),
            alert(error)
        ),
    )
}

fn push_page() -> Html<String> {
    rp(
        "Approve sign-in · Northwind",
        "<h1>Check your phone</h1>\
         <p>We sent a sign-in request to the Northwind app on your phone. Tap the number below on your phone to approve the sign-in.</p>\
         <div class='number'>42</div>\
         <p><small>Waiting for your approval…</small></p>\
         <script>setInterval(async () => { const r = await fetch('/push/status'); if ((await r.text()) !== 'pending') location.href = '/push/done'; }, 400);</script>",
    )
}

fn choose_method() -> Html<String> {
    rp(
        "Verify it's you · Northwind",
        "<h1>Choose how to verify</h1><form>\
         <label class='radio'><input type='radio' name='method' value='sms' onchange=\"location.href='/verify/method?m=sms'\">Text message<br><small>To the phone ending in 1234</small></label>\
         <label class='radio'><input type='radio' name='method' value='email' onchange=\"location.href='/verify/method?m=email'\">Email<br><small>To c***@northwind.test</small></label>\
         </form>",
    )
}

fn choose_profile() -> Html<String> {
    rp(
        "Choose a profile · Northwind",
        "<h1>Which account do you want to use?</h1>\
         <ul class='choices'>\
         <li><button type='button' onclick=\"location.href='/profiles/pick?p=personal'\">Cara Lane<br><small>Personal banking</small></button></li>\
         <li><button type='button' onclick=\"location.href='/profiles/pick?p=business'\">Lane Studio LLC<br><small>Business banking</small></button></li>\
         </ul>",
    )
}

fn trust_page() -> Html<String> {
    rp(
        "Trust this device? · Northwind",
        "<h1>Trust this device?</h1><p>You won't be asked for a code on this browser for 30 days.</p>\
         <form method='post' action='/trust'>\
         <button type='submit' name='trust' value='yes'>Trust this device</button>\
         <button type='submit' name='trust' value='no' class='secondary'>Not now</button></form>",
    )
}

fn terms_page() -> Html<String> {
    rp(
        "Updated terms · Northwind",
        "<h1>Please accept the updated terms</h1><p>We updated our terms of service and privacy notice on 1 October.</p>\
         <form method='post' action='/terms'>\
         <button type='submit' name='accept' value='yes'>Accept and continue</button>\
         <button type='submit' name='accept' value='no' class='secondary'>Decline</button></form>",
    )
}

fn idp(title: &str, body: &str) -> Html<String> {
    page("Google", title, body)
}

async fn no_store(mut response: Response) -> Response {
    // Authenticator setup keys must not enter Chromium's shared-profile disk cache.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

impl LoginFixture {
    /// Binds both sites on free loopback ports and serves them until dropped.
    pub async fn start() -> Self {
        let site = Shared::default();
        let bind = || async {
            tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap_or_else(|e| panic!("login fixture: bind loopback: {e}"))
        };
        let (rp_listener, idp_listener) = (bind().await, bind().await);
        let port = |listener: &tokio::net::TcpListener| {
            listener
                .local_addr()
                .unwrap_or_else(|e| panic!("login fixture: local address: {e}"))
                .port()
        };
        // Vault allowlists accept DNS origins, not IP literals; the provider lives on another
        // host name so the two are distinct sites.
        let origin = format!("http://localhost:{}", port(&rp_listener));
        let idp_origin = format!("http://127.0.0.1:{}", port(&idp_listener));
        let rp = relying_party(site.clone(), idp_origin.clone())
            .layer(axum::middleware::map_response(no_store));
        let provider =
            identity_provider(origin.clone()).layer(axum::middleware::map_response(no_store));
        let tasks = vec![
            tokio::spawn(async move {
                axum::serve(rp_listener, rp)
                    .await
                    .unwrap_or_else(|e| panic!("login fixture: relying party: {e}"));
            }),
            tokio::spawn(async move {
                axum::serve(idp_listener, provider)
                    .await
                    .unwrap_or_else(|e| panic!("login fixture: identity provider: {e}"));
            }),
        ];
        Self {
            origin,
            idp_origin,
            site,
            tasks,
        }
    }

    /// The fixture state, as `GET /__fixture/state` returns it.
    pub fn state(&self) -> Value {
        state_json(&self.site.lock())
    }

    /// Every form post (path, fields) the site received, in order.
    pub fn posts(&self) -> Vec<(String, HashMap<String, String>)> {
        self.site.lock().posts.clone()
    }

    /// Waits until `check` holds on the state, for at most 15 s.
    pub async fn until(&self, what: &str, check: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..300 {
            let state = self.state();
            if check(&state) {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("fixture never reached {what}: {}", self.state());
    }
}

fn state_json(site: &Site) -> Value {
    json!({
        "signed_in": site.signed_in.as_ref().map(|(user, _)| user),
        "via": site.signed_in.as_ref().map(|(_, via)| via),
        "pending": site.pending,
        "push": site.push.wire(),
        "enrolled": site.enrolled,
        "method": site.method,
        "profile": site.profile,
        "trusted": site.trusted,
        "terms": site.terms,
        "oauth_consent": site.oauth_consent.as_ref().map(|(account, allowed)| json!({"account":account,"allowed":allowed})),
        "codes": site.codes.iter().map(|(kind, ok)| json!({"kind":kind,"ok":ok})).collect::<Vec<_>>(),
        "posts": site.posts.len(),
    })
}

fn sign_in(site: &mut Site, user: &str, via: &str) -> Response {
    site.pending = None;
    site.signed_in = Some((user.to_owned(), via.to_owned()));
    Redirect::to("/account").into_response()
}

fn relying_party(site: Shared, idp: String) -> Router {
    let login_idp = idp.clone();
    let error_idp = idp;
    Router::new()
        .route("/", get(|| async { Redirect::to("/login") }))
        .route("/login/slow-error", get(|| async {
            rp("Sign in · Northwind", r#"
                <h1>Sign in</h1>
                <form onsubmit="event.preventDefault();setTimeout(()=>{document.getElementById('delayed-error').textContent='The sign-in was rejected.'},4000);return false">
                <label for="email">Email address</label><input id="email" name="email" type="email" autocomplete="username" required>
                <label for="password">Password</label><input id="password" name="password" type="password" autocomplete="current-password" required>
                <button type="submit">Sign in</button><div id="delayed-error" role="alert"></div></form>
            "#)
        }))
        .route("/security/readonly-key", get(|| async {
            rp("Authenticator setup · Northwind", &format!(
                "<h1>Authenticator setup</h1><label for='setup-key'>Setup key</label><input id='setup-key' readonly value='{TOTP_SEED}'>"
            ))
        }))
        .route(
            "/login",
            get(move || {
                let idp = login_idp.clone();
                async move { login_page(&idp, None, "") }
            })
            .post(
                move |State(site): State<Shared>, Form(form): Form<HashMap<String, String>>| {
                    let idp = error_idp.clone();
                    async move {
                        let mut site = site.lock();
                        site.posts.push(("/login".into(), form.clone()));
                        let email = form.get("email").cloned().unwrap_or_default();
                        let password = form.get("password").cloned().unwrap_or_default();
                        let pair = (email.as_str(), password.as_str());
                        if pair == ALICE {
                            return sign_in(&mut site, ALICE.0, "password");
                        }
                        let next = if pair == PAT {
                            site.push = PushState::Pending;
                            "/push"
                        } else if pair == SAM {
                            "/verify/sms"
                        } else if pair == TOM {
                            "/verify/totp"
                        } else if pair == TESS {
                            if site.enrolled {
                                "/verify/totp"
                            } else {
                                "/security/authenticator"
                            }
                        } else if pair == CARA {
                            "/verify/method"
                        } else {
                            return login_page(
                                &idp,
                                Some("Incorrect email or password. Check them and try again."),
                                &email,
                            )
                            .into_response();
                        };
                        site.pending = Some(email);
                        Redirect::to(next).into_response()
                    }
                },
            ),
        )
        .route(
            "/signin",
            get(|| async { username_page(None) }).post(
                |State(site): State<Shared>, Form(form): Form<HashMap<String, String>>| async move {
                    let mut site = site.lock();
                    site.posts.push(("/signin".into(), form.clone()));
                    if form.get("username").map(String::as_str) == Some(BOB.0) {
                        site.pending = Some(BOB.0.into());
                        Redirect::to("/signin/password").into_response()
                    } else {
                        username_page(Some("We couldn't find an account with that username."))
                            .into_response()
                    }
                },
            ),
        )
        .route(
            "/signin/password",
            get(|State(site): State<Shared>| async move {
                match site.lock().pending.clone() {
                    Some(user) => password_page(&user, None).into_response(),
                    None => Redirect::to("/signin").into_response(),
                }
            })
            .post(
                |State(site): State<Shared>, Form(form): Form<HashMap<String, String>>| async move {
                    let mut site = site.lock();
                    site.posts.push(("/signin/password".into(), form.clone()));
                    let ok = site.pending.as_deref() == Some(BOB.0)
                        && form.get("password").map(String::as_str) == Some(BOB.1);
                    if ok {
                        sign_in(&mut site, BOB.0, "password")
                    } else {
                        password_page(BOB.0, Some("Wrong password. Try again.")).into_response()
                    }
                },
            ),
        )
        .route(
            "/account",
            get(|State(site): State<Shared>| async move {
                match site.lock().signed_in.clone() {
                    Some((user, _)) => home(user.split('@').next().unwrap_or(&user)).into_response(),
                    None => Redirect::to("/login").into_response(),
                }
            }),
        )
        .route("/push", get(|| async { push_page() }))
        .route(
            "/push/status",
            get(|State(site): State<Shared>| async move { site.lock().push.wire() }),
        )
        .route(
            "/push/done",
            get(|State(site): State<Shared>| async move {
                let mut site = site.lock();
                match site.push {
                    PushState::Approved => {
                        let user = site.pending.clone().unwrap_or_default();
                        sign_in(&mut site, &user, "push")
                    }
                    PushState::Denied => rp(
                        "Sign-in denied · Northwind",
                        "<h1>Sign-in not approved</h1><div role='alert'>The sign-in request was denied on your phone.</div><p><a href='/login'>Back to sign in</a></p>",
                    )
                    .into_response(),
                    PushState::Expired => rp(
                        "Request expired · Northwind",
                        "<h1>Request expired</h1><div role='alert'>The sign-in request expired before it was approved.</div><p><a href='/login'>Back to sign in</a></p>",
                    )
                    .into_response(),
                    PushState::Pending | PushState::Idle => Redirect::to("/push").into_response(),
                }
            }),
        )
        .route(
            "/verify/sms",
            get(|| async { sms_page(None) }).post(
                |State(site): State<Shared>, Form(form): Form<HashMap<String, String>>| async move {
                    let mut site = site.lock();
                    site.posts.push(("/verify/sms".into(), form.clone()));
                    let ok = form.get("code").map(String::as_str) == Some(SMS_CODE);
                    site.codes.push(("sms".into(), ok));
                    if !ok {
                        return sms_page(Some("That code didn't match. Check the text and try again.")).into_response();
                    }
                    let user = site.pending.clone().unwrap_or_default();
                    if user == CARA.0 {
                        return Redirect::to("/profiles").into_response();
                    }
                    sign_in(&mut site, &user, "sms")
                },
            ),
        )
        .route(
            "/verify/totp",
            get(|| async { totp_page(None) }).post(
                |State(site): State<Shared>, Form(form): Form<HashMap<String, String>>| async move {
                    let mut site = site.lock();
                    site.posts.push(("/verify/totp".into(), form.clone()));
                    let ok = form.get("code").is_some_and(|code| totp_ok(code));
                    site.codes.push(("totp".into(), ok));
                    if !ok {
                        return totp_page(Some("That code didn't work. Wait for a new code and try again.")).into_response();
                    }
                    let user = site.pending.clone().unwrap_or_default();
                    sign_in(&mut site, &user, "totp")
                },
            ),
        )
        .route(
            "/security/authenticator",
            get(|| async { enroll_page(None) }).post(
                |State(site): State<Shared>, Form(form): Form<HashMap<String, String>>| async move {
                    let mut site = site.lock();
                    site.posts.push(("/security/authenticator".into(), form.clone()));
                    let ok = form.get("code").is_some_and(|code| totp_ok(code));
                    site.codes.push(("enroll".into(), ok));
                    if !ok {
                        return enroll_page(Some("That code didn't work. Check the app and try again.")).into_response();
                    }
                    site.enrolled = true;
                    let user = site.pending.clone().unwrap_or_default();
                    sign_in(&mut site, &user, "password+enrolled")
                },
            ),
        )
        .route(
            "/security/skip",
            get(|State(site): State<Shared>| async move {
                let mut site = site.lock();
                let user = site.pending.clone().unwrap_or_default();
                sign_in(&mut site, &user, "password")
            }),
        )
        .route("/verify/method", get(
            |State(site): State<Shared>, Query(query): Query<HashMap<String, String>>| async move {
                match query.get("m") {
                    Some(method) => {
                        site.lock().method = Some(method.clone());
                        Redirect::to("/verify/sms").into_response()
                    }
                    None => choose_method().into_response(),
                }
            },
        ))
        .route("/profiles", get(|| async { choose_profile() }))
        .route(
            "/profiles/pick",
            get(|State(site): State<Shared>, Query(query): Query<HashMap<String, String>>| async move {
                site.lock().profile = query.get("p").cloned();
                Redirect::to("/trust")
            }),
        )
        .route(
            "/trust",
            get(|| async { trust_page() }).post(
                |State(site): State<Shared>, Form(form): Form<HashMap<String, String>>| async move {
                    let mut site = site.lock();
                    site.posts.push(("/trust".into(), form.clone()));
                    site.trusted = Some(form.get("trust").map(String::as_str) == Some("yes"));
                    Redirect::to("/terms")
                },
            ),
        )
        .route(
            "/terms",
            get(|| async { terms_page() }).post(
                |State(site): State<Shared>, Form(form): Form<HashMap<String, String>>| async move {
                    let mut site = site.lock();
                    site.posts.push(("/terms".into(), form.clone()));
                    let accepted = form.get("accept").map(String::as_str) == Some("yes");
                    site.terms = Some(accepted);
                    if !accepted {
                        return rp("Terms declined · Northwind", "<h1>You're signed out</h1><div role='alert'>You need to accept the updated terms to use online banking.</div>").into_response();
                    }
                    let user = site.pending.clone().unwrap_or_default();
                    sign_in(&mut site, &user, "sms")
                },
            ),
        )
        .route(
            "/oauth/callback",
            get(|State(site): State<Shared>, Query(query): Query<HashMap<String, String>>| async move {
                let mut site = site.lock();
                let account = query.get("account").cloned().unwrap_or_default();
                let allowed = query.get("allowed").map(String::as_str) == Some("yes");
                site.oauth_consent = Some((account.clone(), allowed));
                if !allowed {
                    return rp("Sign-in cancelled · Northwind", "<h1>Sign-in cancelled</h1><div role='alert'>Google sign-in was cancelled.</div>").into_response();
                }
                sign_in(&mut site, &account, &format!("oauth:{account}"))
            }),
        )
        .route(
            "/__fixture/state",
            get(|State(site): State<Shared>| async move { Json(state_json(&site.lock())) }),
        )
        .route(
            "/__fixture/push/{verdict}",
            post(
                |State(site): State<Shared>, axum::extract::Path(verdict): axum::extract::Path<String>| async move {
                    let mut site = site.lock();
                    site.push = match verdict.as_str() {
                        "approve" => PushState::Approved,
                        "deny" => PushState::Denied,
                        _ => PushState::Expired,
                    };
                    Json(state_json(&site))
                },
            ),
        )
        .route(
            "/__fixture/sms",
            get(|| async { ([(header::CACHE_CONTROL, "no-store")], SMS_CODE) }),
        )
        .route(
            "/__fixture/logout",
            post(|State(site): State<Shared>| async move {
                let mut site = site.lock();
                site.signed_in = None;
                site.pending = None;
                site.push = PushState::Idle;
                Json(state_json(&site))
            }),
        )
        .with_state(site)
}

fn username_page(error: Option<&str>) -> Html<String> {
    rp(
        "Sign in · Northwind",
        &format!(
            "<h1>Sign in</h1><p>Enter your Northwind username to continue.</p>\
             <form method='post' action='/signin'>\
             <label for='username'>Username</label>\
             <input id='username' name='username' type='text' autocomplete='username' required>\
             {}<button type='submit'>Next</button></form>",
            alert(error)
        ),
    )
}

fn password_page(user: &str, error: Option<&str>) -> Html<String> {
    rp(
        "Enter your password · Northwind",
        &format!(
            "<h1>Hi {user}</h1><p>Enter your password to finish signing in.</p>\
             <form method='post' action='/signin/password'>\
             <input type='hidden' name='username' value='{user}'>\
             <label for='password'>Password</label>\
             <input id='password' name='password' type='password' autocomplete='current-password' required>\
             {}<button type='submit'>Sign in</button></form>",
            alert(error)
        ),
    )
}

fn identity_provider(rp_origin: String) -> Router {
    let consent_rp = rp_origin;
    Router::new()
        .route(
            "/authorize",
            get(|| async {
                let rows: String = IDP_ACCOUNTS
                    .iter()
                    .map(|account| {
                        let name = if account.starts_with("alice@") { "Alice Kim" } else { "Alice Kim (Work)" };
                        format!(
                            "<li><button type='button' onclick=\"location.href='/consent?account={account}'\">{name}<br><small>{account}</small></button></li>"
                        )
                    })
                    .collect();
                idp(
                    "Sign in – Google Accounts",
                    &format!("<h1>Choose an account</h1><p>to continue to Northwind</p><ul class='choices'>{rows}</ul>"),
                )
            }),
        )
        .route(
            "/consent",
            get(move |Query(query): Query<HashMap<String, String>>| {
                let rp = consent_rp.clone();
                async move {
                    let account = query.get("account").cloned().unwrap_or_default();
                    idp(
                        "Allow access – Google Accounts",
                        &format!(
                            "<h1>Northwind wants to access your Google Account</h1><p>{account}</p>\
                             <p>This will allow Northwind to see your name, email address and profile picture.</p>\
                             <form method='get' action='{rp}/oauth/callback'><input type='hidden' name='account' value='{account}'>\
                             <button type='submit' name='allowed' value='yes'>Allow</button>\
                             <button type='submit' name='allowed' value='no' class='secondary'>Cancel</button></form>"
                        ),
                    )
                }
            }),
        )
}
