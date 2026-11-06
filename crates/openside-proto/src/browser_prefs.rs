use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The person's browser settings that the patched browser applies to its own profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserPrefs {
    /// Whether the values below can be read and changed right now.
    pub status: BrowserPrefsStatus,
    /// Current values read back from the browser; present only when `status` is `available`.
    pub prefs: Option<BrowserPrefValues>,
}

/// Why browser settings are or are not reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserPrefsStatus {
    /// The patched browser is running and exposes its settings.
    Available,
    /// No browser is running; the daemon never launches one to read settings.
    NotRunning,
    /// The running browser is stock Chromium/Chrome or a patched build without settings access.
    Unsupported,
}

/// Values as the browser reports them, never cached by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserPrefValues {
    /// Spell check while typing (`browser.enable_spellchecking`).
    pub spell_check: bool,
    /// Preferred page languages in order (`intl.accept_languages`).
    pub accept_languages: Vec<String>,
    /// Third-party cookie blocking.
    pub third_party_cookies: ThirdPartyCookies,
    /// Whether playing video may enter picture-in-picture automatically when its tab is left;
    /// null when the browser has no content settings for the profile.
    pub auto_picture_in_picture: Option<bool>,
    /// Default page zoom in percent; null when the profile has no zoom preferences.
    pub default_zoom_percent: Option<u16>,
    /// Default search engine and the built-in engines it can be switched to.
    pub search_engine: BrowserSearchEngines,
    /// Whether Alt+Space (⌥Space on macOS) opens the mini popup from anywhere; null when the
    /// openside extension is not enabled in the browser's profile.
    pub mini_popup: Option<bool>,
}

/// Third-party cookie policy as chrome://settings offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ThirdPartyCookies {
    /// Allowed (blocked in incognito only).
    Allow,
    /// Blocked everywhere.
    Block,
}

/// Built-in search engines only; engines the person added themselves and every search URL stay
/// inside the browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSearchEngines {
    /// Id of the current default, or null when it is not a built-in engine.
    pub current: Option<u32>,
    /// Engines the default can be switched to, in the browser's order.
    pub options: Vec<BrowserSearchEngine>,
}

/// One built-in search engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSearchEngine {
    /// Chromium's prepopulated engine id.
    pub id: u32,
    /// Display name.
    pub name: String,
}

/// Change browser settings; absent fields stay unchanged. Fields are applied in declaration
/// order, so on failure the earlier fields may already have been applied.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateBrowserPrefsRequest {
    /// Spell check while typing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spell_check: Option<bool>,
    /// 1-32 distinct BCP 47 language tags, most preferred first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept_languages: Option<Vec<String>>,
    /// Third-party cookie blocking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub third_party_cookies: Option<ThirdPartyCookies>,
    /// Automatic picture-in-picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_picture_in_picture: Option<bool>,
    /// One of 25, 33, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_zoom_percent: Option<u16>,
    /// An id from `search_engine.options`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_engine: Option<u32>,
    /// Global Alt+Space shortcut for the mini popup; needs the openside extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mini_popup: Option<bool>,
}
