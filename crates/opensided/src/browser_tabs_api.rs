use axum::{extract::State, Json};
use openside_browser::{OpenedBy, TabInfo};
use openside_proto::{BrowserTab, BrowserTabList, BrowserTabOwnership};
use std::collections::HashSet;

use crate::{api::HttpError, engine::Shared};

pub(crate) async fn list(State(shared): State<Shared>) -> Result<Json<BrowserTabList>, HttpError> {
    let private = shared
        .db
        .store(|store| {
            Ok(store
                .list_sessions()
                .into_iter()
                .filter(|session| session.incognito)
                .map(|session| session.id.to_string())
                .collect::<HashSet<_>>())
        })
        .await?;
    // Private sessions have separate controllers; never enumerate those profiles.
    let tabs = match shared.registry.browser.existing().await {
        Ok(browser) if browser.is_connected() => browser.tabs().await,
        Ok(_) => {
            return Err(HttpError(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "browser_disconnected",
            ));
        }
        Err(crate::tools::ToolError(reason)) if reason == "Browser is not running" => Vec::new(),
        Err(_) => return Err(HttpError::internal()),
    };
    Ok(Json(BrowserTabList {
        tabs: public_tabs(tabs, &private),
    }))
}

fn public_tabs(tabs: Vec<TabInfo>, private: &HashSet<String>) -> Vec<BrowserTab> {
    tabs.into_iter()
        .filter(|tab| {
            tab.opened_by == OpenedBy::Agent
                && tab
                    .owner_session_id
                    .as_ref()
                    .is_some_and(|id| !private.contains(id))
        })
        .map(|tab| BrowserTab {
            tab_id: tab.id,
            url: crate::browser::public_url(&tab.url),
            title: tab.title,
            opened_by: match tab.opened_by {
                OpenedBy::Agent => BrowserTabOwnership::Agent,
                OpenedBy::User => BrowserTabOwnership::User,
                OpenedBy::External => BrowserTabOwnership::External,
            },
            owner_session_id: tab.owner_session_id,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_tabs_are_omitted_and_provenance_never_depends_on_url() {
        let tab = |id: &str, owner: &str, opened_by| TabInfo {
            id: id.into(),
            url:
                "https://user:private-url-token@same.example.test/?token=private-url-token#fragment"
                    .into(),
            title: "Page".into(),
            opened_by,
            owner_session_id: Some(owner.into()),
            discarded: false,
            unloaded: false,
        };
        let tabs = public_tabs(
            vec![
                tab("agent", "normal", OpenedBy::Agent),
                tab("private", "secret", OpenedBy::Agent),
                tab("user", "normal", OpenedBy::User),
                tab("external", "normal", OpenedBy::External),
                TabInfo {
                    owner_session_id: None,
                    ..tab("unknown", "normal", OpenedBy::Agent)
                },
            ],
            &HashSet::from(["secret".into()]),
        );
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].tab_id, "agent");
        assert_eq!(tabs[0].opened_by, BrowserTabOwnership::Agent);
        assert_eq!(tabs[0].url, "https://same.example.test/");
        assert!(!serde_json::to_string(&tabs)
            .unwrap()
            .contains("private-url-token"));
        assert!(!serde_json::to_string(&tabs).unwrap().contains("secret"));
    }
}
