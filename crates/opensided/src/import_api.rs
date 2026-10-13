//! Local-person browser migration. Jobs and progress are durable; credential values never enter them.

use std::{
    collections::HashMap,
    convert::Infallible,
    fs,
    path::{Path as FsPath, PathBuf},
    sync::LazyLock,
    time::Duration,
};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    Extension, Json,
};
use openside_import::{
    BrowserKind, CategoryReport, DataKind, DiscoveredProfile, KeyringSource, ProfileLayout,
    ProfileRead, SavedPassword, DEFAULT_PROFILE,
};
use openside_proto::{
    ImportAvailability, ImportBatchReport, ImportBrowser, ImportCategory, ImportCategoryReport,
    ImportJob, ImportJobState, ImportPasswordsRequest, ImportProfile, ImportProfileList,
    ImportProgress, ImportRejectedCookie, ImportReport, ImportRequest, ImportedExtension,
    ImportedPermission, ImportedRestoreTab, ImportedSearchEngine,
};
use openside_vault::{AuditContext, Redactor, Secret};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    api::{Caller, HttpError},
    engine::Shared,
    remote::LocalAuthority,
};

static ACTIVE: LazyLock<Mutex<HashMap<PathBuf, String>>> = LazyLock::new(Mutex::default);
fn active() -> &'static Mutex<HashMap<PathBuf, String>> {
    &ACTIVE
}

struct Running(PathBuf);
impl Running {
    fn start(root: &FsPath, id: &str) -> Result<Self, HttpError> {
        let mut jobs = active().lock();
        if jobs.contains_key(root) {
            return Err(HttpError(StatusCode::CONFLICT, "import_in_progress"));
        }
        jobs.insert(root.to_owned(), id.to_owned());
        Ok(Self(root.to_owned()))
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        active().lock().remove(&self.0);
    }
}

#[derive(Serialize, Deserialize)]
struct StoredJob {
    job: ImportJob,
    events: Vec<ImportProgress>,
}

fn browser(kind: BrowserKind) -> ImportBrowser {
    match kind {
        BrowserKind::Chrome => ImportBrowser::Chrome,
        BrowserKind::Chromium => ImportBrowser::Chromium,
        BrowserKind::Firefox => ImportBrowser::Firefox,
        BrowserKind::Safari => ImportBrowser::Safari,
    }
}
fn kind(category: ImportCategory) -> DataKind {
    match category {
        ImportCategory::History => DataKind::History,
        ImportCategory::Bookmarks => DataKind::Bookmarks,
        ImportCategory::Cookies => DataKind::Cookies,
        ImportCategory::Passwords => DataKind::Passwords,
        ImportCategory::Autofill => DataKind::Autofill,
        ImportCategory::Tabs => DataKind::Tabs,
        ImportCategory::SearchEngines => DataKind::SearchEngines,
        ImportCategory::Permissions => DataKind::Permissions,
        ImportCategory::Extensions => DataKind::Extensions,
    }
}
fn category(kind: DataKind) -> ImportCategory {
    match kind {
        DataKind::History => ImportCategory::History,
        DataKind::Bookmarks => ImportCategory::Bookmarks,
        DataKind::Cookies => ImportCategory::Cookies,
        DataKind::Passwords => ImportCategory::Passwords,
        DataKind::Autofill => ImportCategory::Autofill,
        DataKind::Tabs => ImportCategory::Tabs,
        DataKind::SearchEngines => ImportCategory::SearchEngines,
        DataKind::Permissions => ImportCategory::Permissions,
        DataKind::Extensions => ImportCategory::Extensions,
    }
}

async fn discovered() -> Result<Vec<DiscoveredProfile>, HttpError> {
    tokio::task::spawn_blocking(openside_import::discover_system)
        .await
        .map_err(|_| HttpError::internal())
}

async fn vault_flags(shared: &Shared) -> (bool, bool) {
    let needed = !crate::tools::vault::Service::file(shared).is_file();
    let locked = !needed && shared.registry.vault.vault.lock().await.is_none();
    (needed, locked)
}

pub(crate) async fn profiles(
    State(shared): State<Shared>,
    Extension(local): Extension<LocalAuthority>,
) -> Result<Json<ImportProfileList>, HttpError> {
    local.require()?;
    let profiles = discovered()
        .await?
        .into_iter()
        .map(|profile| ImportProfile {
            id: profile.id,
            browser: browser(profile.kind),
            brand: profile.brand,
            name: profile.name,
            email: profile.email,
            avatar: profile.avatar,
            available: ImportAvailability {
                history: profile.available.history,
                bookmarks: profile.available.bookmarks,
                cookies: profile.available.cookies,
                passwords: profile.available.passwords,
                autofill: profile.available.autofill,
                tabs: profile.available.tabs,
                search_engines: profile.available.search_engines,
                permissions: profile.available.permissions,
                extensions: profile.available.extensions,
            },
            warnings: profile.warnings,
        })
        .collect::<Vec<_>>();
    let has_passwords = profiles.iter().any(|profile| profile.available.passwords);
    let (needed, locked) = vault_flags(&shared).await;
    Ok(Json(ImportProfileList {
        profiles,
        cookies_writable: openside_config::discover_browser(&shared.config.loaded.config).is_ok(),
        vault_needed: has_passwords && needed,
        vault_locked: has_passwords && locked,
        warnings: Vec::new(),
    }))
}

async fn selection(
    request: ImportRequest,
) -> Result<(Vec<DiscoveredProfile>, Vec<DataKind>), HttpError> {
    let mut profiles = discovered().await?;
    if let Some(selected) = request.profiles {
        if selected.is_empty() {
            return Err(HttpError::invalid_value());
        }
        if selected
            .iter()
            .any(|id| !profiles.iter().any(|profile| profile.id == *id))
        {
            return Err(HttpError(StatusCode::BAD_REQUEST, "unknown_profile"));
        }
        profiles.retain(|profile| selected.contains(&profile.id));
    }
    let kinds = match request.what {
        None => DataKind::ALL.to_vec(),
        Some(list) if list.is_empty() => return Err(HttpError::invalid_value()),
        Some(list) => {
            let mut kinds = list.into_iter().map(kind).collect::<Vec<_>>();
            kinds.sort();
            kinds.dedup();
            kinds
        }
    };
    Ok((profiles, kinds))
}

fn report_category(report: CategoryReport) -> ImportCategoryReport {
    ImportCategoryReport {
        read: report.read,
        imported: report.imported,
        skipped: report.skipped,
        rejected: report
            .rejected
            .into_iter()
            .map(|cookie| ImportRejectedCookie {
                host: cookie.host,
                name: cookie.name,
            })
            .collect(),
        note: report.note,
    }
}

fn profile_report(
    profile: &DiscoveredProfile,
    read: &mut ProfileRead,
    preview: bool,
) -> ImportReport {
    let mut take = |kind| read.reports.remove(&kind).map(report_category);
    ImportReport {
        id: profile.id.clone(),
        dry_run: preview,
        source: profile.brand.clone(),
        profile: DEFAULT_PROFILE.to_owned(),
        history: take(DataKind::History),
        bookmarks: take(DataKind::Bookmarks),
        cookies: take(DataKind::Cookies),
        passwords: take(DataKind::Passwords),
        autofill: take(DataKind::Autofill),
        tabs: take(DataKind::Tabs),
        search_engines: take(DataKind::SearchEngines),
        permissions: take(DataKind::Permissions),
        extensions: take(DataKind::Extensions),
    }
}

fn add_profile(
    summary: &mut ImportBatchReport,
    profile: &DiscoveredProfile,
    mut read: ProfileRead,
) {
    for (kind, report) in &read.reports {
        *summary.totals.entry(category(*kind)).or_default() += report.imported;
        if let Some(note) = &report.note {
            if note != "Not found in this profile." {
                summary
                    .warnings
                    .push(format!("{} — {}: {note}", profile.name, kind.label()));
            }
        }
        if *kind == DataKind::Bookmarks && report.skipped > 0 && report.note.is_none() {
            summary.warnings.push(format!(
                "{} — bookmarks: {} internal links skipped.",
                profile.name, report.skipped
            ));
        }
    }
    summary.warnings.extend(
        profile
            .warnings
            .iter()
            .map(|warning| format!("{}: {warning}", profile.name)),
    );
    summary
        .restore_tabs
        .extend(read.data.tabs.drain(..).filter_map(|mut tab| {
            let url = openside_import::sanitize_import_url(&tab.url)?;
            if tab.title == tab.url {
                tab.title = url.clone();
            }
            Some(ImportedRestoreTab {
                profile_id: profile.id.clone(),
                url,
                title: tab.title,
            })
        }));
    summary
        .extensions
        .extend(read.data.extensions.drain(..).map(|extension| {
            ImportedExtension {
                profile_id: profile.id.clone(),
                id: extension.id,
                name: extension.name,
                store_url: extension
                    .store_url
                    .and_then(|url| openside_import::sanitize_import_url(&url)),
            }
        }));
    summary
        .search_engines
        .extend(read.data.search_engines.drain(..).filter_map(|engine| {
            let url = openside_import::sanitize_import_url(&engine.url)?;
            Some(ImportedSearchEngine {
                profile_id: profile.id.clone(),
                name: engine.name,
                keyword: engine.keyword,
                url,
            })
        }));
    summary
        .permissions
        .extend(
            read.data
                .permissions
                .drain(..)
                .map(|permission| ImportedPermission {
                    profile_id: profile.id.clone(),
                    origin: permission.origin,
                    permission: permission.permission,
                    setting: permission.setting,
                }),
        );
    summary
        .profiles
        .push(profile_report(profile, &mut read, summary.dry_run));
}

fn summary(preview: bool) -> ImportBatchReport {
    ImportBatchReport {
        dry_run: preview,
        profile: DEFAULT_PROFILE.to_owned(),
        totals: DataKind::ALL
            .into_iter()
            .map(|kind| (category(kind), 0))
            .collect(),
        ..ImportBatchReport::default()
    }
}

fn prepare_cookie_reads(
    profiles: &[DiscoveredProfile],
    requested: bool,
    preview: bool,
) -> (Option<Vec<ProfileRead>>, Vec<String>) {
    if !requested {
        return (None, Vec::new());
    }
    let mut reads: Vec<_> = profiles
        .iter()
        .map(|profile| {
            openside_import::read_profile(
                profile,
                &[DataKind::Cookies],
                KeyringSource::System {
                    brand: profile.brand.clone(),
                },
                preview,
            )
        })
        .collect();
    let sites = if preview {
        Vec::new()
    } else {
        openside_import::select_recent_cookies(&mut reads, chrono::Utc::now().timestamp())
    };
    (Some(reads), sites)
}

fn add_cookie_warnings(summary: &mut ImportBatchReport, sites: Vec<String>) {
    summary.warnings.extend(sites.into_iter().map(|site| format!("Matching cookies for {site} were found in multiple profiles. The most recently used ones will be kept.")));
}

pub(crate) async fn preview(
    State(shared): State<Shared>,
    Extension(local): Extension<LocalAuthority>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<ImportRequest>,
) -> Result<Json<ImportBatchReport>, HttpError> {
    local.require()?;
    let (profiles, kinds) = selection(request).await?;
    let mut summary = tokio::task::spawn_blocking(move || {
        let mut kinds = kinds;
        let (mut planned, sites) =
            prepare_cookie_reads(&profiles, kinds.contains(&DataKind::Cookies), true);
        kinds.retain(|kind| *kind != DataKind::Cookies);
        let mut summary = summary(true);
        add_cookie_warnings(&mut summary, sites);
        for (index, profile) in profiles.into_iter().enumerate() {
            let mut read = openside_import::read_profile(
                &profile,
                &kinds,
                KeyringSource::System {
                    brand: profile.brand.clone(),
                },
                true,
            );
            if let Some(planned) = planned.as_mut() {
                read.data.cookies = std::mem::take(&mut planned[index].data.cookies);
                read.reports.append(&mut planned[index].reports);
            }
            add_profile(&mut summary, &profile, read);
        }
        summary
    })
    .await
    .map_err(|_| HttpError::internal())?;
    let (needed, locked) = vault_flags(&shared).await;
    let passwords = summary
        .totals
        .get(&ImportCategory::Passwords)
        .copied()
        .unwrap_or_default()
        > 0;
    summary.vault_needed = passwords && needed;
    summary.vault_locked = passwords && locked;
    summary.warnings.sort();
    summary.warnings.dedup();
    crate::pairing::audit(
        &shared,
        "import",
        "previewed",
        json!({"device_id":caller.0,"profiles":summary.profiles.len(),"totals":summary.totals}),
    )
    .await;
    Ok(Json(summary))
}

async fn require_vault(shared: &Shared) -> Result<(), HttpError> {
    let (needed, locked) = vault_flags(shared).await;
    if needed {
        Err(HttpError(StatusCode::CONFLICT, "vault_setup_required"))
    } else if locked {
        Err(HttpError(StatusCode::CONFLICT, "vault_locked"))
    } else {
        Ok(())
    }
}

async fn save_passwords(
    shared: &Shared,
    caller: &Caller,
    passwords: &mut Vec<SavedPassword>,
    operation: &str,
) -> CategoryReport {
    let mut report = CategoryReport {
        read: u64::try_from(passwords.len()).unwrap_or(u64::MAX),
        ..CategoryReport::default()
    };
    let who = caller
        .0
        .as_ref()
        .map_or_else(|| "local".to_owned(), |device| format!("device:{device}"));
    let audit = AuditContext {
        who: &who,
        session_id: None,
        tool_call_id: None,
    };
    let mut guard = shared.registry.vault.vault.lock().await;
    let Some(vault) = guard.as_mut() else {
        report.skipped = report.read;
        report.note = Some(
            "Unlock your openside Vault and try again. Other data can still be imported."
                .to_owned(),
        );
        passwords.clear();
        return report;
    };
    for mut login in passwords.drain(..) {
        let origin = match url::Url::parse(&login.origin) {
            Ok(url)
                if url.username().is_empty()
                    && url.password().is_none()
                    && matches!(url.scheme(), "http" | "https") =>
            {
                url.origin().ascii_serialization()
            }
            _ => {
                report.skipped += 1;
                continue;
            }
        };
        let username = Secret::new(std::mem::take(&mut *login.username));
        let password = Secret::new(std::mem::take(&mut *login.password));
        shared
            .registry
            .redactors
            .write()
            .push(Redactor::new([&username, &password]));
        match vault.save_login(&audit, &origin, username, password, operation) {
            Ok(Ok(_)) | Err(openside_vault::Error::AuditIncomplete) => {
                report.imported += 1;
            }
            _ => {
                report.skipped += 1;
                report.note = Some("Some passwords could not be saved in the Vault. Check the Vault's access policy and try again.".to_owned());
            }
        }
    }
    report
}

fn jobs_dir(shared: &Shared) -> PathBuf {
    shared.config.data_dir.join("import-jobs")
}
fn job_path(shared: &Shared, id: &str) -> Result<PathBuf, HttpError> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(HttpError(StatusCode::NOT_FOUND, "unknown_import_job"));
    }
    Ok(jobs_dir(shared).join(format!("{id}.json")))
}
fn persist_job(shared: &Shared, stored: &StoredJob) -> Result<(), HttpError> {
    use std::io::Write;
    let directory = jobs_dir(shared);
    fs::create_dir_all(&directory).map_err(|_| HttpError::internal())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| HttpError::internal())?;
    }
    let mut file =
        tempfile::NamedTempFile::new_in(&directory).map_err(|_| HttpError::internal())?;
    {
        let mut out = std::io::BufWriter::new(file.as_file_mut());
        serde_json::to_writer(&mut out, stored).map_err(|_| HttpError::internal())?;
        out.flush().map_err(|_| HttpError::internal())?;
    }
    file.as_file()
        .sync_all()
        .map_err(|_| HttpError::internal())?;
    file.persist(job_path(shared, &stored.job.id)?)
        .map_err(|_| HttpError::internal())?;
    Ok(())
}
fn progress(stored: &mut StoredJob) {
    stored.events.push(ImportProgress {
        job_id: stored.job.id.clone(),
        state: stored.job.state,
        completed_profiles: stored.job.completed_profiles,
        total_profiles: stored.job.total_profiles,
        current_profile: stored.job.current_profile.clone(),
    });
}
fn load_job(shared: &Shared, id: &str) -> Result<StoredJob, HttpError> {
    let path = job_path(shared, id)?;
    let file = fs::File::open(&path)
        .map_err(|_| HttpError(StatusCode::NOT_FOUND, "unknown_import_job"))?;
    use std::io::Read;
    let mut stored: StoredJob =
        serde_json::from_reader(file.take(32 * 1024 * 1024)).map_err(|_| HttpError::internal())?;
    if stored.job.state == ImportJobState::Running
        && !active()
            .lock()
            .get(&shared.config.data_dir)
            .is_some_and(|active| active == id)
    {
        stored.job.state = ImportJobState::Failed;
        stored.job.error = Some("import_interrupted".to_owned());
        progress(&mut stored);
        persist_job(shared, &stored)?;
    }
    Ok(stored)
}

pub(crate) async fn run(
    State(shared): State<Shared>,
    Extension(local): Extension<LocalAuthority>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<ImportRequest>,
) -> Result<(StatusCode, Json<ImportJob>), HttpError> {
    local.require()?;
    let (profiles, kinds) = selection(request).await?;
    if kinds.contains(&DataKind::Passwords) {
        let sources = profiles
            .iter()
            .map(DiscoveredProfile::source)
            .collect::<Vec<_>>();
        let has_passwords = tokio::task::spawn_blocking(move || {
            sources
                .iter()
                .any(|source| openside_import::password_count(source).unwrap_or_default() > 0)
        })
        .await
        .map_err(|_| HttpError::internal())?;
        if has_passwords {
            require_vault(&shared).await?;
        }
    }
    let id = crate::persistence::random_id();
    let running = Running::start(&shared.config.data_dir, &id)?;
    let mut stored = StoredJob {
        job: ImportJob {
            id: id.clone(),
            state: ImportJobState::Running,
            completed_profiles: 0,
            total_profiles: u64::try_from(profiles.len()).unwrap_or(u64::MAX),
            current_profile: None,
            summary: None,
            error: None,
        },
        events: Vec::new(),
    };
    progress(&mut stored);
    persist_job(&shared, &stored)?;
    let response = stored.job.clone();
    tokio::spawn(async move {
        let _running = running;
        let mut summary = summary(false);
        let target = ProfileLayout::new(&shared.config.data_dir, DEFAULT_PROFILE);
        let outcome = async {
            let target = target.map_err(|_| HttpError::internal())?;
            let (profiles, mut planned, sites, mut kinds) =
                tokio::task::spawn_blocking(move || {
                    let mut kinds = kinds;
                    let (planned, sites) =
                        prepare_cookie_reads(&profiles, kinds.contains(&DataKind::Cookies), false);
                    kinds.retain(|kind| *kind != DataKind::Cookies);
                    (profiles, planned, sites, kinds)
                })
                .await
                .map_err(|_| HttpError::internal())?;
            add_cookie_warnings(&mut summary, sites);
            for (index, profile) in profiles.into_iter().enumerate() {
                if caller
                    .0
                    .as_ref()
                    .is_some_and(|device| shared.remote.is_revoked(device))
                {
                    return Err(HttpError(StatusCode::UNAUTHORIZED, "unauthorized"));
                }
                stored.job.current_profile = Some(profile.id.clone());
                progress(&mut stored);
                persist_job(&shared, &stored)?;
                let (profile, mut read, next_kinds) = tokio::task::spawn_blocking(move || {
                    let read = openside_import::read_profile(
                        &profile,
                        &kinds,
                        KeyringSource::System {
                            brand: profile.brand.clone(),
                        },
                        false,
                    );
                    (profile, read, kinds)
                })
                .await
                .map_err(|_| HttpError::internal())?;
                kinds = next_kinds;
                if let Some(planned) = planned.as_mut() {
                    read.data.cookies = std::mem::take(&mut planned[index].data.cookies);
                    read.reports.append(&mut planned[index].reports);
                }
                if read
                    .reports
                    .get(&DataKind::Passwords)
                    .is_some_and(|report| report.note.is_none())
                {
                    let report =
                        save_passwords(&shared, &caller, &mut read.data.passwords, &id).await;
                    read.reports.insert(DataKind::Passwords, report);
                }
                openside_import::write_profile(&target, &profile, &read).map_err(|_| {
                    HttpError(StatusCode::INTERNAL_SERVER_ERROR, "import_write_failed")
                })?;
                crate::import_delivery::stage(&shared, &profile, &mut read).await?;
                add_profile(&mut summary, &profile, read);
                stored.job.completed_profiles += 1;
                progress(&mut stored);
                persist_job(&shared, &stored)?;
            }
            Ok::<(), HttpError>(())
        }
        .await;
        stored.job.state = if outcome.is_ok() {
            ImportJobState::Completed
        } else {
            ImportJobState::Failed
        };
        stored.job.current_profile = None;
        stored.job.error = outcome.err().map(|error| error.1.to_owned());
        summary.warnings.sort();
        summary.warnings.dedup();
        stored.job.summary = Some(summary);
        progress(&mut stored);
        if persist_job(&shared, &stored).is_err() {
            crate::diag!("opensided: import job state could not be persisted");
        }
        crate::pairing::audit(&shared, "import", if stored.job.state == ImportJobState::Completed { "imported" } else { "failed" }, json!({"job_id":id,"device_id":caller.0,"completed_profiles":stored.job.completed_profiles,"total_profiles":stored.job.total_profiles})).await;
    });
    Ok((StatusCode::ACCEPTED, Json(response)))
}

pub(crate) async fn job(
    State(shared): State<Shared>,
    Extension(local): Extension<LocalAuthority>,
    Path(id): Path<String>,
) -> Result<Json<ImportJob>, HttpError> {
    local.require()?;
    let stored = tokio::task::spawn_blocking(move || load_job(&shared, &id))
        .await
        .map_err(|_| HttpError::internal())??;
    Ok(Json(stored.job))
}

pub(crate) async fn events(
    State(shared): State<Shared>,
    Extension(local): Extension<LocalAuthority>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, HttpError> {
    local.require()?;
    load_job(&shared, &id)?;
    let mut after = headers
        .get("Last-Event-ID")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    let stream = async_stream::stream! {
        let mut interval = tokio::time::interval(Duration::from_millis(300));
        loop {
            interval.tick().await;
            let loaded = tokio::task::spawn_blocking({ let shared = shared.clone(); let id = id.clone(); move || load_job(&shared, &id) }).await;
            let Ok(Ok(stored)) = loaded else { break; };
            for (index, progress) in stored.events.iter().enumerate().skip(after) {
                if let Ok(event) = Event::default().event("import_progress").id((index + 1).to_string()).json_data(progress) { yield Ok(event); }
                after = index + 1;
            }
            if stored.job.state != ImportJobState::Running { break; }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

pub(crate) async fn passwords(
    State(shared): State<Shared>,
    Extension(local): Extension<LocalAuthority>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<ImportPasswordsRequest>,
) -> Result<Json<ImportCategoryReport>, HttpError> {
    local.require()?;
    require_vault(&shared).await?;
    let csv = zeroize::Zeroizing::new(request.csv);
    let mut parsed = tokio::task::spawn_blocking(move || openside_import::parse_password_csv(&csv))
        .await
        .map_err(|_| HttpError::internal())?
        .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_password_csv"))?;
    let id = crate::persistence::random_id();
    let _running = Running::start(&shared.config.data_dir, &id)?;
    let mut report = save_passwords(&shared, &caller, &mut parsed.passwords, &id).await;
    report.read = parsed.read;
    report.skipped += parsed.skipped;
    Ok(Json(report_category(report)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_family_summary_urls_and_url_derived_titles_exclude_credentials() {
        let canaries = [
            "URL_PASSWORD_CANARY",
            "URL_QUERY_CANARY",
            "URL_FRAGMENT_CANARY",
        ];
        let address = "https://person:URL_PASSWORD_CANARY@example.test/path?access_token=URL_QUERY_CANARY&q=kept#token=URL_FRAGMENT_CANARY";
        for (brand, kind) in [
            ("chrome", openside_import::BrowserKind::Chrome),
            ("firefox", openside_import::BrowserKind::Firefox),
            ("safari", openside_import::BrowserKind::Safari),
            ("arc", openside_import::BrowserKind::Chromium),
        ] {
            let profile = DiscoveredProfile {
                id: brand.to_owned(),
                kind,
                brand: brand.to_owned(),
                name: brand.to_owned(),
                email: None,
                avatar: None,
                dir: std::path::PathBuf::new(),
                available: openside_import::Available::default(),
                warnings: Vec::new(),
            };
            let mut read = ProfileRead::default();
            read.data.tabs.push(openside_import::RestoreTab {
                url: address.to_owned(),
                title: address.to_owned(),
            });
            read.data
                .search_engines
                .push(openside_import::SearchEngine {
                    name: "Search".to_owned(),
                    keyword: "s".to_owned(),
                    url: address.to_owned(),
                });
            read.data
                .extensions
                .push(openside_import::ExtensionMetadata {
                    id: "fixture".to_owned(),
                    name: "Fixture".to_owned(),
                    store_url: Some(address.to_owned()),
                });
            let mut summary = summary(true);
            add_profile(&mut summary, &profile, read);
            let json = serde_json::to_string(&summary).unwrap();
            for canary in canaries {
                assert!(!json.contains(canary), "{brand} leaked {canary}");
            }
            assert!(summary.restore_tabs[0].url.contains("q=kept"));
            assert_eq!(summary.restore_tabs[0].title, summary.restore_tabs[0].url);
        }
    }
}
