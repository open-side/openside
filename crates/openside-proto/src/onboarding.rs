use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where the first-run guide stands. `pending` until a person finishes or skips it once;
/// the flag lives on the daemon, so every device and browser agrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingState {
    /// Never finished or skipped.
    Pending,
    /// Walked through to the end.
    Completed,
    /// Dismissed before the end.
    Skipped,
}

/// `GET /v1/onboarding` and the `PUT` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OnboardingStatus {
    /// Current state.
    pub state: OnboardingState,
    /// When the state last changed; `null` while pending.
    pub updated_at: Option<DateTime<Utc>>,
}

/// `PUT /v1/onboarding`: record how the guide ended. Only `completed` and `skipped` are
/// accepted; reopening the guide from Settings does not reset the flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateOnboardingRequest {
    /// `completed` or `skipped`.
    pub state: OnboardingState,
}

/// An agent CLI openside can install its browser skill for (`openside skills install`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExternalAgent {
    /// Preset name (`claude`, `codex`, `omp`) or an entry of `[skills.agents]`.
    pub id: String,
    /// Product name for display.
    pub label: String,
    /// The agent's executable is on the daemon's `PATH` or its configuration directory exists.
    pub detected: bool,
    /// `openside-browser` is already in the agent's skills directory.
    pub skill_installed: bool,
    /// Where the skill would be (or is) installed, written `~/…` when under the daemon user's
    /// home; `null` when no directory is known.
    pub skills_dir: Option<String>,
    /// Added by the user rather than a preset: its folder can be changed or removed
    /// (`PUT`/`DELETE /v1/external-agents/{id}`).
    #[serde(default)]
    pub custom: bool,
}

/// `GET /v1/external-agents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExternalAgentList {
    /// The three built-in presets first, then configured agents by name.
    pub agents: Vec<ExternalAgent>,
}

/// `POST /v1/external-agents/{id}/browser-skill` body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallAgentSkillRequest {
    /// Replace an `openside-browser` skill that is already installed.
    #[serde(default)]
    pub force: bool,
}

/// What the install wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstalledAgentSkill {
    /// Agent the skill was installed for.
    pub agent: String,
    /// Installed skill name (`openside-browser`).
    pub skill: String,
    /// Final location of the skill directory.
    pub path: String,
    /// Files written.
    pub files: u32,
}

/// Parser family; UI brand identity is carried separately on each profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportBrowser {
    /// Google Chrome's profile formats.
    Chrome,
    /// Chromium-derived profile formats.
    Chromium,
    /// Firefox-derived profile formats.
    Firefox,
    /// macOS Safari's permitted local files.
    Safari,
}

/// Independent import categories; unavailable/protected kinds do not stop other kinds.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ImportCategory {
    /// Web visits retained for browser history.
    History,
    /// Web bookmarks with folder paths.
    Bookmarks,
    /// Authenticated sessions written through the target browser.
    Cookies,
    /// Sign-ins saved exclusively into encrypted Vault.
    Passwords,
    /// Non-payment address/form fields.
    Autofill,
    /// Current tabs offered for explicit restoration.
    Tabs,
    /// Search-provider inventory.
    SearchEngines,
    /// Site preference inventory, not openside authorization.
    Permissions,
    /// Extension labels/store links, never installations.
    Extensions,
}

/// Known source-file presence; true does not guarantee successful decryption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportAvailability {
    /// A history source exists.
    pub history: bool,
    /// A bookmark source exists.
    pub bookmarks: bool,
    /// A cookie source exists.
    pub cookies: bool,
    /// A password record source exists.
    pub passwords: bool,
    /// An address/form source exists.
    pub autofill: bool,
    /// An open-session source exists.
    pub tabs: bool,
    /// A provider metadata source exists.
    pub search_engines: bool,
    /// A site preference source exists.
    pub permissions: bool,
    /// An extension metadata source exists.
    pub extensions: bool,
}

/// Safe local profile metadata; absolute source paths remain host-side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportProfile {
    /// Opaque, collision-safe selection identifier.
    pub id: String,
    /// Family selecting the host-side parsers.
    pub browser: ImportBrowser,
    /// Stable browser brand/icon identifier.
    pub brand: String,
    /// Browser-provided display name with a directory-name fallback.
    pub name: String,
    /// Account label from Local State when present.
    pub email: Option<String>,
    /// Browser-local avatar identifier, never a remote image URL.
    pub avatar: Option<String>,
    /// Known source-file availability by category.
    pub available: ImportAvailability,
    /// Safe permission guidance, including Safari Full Disk Access.
    pub warnings: Vec<String>,
}

/// Local detection response; preview supplies actual per-kind record counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportProfileList {
    /// Every profile found in the supported standard locations.
    pub profiles: Vec<ImportProfile>,
    /// The target Chromium executable is available.
    pub cookies_writable: bool,
    /// Detected password sources require creating Vault first.
    pub vault_needed: bool,
    /// Detected password sources require unlocking the configured Vault.
    pub vault_locked: bool,
    /// Safe whole-detection guidance, never filesystem paths.
    pub warnings: Vec<String>,
}

/// Selection shared by preview and run; omitted/null means every profile/kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    /// Discovered opaque ids; an explicit empty array is invalid.
    #[serde(default)]
    pub profiles: Option<Vec<String>>,
    /// Category selection; an explicit empty array is invalid.
    #[serde(default)]
    pub what: Option<Vec<ImportCategory>>,
}

/// Value-free label for a cookie the target browser rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportRejectedCookie {
    /// Source host/domain, never the cookie value.
    pub host: String,
    /// Source cookie name.
    pub name: String,
}

/// Per-kind outcome; credential preview counts source rows without decryption or key access.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportCategoryReport {
    /// Source records successfully read or counted.
    pub read: u64,
    /// Written records, or source candidates during preview (credentials are not verified).
    pub imported: u64,
    /// Excluded invalid, duplicate, expired or unmappable records.
    pub skipped: u64,
    /// Rejected-cookie labels with no values.
    pub rejected: Vec<ImportRejectedCookie>,
    /// Safe unavailable/failed guidance, or preview guidance about explicit import-time access.
    pub note: Option<String>,
}

/// One selected profile's safe category reports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportReport {
    /// Opaque source profile id.
    pub id: String,
    /// True if no target data was written.
    pub dry_run: bool,
    /// Source browser brand.
    pub source: String,
    /// Agent destination profile, always default.
    pub profile: String,
    /// Selected history outcome.
    pub history: Option<ImportCategoryReport>,
    /// Selected bookmark outcome.
    pub bookmarks: Option<ImportCategoryReport>,
    /// Selected cookie outcome, never values.
    pub cookies: Option<ImportCategoryReport>,
    /// Selected Vault-password outcome, never values.
    pub passwords: Option<ImportCategoryReport>,
    /// Selected address/form outcome.
    pub autofill: Option<ImportCategoryReport>,
    /// Selected current-tab outcome.
    pub tabs: Option<ImportCategoryReport>,
    /// Selected search-provider outcome.
    pub search_engines: Option<ImportCategoryReport>,
    /// Selected site-preference outcome.
    pub permissions: Option<ImportCategoryReport>,
    /// Selected extension-inventory outcome.
    pub extensions: Option<ImportCategoryReport>,
}

/// A current tab offered as a restore link; it is never automatically opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportedRestoreTab {
    /// Opaque profile that supplied the tab.
    pub profile_id: String,
    /// Current supported web navigation.
    pub url: String,
    /// Source tab display title.
    pub title: String,
}

/// Imported extension inventory, not an installation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportedExtension {
    /// Source profile selector.
    pub profile_id: String,
    /// Browser/store extension id.
    pub id: String,
    /// Source human-readable label.
    pub name: String,
    /// Official store page when identifiable.
    pub store_url: Option<String>,
}

/// Search-provider metadata shown without altering the browser's default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportedSearchEngine {
    /// Source profile selector.
    pub profile_id: String,
    /// Source provider display label.
    pub name: String,
    /// Source omnibox shortcut.
    pub keyword: String,
    /// Provider substitution template.
    pub url: String,
}

/// Source site-preference metadata, not a grant to openside tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportedPermission {
    /// Source profile selector.
    pub profile_id: String,
    /// Source origin or complete primary/secondary match pattern.
    pub origin: String,
    /// Source capability label.
    pub permission: String,
    /// Source browser-specific allow/block/default value.
    pub setting: String,
}

/// Complete preview or final/partial summary; secret values are excluded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportBatchReport {
    /// True for a read-only preview.
    pub dry_run: bool,
    /// Agent destination, always default.
    pub profile: String,
    /// One outcome per selected source profile.
    pub profiles: Vec<ImportReport>,
    /// Written/prospective counts summed by category.
    pub totals: std::collections::BTreeMap<ImportCategory, u64>,
    /// Safe per-kind and permission guidance.
    pub warnings: Vec<String>,
    /// Selected passwords require creating Vault first.
    pub vault_needed: bool,
    /// Selected passwords require unlocking Vault first.
    pub vault_locked: bool,
    /// Current tabs for explicit user restoration.
    pub restore_tabs: Vec<ImportedRestoreTab>,
    /// Extension names and official store links, never binaries.
    pub extensions: Vec<ImportedExtension>,
    /// Search-provider inventory.
    pub search_engines: Vec<ImportedSearchEngine>,
    /// Site-preference inventory.
    pub permissions: Vec<ImportedPermission>,
}

/// Durable background job lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportJobState {
    /// Admitted job still processing profiles.
    Running,
    /// Every selected profile was processed; category warnings may remain.
    Completed,
    /// Processing ended early; completed profiles remain in the summary.
    Failed,
}

/// Pollable job status; never contains source credential values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportJob {
    /// Random opaque job id.
    pub id: String,
    /// Current or terminal lifecycle state.
    pub state: ImportJobState,
    /// Fully processed source profiles.
    pub completed_profiles: u64,
    /// Selected source profiles at admission.
    pub total_profiles: u64,
    /// Currently processed opaque source id.
    pub current_profile: Option<String>,
    /// Final or partial safe report once processing ends.
    pub summary: Option<ImportBatchReport>,
    /// Stable failure code, never exception text.
    pub error: Option<String>,
}

/// Credential-free durable SSE job progress payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportProgress {
    /// Job owning this progress record.
    pub job_id: String,
    /// Lifecycle after this record was committed.
    pub state: ImportJobState,
    /// Fully processed profiles.
    pub completed_profiles: u64,
    /// Profiles selected at job admission.
    pub total_profiles: u64,
    /// Currently processed source id, not a directory path.
    pub current_profile: Option<String>,
}

/// Local-person CSV upload parsed in memory directly into encrypted Vault.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportPasswordsRequest {
    /// User-selected UTF-8 export; never persist, log or echo this string.
    pub csv: String,
}

/// Independently acknowledged destinations for the same migration revision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportDeliveryTarget {
    /// Dedicated desktop openside-browser profile.
    #[default]
    User,
    /// Managed agent browser profile and its host-side lookup store.
    Agent,
}

/// Local-only delivery selector; it cannot name an arbitrary browser/profile path.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ImportDeliveryQuery {
    /// Omitted means the dedicated desktop user profile.
    #[serde(default)]
    pub target: ImportDeliveryTarget,
}

/// Acknowledgement sent only after the destination applied the batch.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportDeliveryAck {
    /// Destination whose pending marker is cleared.
    pub target: ImportDeliveryTarget,
}

/// Safe origin of a pending native-delivery batch.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportDeliverySource {
    /// Opaque discovered profile id.
    pub id: String,
    /// Stable source browser brand.
    pub brand: String,
    /// Profile display label.
    pub name: String,
}

/// Bookmark transported to the extension's supported native bookmarks API.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportDeliveryBookmark {
    /// HTTP(S), file or FTP URL, used with folder path for native dedupe.
    pub url: String,
    /// Source display title.
    pub title: String,
    /// Folder names, outermost first.
    pub folder: Vec<String>,
    /// Source creation time in Unix milliseconds when known.
    pub added_at_ms: Option<i64>,
}

/// Visit transported to the extension's supported native history API.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportDeliveryVisit {
    /// Supported web URL used for native history dedupe.
    pub url: String,
    /// Source display title when known.
    pub title: Option<String>,
    /// Source time in Unix milliseconds.
    pub visited_at_ms: i64,
}

/// Chrome's partition boundary, preserved when supported rather than silently dropped.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportDeliveryCookiePartitionKey {
    /// Schemeful top-level site owning the partition.
    pub top_level_site: String,
    /// Chromium's cross-site-ancestor boundary when reported.
    pub has_cross_site_ancestor: Option<bool>,
}

/// Authenticated daemon-to-extension cookie transport; never persist, log or expose to pages.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportDeliveryCookie {
    /// URL establishing the cookie's supported origin.
    pub url: String,
    /// Source cookie name.
    pub name: String,
    /// Secret value read live from the encrypted agent cookie store.
    pub value: String,
    /// Domain scope; absent means host-only.
    pub domain: Option<String>,
    /// Source cookie path.
    pub path: String,
    /// Source Secure attribute.
    pub secure: bool,
    /// Source HttpOnly attribute.
    pub http_only: bool,
    /// Chrome API SameSite enum string when recorded.
    pub same_site: Option<String>,
    /// Unix seconds; absent means a session cookie.
    pub expiration_date: Option<f64>,
    /// Live native partition boundary; current source parsers skip unmappable partitions.
    pub partition_key: Option<ImportDeliveryCookiePartitionKey>,
}

impl Drop for ImportDeliveryCookie {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.value);
    }
}

/// Native-delivery revision; cookie values exist only in the live local response.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportDeliveryBatch {
    /// Opaque revision id, replayed until that target acknowledges it.
    pub id: String,
    /// Value-free source display metadata.
    pub source: ImportDeliverySource,
    /// Folder-aware web bookmarks.
    pub bookmarks: Vec<ImportDeliveryBookmark>,
    /// Source visits for the native history API.
    pub history: Vec<ImportDeliveryVisit>,
    /// Live secret transport, excluded from the durable pending metadata.
    pub cookies: Vec<ImportDeliveryCookie>,
}

/// Value-free local-person delivery descriptors; the extension fetches the actual records.
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ImportDeliveryList {
    /// Pending revision descriptors, containing no native cookie values.
    pub batches: Vec<ImportDeliveryPending>,
}

/// A pending user-profile import revision with exact permission origins.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportDeliveryPending {
    /// Stable native-delivery revision identifier.
    pub id: String,
    /// Exact optional host patterns; no cookie values or source records.
    pub cookie_origins: Vec<String>,
}

/// Local-person request to authorize one batch for a paired extension.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportDeliveryTicketRequest {
    /// Extension's revocation identifier from its verified pairing.
    pub device_id: String,
}

/// Short-lived authorization redeemed only by the designated extension device.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportDeliveryTicket {
    /// Single-use, short-lived capability bound to the paired extension device and batch.
    pub ticket: String,
}

/// Device-authenticated, HMAC-protected native import response.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportDeliverySigned {
    /// Exact single-use ticket consumed by this response.
    pub ticket: String,
    /// Native-delivery revision authenticated together with the payload.
    pub batch_id: String,
    /// Exact UTF-8 JSON signed with HMAC-SHA256; never forwarded to a page.
    pub payload: String,
    /// Lowercase hex HMAC-SHA256(ticket || batch_id || SHA256(payload) as hex).
    pub signature: String,
}

impl Drop for ImportDeliverySigned {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.payload);
    }
}
