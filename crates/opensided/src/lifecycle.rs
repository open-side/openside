//! Controlled restarts re-exec the same PID after graceful shutdown, including under a supervisor.
use crate::{api::HttpError, engine::Shared, remote::LocalAuthority};
use axum::{extract::State, http::StatusCode, Extension, Json};
use chrono::{DateTime, Utc};
use openside_proto::{DaemonStatus, RestartDaemonResponse, RestartMethod};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    ffi::OsString,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
use zeroize::Zeroizing;

const MARKER: &str = "OPENSIDE_CONTROLLED_RESTART";
const HANDOFF_FILE: &str = openside_config::data_layout::DataPath::RestartHandoff.relative();
const TTL: u64 = 300;
static START_MARKER: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

/// Must run before constructing the runtime or starting any threads.
/// # Safety
/// Call before constructing the runtime or starting any threads.
#[allow(unsafe_code)]
pub unsafe fn capture_restart_marker() {
    let marker = std::env::var(MARKER).ok();
    // The executable invokes this before creating any threads; children must never inherit it.
    unsafe {
        std::env::remove_var(MARKER);
    }
    *START_MARKER.lock() = marker;
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) struct Lifecycle {
    boot_id: String,
    request_id: String,
    started_at: DateTime<Utc>,
    argv: Vec<OsString>,
    restarting: AtomicBool,
    reexec: watch::Sender<bool>,
}
impl Lifecycle {
    pub(crate) fn new() -> Self {
        Self {
            boot_id: crate::persistence::random_id(),
            request_id: crate::persistence::random_id(),
            started_at: Utc::now(),
            argv: std::env::args_os().collect(),
            restarting: AtomicBool::new(false),
            reexec: watch::channel(false).0,
        }
    }
    pub(crate) fn status(&self) -> DaemonStatus {
        DaemonStatus {
            version: env!("CARGO_PKG_VERSION").into(),
            boot_id: self.boot_id.clone(),
            started_at: self.started_at,
            restart_method: RestartMethod::ReExec,
        }
    }
    pub(crate) async fn reexec_requested(&self) {
        let mut requested = self.reexec.subscribe();
        let _ = requested.wait_for(|value| *value).await;
    }
    fn marker(&self) -> String {
        format!("{}:{}", self.boot_id, self.request_id)
    }
    #[cfg(unix)]
    pub(crate) fn exec(&self) -> std::io::Error {
        use std::os::unix::process::CommandExt;
        let Ok(program) = std::env::current_exe() else {
            return std::io::Error::other("daemon executable unavailable");
        };
        std::process::Command::new(program)
            .args(&self.argv[1..])
            .env(MARKER, self.marker())
            .exec()
    }
    #[cfg(not(unix))]
    pub(crate) fn exec(&self) -> std::io::Error {
        std::io::Error::other("restart unsupported")
    }
}

#[derive(Serialize, Deserialize)]
struct Handoff {
    secret: String,
    boot_id: String,
    request_id: String,
    created: u64,
}

#[cfg(unix)]
fn private(meta: &std::fs::Metadata, mode: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    private_owner_mode(meta.uid(), meta.mode(), mode)
}

#[cfg(unix)]
fn private_owner_mode(owner: u32, actual_mode: u32, expected_mode: u32) -> bool {
    owner == nix::unistd::getuid().as_raw() && actual_mode & 0o777 == expected_mode
}

pub(crate) fn write_handoff(
    dir: &Path,
    secret: &str,
    lifecycle: &Lifecycle,
) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    if !private(&std::fs::metadata(dir)?, 0o700) {
        return Err(std::io::Error::other("unsafe data directory"));
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(dir.join(HANDOFF_FILE))?;
    let bytes = Zeroizing::new(serde_json::to_vec(&Handoff {
        secret: secret.into(),
        boot_id: lifecycle.boot_id.clone(),
        request_id: lifecycle.request_id.clone(),
        created: now(),
    })?);
    file.write_all(&bytes)?;
    file.sync_all()
}

pub(crate) fn take_handoff(dir: &Path) -> Option<Zeroizing<String>> {
    take_marked_handoff(dir, START_MARKER.lock().take().as_deref(), now())
}

fn take_marked_handoff(
    dir: &Path,
    marker: Option<&str>,
    started: u64,
) -> Option<Zeroizing<String>> {
    use std::io::Read;
    let path = dir.join(HANDOFF_FILE);
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    let opened = options.open(&path);
    // Unlink before parsing, even on ordinary boot or a malformed handoff.
    let _ = std::fs::remove_file(&path);
    let file = opened.ok()?;
    let marker = marker?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > 4096 {
        return None;
    }
    #[cfg(unix)]
    if !private(&meta, 0o600) || !private(&std::fs::metadata(dir).ok()?, 0o700) {
        return None;
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(4097).read_to_end(&mut bytes).ok()?;
    let document: Handoff = serde_json::from_slice(&bytes).ok()?;
    let secret = Zeroizing::new(document.secret);
    if marker != format!("{}:{}", document.boot_id, document.request_id)
        || started < document.created
        || started - document.created >= TTL
        || secret.len() != 64
        || !secret.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(secret)
}

pub(crate) async fn status(State(s): State<Shared>) -> Json<DaemonStatus> {
    Json(s.ops.lifecycle.status())
}

pub(crate) async fn restart(
    State(s): State<Shared>,
    Extension(local): Extension<LocalAuthority>,
) -> Result<(StatusCode, Json<RestartDaemonResponse>), HttpError> {
    local.require()?;
    // Once admitted, disconnecting cannot strand a handoff without requesting the exec.
    tokio::spawn(restart_owned(s))
        .await
        .map_err(|_| HttpError::internal())?
}

async fn restart_owned(s: Shared) -> Result<(StatusCode, Json<RestartDaemonResponse>), HttpError> {
    let lifecycle = &s.ops.lifecycle;
    if lifecycle.restarting.swap(true, Ordering::SeqCst) {
        return Err(HttpError(StatusCode::CONFLICT, "restart_in_progress"));
    }
    crate::pairing::audit(
        &s,
        "daemon",
        "restart_requested",
        json!({"boot_id": lifecycle.boot_id, "request_id": lifecycle.request_id}),
    )
    .await;
    let shared = s.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        shared.ops.lifecycle.reexec.send_replace(true);
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(RestartDaemonResponse {
            method: RestartMethod::ReExec,
            boot_id: lifecycle.boot_id.clone(),
        }),
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    #[test]
    fn handoff_rejects_wrong_owner_even_when_mode_is_private() {
        let uid = nix::unistd::getuid().as_raw();
        assert!(private_owner_mode(uid, 0o100600, 0o600));
        assert!(!private_owner_mode(uid.wrapping_add(1), 0o100600, 0o600));
        assert!(!private_owner_mode(uid, 0o100640, 0o600));
    }
    #[test]
    fn handoff_requires_provenance_private_file_and_fresh_single_use() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let lifecycle = Lifecycle::new();
        let secret = "a".repeat(64);
        let path = dir.path().join(HANDOFF_FILE);
        for (marker, age, mode, accepted) in [
            (Some(lifecycle.marker()), 0, 0o600, true),
            (Some(lifecycle.marker()), TTL, 0o600, false),
            (Some("wrong:request".into()), 0, 0o600, false),
            (None, 0, 0o600, false),
            (Some(lifecycle.marker()), 0, 0o640, false),
        ] {
            write_handoff(dir.path(), &secret, &lifecycle).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert_eq!(
                take_marked_handoff(dir.path(), marker.as_deref(), now() + age).is_some(),
                accepted
            );
            assert!(!path.exists());
            assert!(take_marked_handoff(dir.path(), marker.as_deref(), now()).is_none());
        }
        let target = dir.path().join("target");
        std::fs::write(&target, "untouched").unwrap();
        symlink(&target, &path).unwrap();
        assert!(write_handoff(dir.path(), &secret, &lifecycle).is_err());
        assert!(take_marked_handoff(dir.path(), Some(&lifecycle.marker()), now()).is_none());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "untouched");
    }
}
