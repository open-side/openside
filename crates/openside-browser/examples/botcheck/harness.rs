use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::header;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use openside_browser::{Browser, BrowserConfig, GatewayConfig};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::task::JoinHandle;

pub type Result<T, E = Box<dyn std::error::Error + Send + Sync>> = std::result::Result<T, E>;

const OWNER: &str = "opensided";
const P7: &[&str] = &[
    "navigator.webdriver",
    "user_agent.headless",
    "client_hints.headless",
    "cdp.runtime_console_getter",
    "browser.automation_infobar",
];
const LOCAL_IDS: &[&str] = &[
    "navigator.webdriver",
    "user_agent.headless",
    "client_hints.headless",
    "window.chrome",
    "navigator.plugins",
    "navigator.mime_types",
    "navigator.languages",
    "permissions.notifications",
    "webgl.identity_present",
    "window.dimensions",
    "window.automation_globals",
    "iframe.content_window",
    "native.to_string_integrity",
    "cdp.runtime_console_getter",
];

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    StockCdp,
    PatchedGateway,
    NegativeStockAutomation,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Check {
    pub id: String,
    pub passed: bool,
    pub observed: Value,
}

#[derive(Debug, Serialize)]
pub struct Score {
    pub passed: usize,
    pub total: usize,
    pub ratio: f64,
}

impl Score {
    pub fn from_checks(checks: &[Check]) -> Self {
        let passed = checks.iter().filter(|check| check.passed).count();
        let total = checks.len();
        Self {
            passed,
            total,
            ratio: passed as f64 / total.max(1) as f64,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Threshold {
    pub gated: bool,
    pub passed: bool,
    pub required_check_ids: Vec<String>,
    pub not_observed: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Record {
    pub browser_version: String,
    pub mode: Mode,
    pub checks: Vec<Check>,
    pub score: Score,
    pub threshold: Threshold,
    pub runtime_enable_sent: bool,
    pub public_pages: Vec<PublicResult>,
}

#[derive(Debug, Serialize)]
pub struct Run {
    pub schema_version: u32,
    pub suite: String,
    pub recorded_at_unix_ms: u128,
    pub records: Vec<Record>,
    pub skipped: Vec<String>,
}

impl Run {
    pub fn new() -> Self {
        Self {
            schema_version: 1,
            suite: "openside-local-botcheck".into(),
            recorded_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            records: Vec::new(),
            skipped: Vec::new(),
        }
    }
}

pub fn stock_browser() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for name in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "chrome",
    ] {
        for directory in std::env::split_paths(&paths) {
            let path = directory.join(name);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

pub fn patched_browser() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("OPENSIDE_PATCHED_BROWSER")?);
    path.is_file().then_some(path)
}

pub struct FixtureServer {
    pub origin: String,
    task: JoinHandle<()>,
}

impl FixtureServer {
    pub async fn start() -> Result<Self> {
        let policy =
            "default-src 'self'; script-src 'unsafe-inline'; frame-src 'self'; connect-src 'none'";
        let app = Router::new()
            .route("/classic", get(|| async { Html(include_str!("../../../../bench/botcheck/pages/classic.html")) }))
            .route("/runtime", get(|| async { Html(include_str!("../../../../bench/botcheck/pages/runtime.html")) }))
            .route("/integrity", get(|| async { Html(include_str!("../../../../bench/botcheck/pages/integrity.html")) }))
            .route("/public-table", get(|| async { Html("<!doctype html><title>Local public extractor fixture</title><table id='checks'><tr><td>clean</td><td class='passed'>OK</td></tr><tr><td>leak</td><td class='failed'>FAIL</td></tr></table>") }))
            .route("/public-failure-only", get(|| async { Html("<!doctype html><title>Local failure-only table</title><table id='checks'><tr><td>clean</td><td>present</td></tr><tr><td>leak</td><td class='failed'>missing</td></tr></table>") }))
            .route("/public-pending", get(|| async { Html("<!doctype html><title>Unfinished table</title><table id='checks'><tr><td>unfinished</td><td></td></tr></table>") }))
            .route("/public-score", get(|| async { Html("<!doctype html><title>Local numeric extractor fixture</title><span id='passed'>7</span><span id='total'>10</span>") }))
            .layer(axum::middleware::map_response(move |mut response: axum::response::Response| async move {
                response.headers_mut().insert(header::CONTENT_SECURITY_POLICY, header::HeaderValue::from_static(policy));
                response
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let task = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, app).await {
                eprintln!("botcheck fixture server: {error}");
            }
        });
        Ok(Self { origin, task })
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn launch(executable: &Path, mode: Mode) -> Result<(Browser, tempfile::TempDir)> {
    let runtime = tempfile::Builder::new()
        .prefix("botcheck-runtime-")
        .tempdir()?;
    let mut config = BrowserConfig::new(executable);
    if mode == Mode::PatchedGateway {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700))?;
        }
        config.gateway = Some(GatewayConfig {
            runtime_dir: Some(runtime.path().to_owned()),
            ..GatewayConfig::new(OWNER)
        });
    }
    let (browser, _events) = Browser::launch(config).await?;
    if (mode == Mode::PatchedGateway) != browser.gateway().is_some() {
        browser.shutdown().await?;
        return Err("browser controller mode did not match requested mode".into());
    }
    Ok((browser, runtime))
}

async fn await_value(
    browser: &Browser,
    tab: &str,
    expression: &str,
    timeout: Duration,
) -> Result<Value> {
    tokio::time::timeout(timeout, async {
        let mut interval = tokio::time::interval(Duration::from_millis(25));
        loop {
            interval.tick().await;
            let value = browser.evaluate(tab, expression).await?;
            if !value.is_null() {
                return Ok(value);
            }
        }
    })
    .await
    .map_err(|_| "botcheck page did not produce results before its deadline")?
}

async fn local_checks(
    browser: &Browser,
    server: &FixtureServer,
) -> Result<(Vec<Check>, Vec<String>)> {
    let mut checks = Vec::new();
    let mut live_tabs = Vec::new();
    for page in ["classic", "integrity", "runtime"] {
        let tab = browser
            .create_agent_tab(OWNER, &format!("{}/{page}", server.origin))
            .await?;
        browser.snapshot(&tab.id).await?;
        let value = await_value(
            browser,
            &tab.id,
            "window.__botcheck_results ?? null",
            Duration::from_secs(15),
        )
        .await?;
        checks.extend(serde_json::from_value::<Vec<Check>>(value)?);
        live_tabs.push(tab.id);
    }
    let found: BTreeSet<_> = checks.iter().map(|check| check.id.as_str()).collect();
    let expected: BTreeSet<_> = LOCAL_IDS.iter().copied().collect();
    if found != expected || checks.len() != expected.len() {
        return Err("local fixture returned missing, duplicate, or unknown check IDs".into());
    }
    Ok((checks, live_tabs))
}

pub async fn record(executable: &Path, mode: Mode, public: &[PublicPage]) -> Result<Record> {
    let version = tokio::process::Command::new(executable)
        .arg("--version")
        .output()
        .await?;
    if !version.status.success() {
        return Err("browser --version failed".into());
    }
    let browser_version = String::from_utf8(version.stdout)?.trim().to_owned();
    let server = FixtureServer::start().await?;
    let (browser, _runtime) = launch(executable, mode).await?;
    browser.record_protocol(true);
    let result = async {
        let (mut checks, live_tabs) = local_checks(&browser, &server).await?;
        let runtime_enable_sent = browser.take_protocol_log().iter().any(|(method, _)| method == "Runtime.enable");
        let runtime_check = checks.iter_mut().find(|check| check.id == "cdp.runtime_console_getter").ok_or("missing console leak check")?;
        runtime_check.observed["runtime_enable_sent"] = json!(runtime_enable_sent);
        // New Chromium versions may stop previewing Error.stack; the protocol observation
        // prevents that change from turning a leaky controller into a passing P7 result.
        runtime_check.passed &= !runtime_enable_sent;
        let mut not_observed = Vec::new();
        if let Some(gateway) = browser.gateway() {
            let observed = match gateway.request("browser.status", json!({})).await {
                Ok(status) => status,
                Err(error) => json!({"error": error.to_string()}),
            };
            checks.push(Check {
                id: "browser.automation_infobar".into(),
                passed: observed["automation_infobar_count"].as_u64() == Some(0),
                observed,
            });
        } else {
            not_observed.push("browser.automation_infobar: native browser UI is not page-observable; stock CDP provides no native count".into());
        }
        // Observe native UI before closing the fixture windows; closing them would hide
        // per-window infobars and make a zero count meaningless.
        for tab in live_tabs {
            browser.close_agent_tab(OWNER, &tab).await?;
        }
        let passed = P7.iter().all(|id| checks.iter().any(|check| check.id == *id && check.passed));
        let threshold = Threshold {
            gated: mode == Mode::PatchedGateway,
            passed,
            required_check_ids: P7.iter().map(|id| (*id).into()).collect(),
            not_observed,
        };
        let mut public_pages = Vec::new();
        for page in public {
            public_pages.push(extract_public(&browser, page).await);
        }
        let score = Score::from_checks(&checks);
        Ok(Record { browser_version, mode, checks, score, threshold, runtime_enable_sent, public_pages })
    }.await;
    let shutdown = browser.shutdown().await;
    shutdown?;
    result
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicPage {
    pub id: String,
    pub url: String,
    pub extractor: Extractor,
    #[serde(default = "public_timeout")]
    pub timeout_seconds: u64,
}

fn public_timeout() -> u64 {
    30
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Extractor {
    Table {
        row_selector: String,
        result_selector: String,
        pass_classes: Vec<String>,
        fail_classes: Vec<String>,
        #[serde(default)]
        pass_when_no_fail_class: bool,
    },
    Numeric {
        passed_selector: String,
        total_selector: String,
    },
}

#[derive(Debug, Serialize)]
pub struct PublicResult {
    pub id: String,
    pub url: String,
    pub extractor: Value,
    pub status: String,
    pub observed: Value,
    pub score: Option<Value>,
}

pub fn read_public_manifest(path: &Path) -> Result<Vec<PublicPage>> {
    let pages: Vec<PublicPage> = serde_json::from_slice(&std::fs::read(path)?)?;
    let mut ids = BTreeSet::new();
    if pages.is_empty() {
        return Err("--public-urls must contain at least one page".into());
    }
    for page in &pages {
        if page.id.is_empty()
            || !ids.insert(&page.id)
            || !page.url.starts_with("https://")
            || page.timeout_seconds == 0
            || page.timeout_seconds > 300
        {
            return Err("public pages require unique nonempty IDs, HTTPS URLs, and a timeout in 1..300 seconds".into());
        }
        if let Extractor::Table {
            pass_classes,
            fail_classes,
            pass_when_no_fail_class,
            ..
        } = &page.extractor
        {
            if (pass_classes.is_empty() && !pass_when_no_fail_class)
                || fail_classes.is_empty()
                || pass_classes
                    .iter()
                    .any(|class| fail_classes.contains(class))
            {
                return Err("table extractors require fail_classes and either disjoint pass_classes or an explicit pass_when_no_fail_class convention".into());
            }
        }
    }
    Ok(pages)
}

pub async fn extract_public(browser: &Browser, page: &PublicPage) -> PublicResult {
    let extractor = serde_json::to_value(&page.extractor).unwrap_or(Value::Null);
    let result = async {
        let tab = browser.create_agent_tab(OWNER, &page.url).await?;
        let expression = format!(r#"(() => {{
          if (document.readyState !== 'complete') return null;
          const spec = {extractor};
          if (spec.kind === 'numeric') {{
            const passed = document.querySelector(spec.passed_selector)?.textContent.trim();
            const total = document.querySelector(spec.total_selector)?.textContent.trim();
            if (!passed || !total) return null;
            const p = Number(passed), t = Number(total);
            if (!Number.isFinite(p) || !Number.isFinite(t) || t <= 0 || p < 0 || p > t) return null;
            return {{observed: {{passed_text: passed, total_text: total}}, score: {{passed: p, total: t, ratio: p / t}}}};
          }}
          const rows = Array.from(document.querySelectorAll(spec.row_selector));
          if (!rows.length) return null;
          const observed = rows.map(row => {{
            const cell = row.querySelector(spec.result_selector);
            const classes = cell ? Array.from(cell.classList) : [];
            const text = cell?.textContent.trim() ?? '';
            const fail = spec.fail_classes.some(name => classes.includes(name));
            const pass = spec.pass_classes.some(name => classes.includes(name)) || (spec.pass_when_no_fail_class && !fail && text.length > 0);
            return {{label: row.cells?.[0]?.textContent.trim() ?? '', text, classes, passed: !text ? null : pass && !fail ? true : fail && !pass ? false : null}};
          }});
          if (observed.some(row => row.passed === null)) return null;
          const passed = observed.filter(row => row.passed).length;
          return {{observed, score: {{passed, total: observed.length, ratio: passed / observed.length}}}};
        }})()"#);
        let extracted = await_value(browser, &tab.id, &expression, Duration::from_secs(page.timeout_seconds)).await;
        browser.close_agent_tab(OWNER, &tab.id).await?;
        extracted
    }.await;
    match result {
        Ok(value) => PublicResult {
            id: page.id.clone(),
            url: page.url.clone(),
            extractor,
            status: "scored".into(),
            observed: value["observed"].clone(),
            score: Some(value["score"].clone()),
        },
        Err(error) => PublicResult {
            id: page.id.clone(),
            url: page.url.clone(),
            extractor,
            status: "extraction_error".into(),
            observed: json!({"error":error.to_string()}),
            score: None,
        },
    }
}

#[cfg(unix)]
pub fn leaky_wrapper(executable: &Path, directory: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let executable = std::fs::canonicalize(executable)?;
    let quoted = executable
        .to_str()
        .ok_or("browser path is not UTF-8")?
        .replace('\'', "'\\''");
    let wrapper = directory.join("leaky-chromium");
    std::fs::write(
        &wrapper,
        format!("#!/bin/sh\nexec '{quoted}' --enable-automation \"$@\"\n"),
    )?;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700))?;
    Ok(wrapper)
}
