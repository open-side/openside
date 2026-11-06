//! The person's browser settings (spell check, languages, cookies, picture-in-picture, zoom,
//! search engine, the mini popup shortcut), read and written in the person's own browser through
//! its person-role gateway
//! (see [`crate::person_gateway`]), never in the agent browser. Stock Chromium and a closed
//! browser are reported, never emulated: the daemon keeps no copy of these values.

use axum::{extract::State, http::StatusCode, Extension, Json};
use openside_browser::{BrowserError, BrowserPrefUpdate, CookiePolicy, Gateway};
use openside_proto::{
    BrowserPrefValues, BrowserPrefs, BrowserPrefsStatus, BrowserSearchEngine, BrowserSearchEngines,
    ThirdPartyCookies, UpdateBrowserPrefsRequest,
};
use serde_json::json;

use crate::{
    api::{Caller, HttpError},
    engine::Shared,
    person_gateway::PersonBrowser,
};

/// The gateway calls this API needs; the fake in the tests stands in for a patched browser.
pub(crate) trait PrefsGateway {
    fn supports_prefs(&self) -> bool;
    async fn prefs(&self) -> Result<openside_browser::BrowserPrefs, BrowserError>;
    async fn set_pref(
        &self,
        update: &BrowserPrefUpdate,
    ) -> Result<openside_browser::BrowserPrefs, BrowserError>;
}

impl PrefsGateway for Gateway {
    fn supports_prefs(&self) -> bool {
        Gateway::supports_prefs(self)
    }
    async fn prefs(&self) -> Result<openside_browser::BrowserPrefs, BrowserError> {
        Gateway::prefs(self).await
    }
    async fn set_pref(
        &self,
        update: &BrowserPrefUpdate,
    ) -> Result<openside_browser::BrowserPrefs, BrowserError> {
        Gateway::set_pref(self, update).await
    }
}

pub(crate) async fn get(State(shared): State<Shared>) -> Result<Json<BrowserPrefs>, HttpError> {
    Ok(Json(match shared.registry.person_gateway.get().await {
        PersonBrowser::Gateway(gateway) => read(Some(&*gateway)).await?,
        PersonBrowser::Stock => without(BrowserPrefsStatus::Unsupported),
        PersonBrowser::NotRunning => without(BrowserPrefsStatus::NotRunning),
    }))
}

pub(crate) async fn patch(
    State(shared): State<Shared>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<UpdateBrowserPrefsRequest>,
) -> Result<Json<BrowserPrefs>, HttpError> {
    let gateway = match shared.registry.person_gateway.get().await {
        PersonBrowser::Gateway(gateway) => gateway,
        PersonBrowser::Stock => return Err(unsupported()),
        PersonBrowser::NotRunning => {
            return Err(HttpError(StatusCode::CONFLICT, "browser_not_running"))
        }
    };
    let mut applied = Vec::new();
    let result = update(Some(&*gateway), request, &mut applied).await;
    // Every field the browser applied, also when a later one failed; names only, values
    // (languages, engine choice) stay out of the audit log.
    if !applied.is_empty() {
        crate::pairing::audit(
            &shared,
            "browser_prefs",
            "changed",
            json!({"device": caller.0, "prefs": applied, "complete": result.is_ok()}),
        )
        .await;
    }
    result.map(Json)
}

pub(crate) async fn read<G: PrefsGateway>(gateway: Option<&G>) -> Result<BrowserPrefs, HttpError> {
    let Some(gateway) = gateway.filter(|gateway| gateway.supports_prefs()) else {
        return Ok(without(BrowserPrefsStatus::Unsupported));
    };
    match gateway.prefs().await {
        Ok(prefs) => Ok(available(prefs)),
        // A patched build that has no person profile to read from.
        Err(BrowserError::Gateway { code, .. }) if code == "unsupported" => {
            Ok(without(BrowserPrefsStatus::Unsupported))
        }
        Err(error) => Err(http_error(error)),
    }
}

/// Applies the request's fields in order, pushing each one the browser applied onto `applied`.
pub(crate) async fn update<G: PrefsGateway>(
    gateway: Option<&G>,
    request: UpdateBrowserPrefsRequest,
    applied: &mut Vec<&'static str>,
) -> Result<BrowserPrefs, HttpError> {
    let Some(gateway) = gateway.filter(|gateway| gateway.supports_prefs()) else {
        return Err(unsupported());
    };
    let mut written = None;
    for update in updates(request) {
        written = Some(gateway.set_pref(&update).await.map_err(http_error)?);
        applied.push(update.name());
    }
    match written {
        Some(prefs) => Ok(available(prefs)),
        None => gateway.prefs().await.map(available).map_err(http_error),
    }
}

/// Field order of the request, which is the documented application order.
fn updates(request: UpdateBrowserPrefsRequest) -> Vec<BrowserPrefUpdate> {
    let UpdateBrowserPrefsRequest {
        spell_check,
        accept_languages,
        third_party_cookies,
        auto_picture_in_picture,
        default_zoom_percent,
        search_engine,
        mini_popup,
    } = request;
    [
        spell_check.map(BrowserPrefUpdate::SpellCheck),
        accept_languages.map(BrowserPrefUpdate::AcceptLanguages),
        third_party_cookies.map(|policy| {
            BrowserPrefUpdate::ThirdPartyCookies(match policy {
                ThirdPartyCookies::Allow => CookiePolicy::Allow,
                ThirdPartyCookies::Block => CookiePolicy::Block,
            })
        }),
        auto_picture_in_picture.map(BrowserPrefUpdate::AutoPictureInPicture),
        default_zoom_percent.map(BrowserPrefUpdate::DefaultZoomPercent),
        search_engine.map(BrowserPrefUpdate::SearchEngine),
        mini_popup.map(BrowserPrefUpdate::MiniPopup),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn without(status: BrowserPrefsStatus) -> BrowserPrefs {
    BrowserPrefs {
        status,
        prefs: None,
    }
}

fn available(prefs: openside_browser::BrowserPrefs) -> BrowserPrefs {
    BrowserPrefs {
        status: BrowserPrefsStatus::Available,
        prefs: Some(BrowserPrefValues {
            spell_check: prefs.spell_check,
            accept_languages: prefs.accept_languages,
            third_party_cookies: match prefs.third_party_cookies {
                CookiePolicy::Allow => ThirdPartyCookies::Allow,
                CookiePolicy::Block => ThirdPartyCookies::Block,
            },
            auto_picture_in_picture: prefs.auto_picture_in_picture,
            default_zoom_percent: prefs.default_zoom_percent,
            search_engine: BrowserSearchEngines {
                current: prefs.search_engine.current,
                options: prefs
                    .search_engine
                    .options
                    .into_iter()
                    .map(|engine| BrowserSearchEngine {
                        id: engine.id,
                        name: engine.name,
                    })
                    .collect(),
            },
            mini_popup: prefs.mini_popup,
        }),
    }
}

fn disconnected() -> HttpError {
    HttpError(StatusCode::SERVICE_UNAVAILABLE, "browser_disconnected")
}

fn unsupported() -> HttpError {
    HttpError(StatusCode::CONFLICT, "browser_prefs_unsupported")
}

fn http_error(error: BrowserError) -> HttpError {
    match error {
        BrowserError::Gateway { code, .. } => match code.as_str() {
            "invalid_request" => HttpError::invalid_value(),
            "policy_denied" => HttpError(StatusCode::CONFLICT, "browser_pref_managed"),
            "unsupported" => unsupported(),
            // The browser's bounded wait for its search engines expired or is full.
            "timeout" | "resource_exhausted" => {
                HttpError(StatusCode::SERVICE_UNAVAILABLE, "browser_busy")
            }
            _ => HttpError::internal(),
        },
        BrowserError::Disconnected | BrowserError::Timeout(_) => disconnected(),
        _ => HttpError::internal(),
    }
}
