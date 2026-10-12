//! `GET /v1/import/profiles` and `POST /v1/import` (F-5): browser data import for the
//! first-run guide, over `openside-import`.
//!
//! The request names a profile by the id this module hands out, never by a path, so the API
//! cannot be used to probe or read arbitrary directories. Cookies encrypted with the OS
//! keyring need a password command, which only the CLI accepts (a command line from the
//! network would be remote code execution). Error codes are fixed strings: the importer's
//! messages name local file paths, which stay in the daemon's own log.

use std::sync::atomic::{AtomicBool, Ordering};

use axum::{extract::State, http::StatusCode, Extension, Json};
use openside_import::{
    BrowserKind, CategoryReport, ImportError, ImportOptions, KeyringSource, ProfileLayout, Report,
    What, DEFAULT_PROFILE,
};
use openside_proto::{
    ImportAvailability, ImportBrowser, ImportCategory, ImportCategoryReport, ImportProfile,
    ImportProfileList, ImportRejectedCookie, ImportReport, ImportRequest,
};
use serde_json::json;

use crate::{
    api::{Caller, HttpError},
    engine::Shared,
};

/// One import at a time: two would race on the profile's cookie store and history files.
static RUNNING: AtomicBool = AtomicBool::new(false);

struct Running;

impl Running {
    fn start() -> Option<Self> {
        (!RUNNING.swap(true, Ordering::AcqRel)).then_some(Self)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Release);
    }
}

fn browser(kind: BrowserKind) -> ImportBrowser {
    match kind {
        BrowserKind::Chrome => ImportBrowser::Chrome,
        BrowserKind::Chromium => ImportBrowser::Chromium,
        BrowserKind::Firefox => ImportBrowser::Firefox,
    }
}

async fn discovered() -> Result<Vec<openside_import::DiscoveredProfile>, HttpError> {
    tokio::task::spawn_blocking(openside_import::discover_system)
        .await
        .map_err(|_| HttpError::internal())
}

pub(crate) async fn profiles(
    State(s): State<Shared>,
) -> Result<Json<ImportProfileList>, HttpError> {
    let profiles = discovered()
        .await?
        .into_iter()
        .map(|profile| ImportProfile {
            id: profile.id,
            browser: browser(profile.kind),
            name: profile.name,
            available: ImportAvailability {
                history: profile.available.history,
                bookmarks: profile.available.bookmarks,
                cookies: profile.available.cookies,
            },
        })
        .collect();
    Ok(Json(ImportProfileList {
        profiles,
        cookies_writable: openside_config::discover_browser(&s.config.loaded.config).is_ok(),
    }))
}

fn error(error: &ImportError) -> HttpError {
    use ImportError::*;
    match error {
        MissingFile { .. } | Unreadable { .. } => {
            HttpError(StatusCode::UNPROCESSABLE_ENTITY, "source_unreadable")
        }
        Locked { .. } => HttpError(StatusCode::UNPROCESSABLE_ENTITY, "source_locked"),
        Corrupt { .. } | CorruptCookie { .. } => {
            HttpError(StatusCode::UNPROCESSABLE_ENTITY, "source_corrupt")
        }
        UnsupportedVersion { .. } | UnknownCookieVersion { .. } => {
            HttpError(StatusCode::UNPROCESSABLE_ENTITY, "source_unsupported")
        }
        KeyringPasswordRequired { .. } | WrongKeyringPassword { .. } | KeyringCommand(_) => {
            HttpError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "keyring_password_required",
            )
        }
        BrowserRequired(_) => HttpError(StatusCode::SERVICE_UNAVAILABLE, "browser_unavailable"),
        ProfileInUse(_) => HttpError(StatusCode::CONFLICT, "profile_in_use"),
        InvalidSource(_) | InvalidProfileName(_) | Browser(_) | Write { .. } => {
            crate::diag!("opensided: import failed: {error}");
            HttpError(StatusCode::INTERNAL_SERVER_ERROR, "import_failed")
        }
    }
}

fn category(report: Option<CategoryReport>) -> Option<ImportCategoryReport> {
    report.map(|report| ImportCategoryReport {
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
        // The importer's note names the missing file's path.
        note: report.note.map(|_| "Not found in this profile.".to_owned()),
    })
}

fn counts(report: &Option<ImportCategoryReport>) -> u64 {
    report.as_ref().map_or(0, |report| report.imported)
}

pub(crate) async fn run(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<ImportRequest>,
) -> Result<Json<ImportReport>, HttpError> {
    let profile = discovered()
        .await?
        .into_iter()
        .find(|profile| profile.id == request.profile)
        .ok_or(HttpError(StatusCode::BAD_REQUEST, "unknown_profile"))?;
    let what = match &request.what {
        None => What::ALL,
        Some(list) if list.is_empty() => return Err(HttpError::invalid_value()),
        Some(list) => What {
            history: list.contains(&ImportCategory::History),
            bookmarks: list.contains(&ImportCategory::Bookmarks),
            cookies: list.contains(&ImportCategory::Cookies),
        },
    };
    let target = ProfileLayout::new(&s.config.data_dir, DEFAULT_PROFILE).map_err(|e| error(&e))?;
    let options = ImportOptions {
        source: profile.source(),
        target,
        what,
        // As the CLI: categories left to default may be absent from the profile.
        skip_missing: request.what.is_none(),
        dry_run: request.dry_run,
        keyring: KeyringSource::None,
        browser: openside_config::discover_browser(&s.config.loaded.config).ok(),
    };
    let running = Running::start().ok_or(HttpError(StatusCode::CONFLICT, "import_in_progress"))?;
    // Its own task: a client that gives up must not leave cookies half-applied or release
    // the single-import guard while the browser is still writing.
    let report: Report = tokio::spawn(async move {
        let _running = running;
        openside_import::import(options).await
    })
    .await
    .map_err(|_| HttpError::internal())?
    .map_err(|e| error(&e))?;
    let report = ImportReport {
        dry_run: report.dry_run,
        source: browser(profile.kind),
        profile: report.profile,
        history: category(report.history),
        bookmarks: category(report.bookmarks),
        cookies: category(report.cookies),
    };
    crate::pairing::audit(
        &s,
        "import",
        if report.dry_run {
            "previewed"
        } else {
            "imported"
        },
        json!({
            "device_id": caller.0,
            "source": profile.id,
            "history": counts(&report.history),
            "bookmarks": counts(&report.bookmarks),
            "cookies": counts(&report.cookies),
        }),
    )
    .await;
    Ok(Json(report))
}
