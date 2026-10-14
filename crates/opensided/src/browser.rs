use std::path::PathBuf;

use openside_browser::{BrowserConfig, BrowserEvent, GatewayConfig};
use openside_config::BrowserGatewaySetting;
use openside_proto::EventKind;
use serde_json::json;

/// Converts a browser observation to a credential-free public event payload.
/// URL credentials, query parameters and fragments are never included.
pub fn browser_event(event: BrowserEvent) -> EventKind {
    match event {
        BrowserEvent::OriginCommitted { .. } => EventKind::runtime_error(
            "browser_internal_event",
            "Internal browser observation cannot be published",
        ),
        BrowserEvent::TabOpened { tab } | BrowserEvent::PopupOpened { tab, .. } => {
            EventKind::TabOpened {
                tab_id: tab.id,
                url: public_url(&tab.url),
            }
        }
        BrowserEvent::TabClosed { tab } => EventKind::TabClosed { tab_id: tab.id },
        BrowserEvent::DownloadStarted {
            guid,
            url,
            suggested_filename: _,
            tab_id,
        } => EventKind::ToolUpdated {
            tool_call_id: guid,
            update: json!({"browser_event":"download.started","url":public_url(&url),"tab_id":tab_id}),
        },
        BrowserEvent::DownloadFinished { guid, status } => EventKind::ToolEnded {
            card: None,
            tool_call_id: guid,
            result: json!({"browser_event":"download.finished"}),
            is_error: status == openside_browser::DownloadStatus::Canceled,
        },
        BrowserEvent::DialogOpened {
            tab_id,
            kind,
            message: _,
        } => EventKind::ToolUpdated {
            tool_call_id: tab_id,
            update: json!({"browser_event":"dialog.opened","dialog_kind":kind}),
        },
        BrowserEvent::TooManyTabs { count } => {
            EventKind::runtime_error("browser_tab_limit", format!("{count} live browser tabs"))
        }
    }
}

/// The daemon shares one browser across sessions, while the patched browser binds a single
/// launch scope per process. The gateway scope is therefore the daemon itself: browser-side
/// `owner:agent` means "opened by this daemon", and per-session ownership stays in the CDP
/// ledger. `runtime_dir` None uses `XDG_RUNTIME_DIR`.
pub(crate) const GATEWAY_SCOPE: &str = "opensided";

/// Launch settings for the daemon's detached browser. `auto` never fails a launch for lack
/// of a gateway: a stock Chromium, or a missing private runtime directory, runs CDP-only.
pub(crate) fn launch_config(
    executable: PathBuf,
    profile_dir: PathBuf,
    gateway: BrowserGatewaySetting,
    runtime_dir: Option<PathBuf>,
) -> BrowserConfig {
    let mut config = BrowserConfig::new(executable);
    config.profile_dir = Some(profile_dir);
    config.detached = true;
    // Production: a detached browser must survive this daemon's exit for reconnects. In the
    // crate's own tests, though, no relaunched daemon ever adopts it, so it is tied to the
    // spawning thread instead; a test process killed without unwinding leaves no Chromium.
    #[cfg(test)]
    {
        config.die_with_spawner = true;
    }
    config.gateway = match gateway {
        BrowserGatewaySetting::Auto => Some(GatewayConfig {
            runtime_dir,
            required: false,
            ..GatewayConfig::new(GATEWAY_SCOPE)
        }),
        BrowserGatewaySetting::Off => None,
    };
    config
}
pub(crate) fn public_url(value: &str) -> String {
    match url::Url::parse(value) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        Err(_) => String::new(),
    }
}

struct DisplayFacetRule {
    host: &'static str,
    paths: &'static [&'static str],
    key: &'static str,
    values: &'static [&'static str],
    default: &'static str,
}

const DISPLAY_FACETS: &[DisplayFacetRule] = &[DisplayFacetRule {
    host: "github.com",
    paths: &["/trending", "/trending/developers"],
    key: "since",
    values: &["daily", "weekly", "monthly"],
    default: "daily",
}];

fn display_facet(url: &url::Url) -> Option<(&'static str, &'static str)> {
    if url.scheme() != "https" || url.port().is_some() {
        return None;
    }
    let rule = DISPLAY_FACETS
        .iter()
        .find(|rule| url.host_str() == Some(rule.host) && rule.paths.contains(&url.path()))?;
    let mut pairs = url.query_pairs().filter(|(key, _)| key == rule.key);
    let value = match pairs.next() {
        Some((_, value)) => rule
            .values
            .iter()
            .copied()
            .find(|allowed| *allowed == value)?,
        None => rule.default,
    };
    // Repeated keys have site-dependent semantics, so they cannot identify a safe facet.
    if pairs.next().is_some() {
        return None;
    }
    Some((rule.key, value))
}

/// Presentation and history may distinguish known page variants without restoring raw queries.
pub(crate) fn display_url(value: &str) -> String {
    let Ok(mut url) = url::Url::parse(value) else {
        return String::new();
    };
    let facet = display_facet(&url);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    if let Some((key, value)) = facet {
        url.query_pairs_mut().append_pair(key, value);
    }
    url.to_string()
}

/// Nobody can answer a JavaScript dialog while the page's input call is blocked on it, and the
/// dialog text is untrusted, so the answer is fixed: an alert or a leave-page prompt is
/// acknowledged, while a confirm or prompt is cancelled rather than agreed to on the user's behalf.
pub(crate) fn accepts_dialog(kind: &str) -> bool {
    matches!(kind, "alert" | "beforeunload")
}

/// Steer text for browser activity the agent did not ask for but must react to (B-2): a popup,
/// a download, a tab closing. Only the origin of page-chosen URLs is included: the text is
/// injected as a user-role message, so page-controlled paths or file names must not reach it.
pub(crate) fn steer_text(event: &BrowserEvent) -> Option<String> {
    let origin =
        |url: &str| openside_policy::origin_of(url).unwrap_or_else(|| "an unknown origin".into());
    Some(match event {
        BrowserEvent::PopupOpened { tab, opener_id } => {
            let access = if tab.browser_context_id.is_some() {
                "This popup belongs to your isolated space; use its tab id with the ordinary browser tools, subject to the same policy and approval checks."
            } else {
                "Popup tabs outside your isolated space are read-only for you; finish the task in your own tab or ask the user."
            };
            format!(
                "[browser event] The page in tab {opener_id} opened a popup tab {} at {}. {access}",
                tab.id,
                origin(&tab.url)
            )
        }
        BrowserEvent::TabClosed { tab } => format!(
            "[browser event] Tab {} was closed; its refs are gone.",
            tab.id
        ),
        BrowserEvent::DownloadStarted {
            guid, url, tab_id, ..
        } => format!(
            "[browser event] Download {guid} started from {}{}.",
            origin(url),
            tab_id
                .as_deref()
                .map(|tab| format!(" in tab {tab}"))
                .unwrap_or_default()
        ),
        BrowserEvent::DownloadFinished { guid, status } => format!(
            "[browser event] Download {guid} {}.",
            match status {
                openside_browser::DownloadStatus::Completed => "finished",
                openside_browser::DownloadStatus::Canceled => "was canceled or failed",
            }
        ),
        BrowserEvent::DialogOpened { tab_id, kind, .. } => format!(
            "[browser event] The page in tab {tab_id} opened a JavaScript {kind} dialog; openside {} it automatically. The dialog text is not shown to you.",
            if accepts_dialog(kind) {
                "accepted"
            } else {
                "cancelled"
            }
        ),
        BrowserEvent::TabOpened { .. }
        | BrowserEvent::TooManyTabs { .. }
        | BrowserEvent::OriginCommitted { .. } => return None,
    })
}

/// Inject `text` on the session's steer lane at its next model request, if a run is in flight.
/// Best effort: a failed notice never disturbs the browser event pipeline.
pub(crate) async fn steer_notice(
    shared: &crate::engine::Shared,
    session: &openside_proto::SessionId,
    text: String,
) {
    let _ = shared
        .command(session, crate::engine::Action::BrowserNotice(text))
        .await;
}

/// [`steer_notice`] for the browser events in [`steer_text`]; other events are ignored.
pub(crate) async fn steer_browser_event(
    shared: &crate::engine::Shared,
    session: &openside_proto::SessionId,
    event: &BrowserEvent,
) {
    if let Some(text) = steer_text(event) {
        if let Some(task) = shared
            .registry
            .browser
            .task_tabs
            .event_owner(session, event)
        {
            let _ = shared
                .command(
                    session,
                    crate::engine::Action::Subagent(
                        task,
                        openside_core::SubagentControl::Steer { text },
                    ),
                )
                .await;
        } else {
            steer_notice(shared, session, text).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openside_browser::{DownloadStatus, OpenedBy, TabInfo};
    fn tab() -> TabInfo {
        TabInfo {
            id: "t1".into(),
            url: "https://user:secret@example.test/path?token=secret#secret".into(),
            title: String::new(),
            opened_by: OpenedBy::Agent,
            owner_session_id: None,
            discarded: false,
            unloaded: false,
            browser_context_id: None,
        }
    }
    #[test]
    fn lifecycle_and_popup_strip_sensitive_url_components() {
        for input in [
            BrowserEvent::TabOpened { tab: tab() },
            BrowserEvent::PopupOpened {
                tab: tab(),
                opener_id: "parent".into(),
            },
        ] {
            assert_eq!(
                browser_event(input),
                EventKind::TabOpened {
                    tab_id: "t1".into(),
                    url: "https://example.test/path".into()
                }
            );
        }
        assert_eq!(
            browser_event(BrowserEvent::TabClosed { tab: tab() }),
            EventKind::TabClosed {
                tab_id: "t1".into()
            }
        );
    }

    #[test]
    fn display_facets_allow_only_known_page_periods() {
        for path in ["/trending", "/trending/developers"] {
            for period in ["daily", "weekly", "monthly"] {
                let raw = format!(
                    "https://user:secret@github.com{path}?token=secret&since={period}&q=secret#secret"
                );
                assert_eq!(
                    display_url(&raw),
                    format!("https://github.com{path}?since={period}")
                );
                assert_eq!(public_url(&raw), format!("https://github.com{path}"));
            }
        }
        assert_eq!(
            display_url("https://github.com/trending?token=secret"),
            "https://github.com/trending?since=daily"
        );
        for raw in [
            "https://github.com/trending?since=secret",
            "https://github.com/trending?since=weekly%26token%3Dsecret",
            "https://github.com/trending?since=weekly&since=daily",
            "https://github.com/trending?since=weekly&%73ince=secret",
            "https://github.com/login?since=weekly&token=secret",
            "https://github.com/trending/private?since=weekly&token=secret",
            "https://github.com.evil.test/trending?since=weekly&token=secret",
            "https://github.com:444/trending?since=weekly&token=secret",
            "http://github.com/trending?since=weekly&token=secret",
        ] {
            assert_eq!(display_url(raw), public_url(raw), "{raw}");
            assert!(!display_url(raw).contains("secret"));
        }
        assert!(display_url("not a URL?since=weekly&token=secret").is_empty());
    }

    #[test]
    fn display_facets_keep_daily_and_weekly_history_distinct() {
        let root = tempfile::tempdir().unwrap();
        let store = openside_memory::MemoryStore::open(
            &root.path().join("history.db"),
            openside_memory::Options::default(),
        )
        .unwrap();
        let daily = store
            .record_visit(
                &display_url("https://github.com/trending?token=secret"),
                Some("Daily trending"),
                Some("Daily repositories"),
                "browser_history:test",
            )
            .unwrap();
        let weekly = store
            .record_visit(
                &display_url("https://github.com/trending?since=weekly&token=secret"),
                Some("Weekly trending"),
                Some("Weekly repositories"),
                "browser_history:test",
            )
            .unwrap();
        assert_ne!(daily.id, weekly.id);
        let revisited = store
            .record_visit(
                &display_url("https://github.com/trending?since=daily&session=other-secret"),
                Some("Daily trending"),
                Some("Updated daily repositories"),
                "browser_history:test",
            )
            .unwrap();
        assert_eq!(daily.id, revisited.id);
        assert_eq!(
            store.get(&weekly.id).unwrap().content,
            "Weekly repositories"
        );
    }
    #[test]
    fn maps_all_non_lifecycle_observations_without_dialog_or_filename_content() {
        let inputs = [
            BrowserEvent::DownloadStarted {
                guid: "g".into(),
                url: tab().url,
                suggested_filename: "secret".into(),
                tab_id: None,
            },
            BrowserEvent::DownloadFinished {
                guid: "g".into(),
                status: DownloadStatus::Canceled,
            },
            BrowserEvent::DialogOpened {
                tab_id: "t".into(),
                kind: "confirm".into(),
                message: "secret".into(),
            },
            BrowserEvent::TooManyTabs { count: 5 },
        ];
        for input in inputs {
            assert!(!serde_json::to_string(&browser_event(input))
                .unwrap()
                .contains("secret"));
        }
    }

    #[test]
    fn gateway_setting_selects_an_optional_daemon_scoped_gateway() {
        let auto = launch_config(
            "chrome".into(),
            "profile".into(),
            BrowserGatewaySetting::Auto,
            None,
        );
        assert!(auto.detached);
        assert_eq!(
            auto.profile_dir.as_deref(),
            Some(std::path::Path::new("profile"))
        );
        let gateway = auto.gateway.unwrap();
        assert_eq!(gateway.session_id, GATEWAY_SCOPE);
        assert_eq!(gateway.subagent_id, None);
        assert!(!gateway.required);
        let off = launch_config(
            "chrome".into(),
            "profile".into(),
            BrowserGatewaySetting::Off,
            None,
        );
        assert!(off.gateway.is_none());
    }

    #[cfg(unix)]
    /// Launches with the daemon's `auto` settings: stock Chromium stays CDP-only, while the
    /// patched browser (`OPENSIDE_PATCHED_BROWSER`) is reached through its gateway.
    #[tokio::test]
    async fn browser_auto_gateway_launch_works_with_stock_and_patched_chromium() {
        let mut executables = Vec::new();
        match openside_config::discover_browser(&openside_config::Config::default()) {
            Ok(stock) => executables.push((stock, false)),
            Err(_) => eprintln!("skip stock browser: no chromium-compatible browser found"),
        }
        match std::env::var_os("OPENSIDE_PATCHED_BROWSER") {
            Some(patched) => executables.push((PathBuf::from(patched), true)),
            None => eprintln!("skip patched browser: OPENSIDE_PATCHED_BROWSER is not set"),
        }
        for (executable, patched) in executables {
            let root = tempfile::tempdir().unwrap();
            let runtime = root.path().join("runtime");
            std::fs::create_dir(&runtime).unwrap();
            std::fs::set_permissions(
                &runtime,
                std::os::unix::fs::PermissionsExt::from_mode(0o700),
            )
            .unwrap();
            let mut config = launch_config(
                executable.clone(),
                root.path().join("browser-profile"),
                BrowserGatewaySetting::Auto,
                Some(runtime.clone()),
            );
            // A failing assertion must not leave a detached browser running.
            config.detached = false;
            let (browser, _events) = openside_browser::Browser::launch(config).await.unwrap();
            assert_eq!(
                browser.gateway().is_some(),
                patched,
                "{}",
                executable.display()
            );
            let tab = browser
                .create_agent_tab("session-a", "about:blank")
                .await
                .unwrap();
            let png = browser.screenshot(&tab.id).await.unwrap();
            assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
            if let Some(gateway) = browser.gateway() {
                let record = gateway.tab_for_target(&tab.id).await.unwrap().unwrap();
                assert_eq!(record.session_id.as_deref(), Some(GATEWAY_SCOPE));
                let size = (
                    u32::from_be_bytes(png[16..20].try_into().unwrap()),
                    u32::from_be_bytes(png[20..24].try_into().unwrap()),
                );
                assert_eq!(size, (1440, 900));
            } else {
                // Stock Chromium ignores the gateway switches and publishes no gateway
                // (it may still keep its own unrelated files in the runtime directory).
                assert!(!runtime.join("openside").exists());
            }
            browser.shutdown().await.unwrap();
        }
    }
}
