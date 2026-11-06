//! The person's own browser, reached through the person-role gateway that `openside-browser`
//! starts it with: its settings and native tools only, never its tabs. The daemon never launches
//! it; when it is closed there is nothing to reach.

use std::{path::PathBuf, sync::Arc, time::Duration};

use openside_browser::Gateway;
use serde::Deserialize;
use tokio::sync::Mutex;

/// Under the daemon's runtime directory; `tools/openside-browser` passes it to the browser as its
/// gateway runtime directory and records each launch there.
pub(crate) const RUNTIME_SUBDIR: &str = "person-browser";

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// `{pid, patched}` written by `tools/openside-browser` for the browser it execs.
const LAUNCH_RECORD: &str = "launch.json";
/// The launch switch only a browser started by `openside-browser` carries; a reused pid lacks it.
const PERSON_ROLE_ARG: &[u8] = b"--openside-gateway-role=person";

/// What the daemon can reach of the person's own browser.
pub(crate) enum PersonBrowser {
    /// A patched browser with its person-role gateway.
    Gateway(Arc<Gateway>),
    /// A browser `openside-browser` started from a stock build: running, but with no gateway.
    Stock,
    /// Closed, or a patched browser whose gateway is not up.
    NotRunning,
}

#[derive(Deserialize)]
struct LaunchRecord {
    pid: u32,
    patched: bool,
}

pub(crate) struct PersonGateway {
    runtime_dir: PathBuf,
    current: Mutex<Option<Arc<Gateway>>>,
}

impl PersonGateway {
    pub(crate) fn new(runtime_dir: PathBuf) -> Self {
        Self {
            runtime_dir,
            current: Mutex::new(None),
        }
    }

    /// The one place the person's browser publishes; `openside-browser` learns it from
    /// `/v1/remote/local-access`.
    pub(crate) fn runtime_dir(&self) -> &std::path::Path {
        &self.runtime_dir
    }

    /// The connected person gateway, reconnecting after the browser restarted. Without one, the
    /// launch record tells a running stock browser from a closed one.
    pub(crate) async fn get(&self) -> PersonBrowser {
        let mut current = self.current.lock().await;
        if let Some(gateway) = current.as_ref().filter(|gateway| gateway.is_connected()) {
            return PersonBrowser::Gateway(gateway.clone());
        }
        *current = Gateway::connect_person(&self.runtime_dir, COMMAND_TIMEOUT)
            .await
            .ok()
            .flatten()
            .map(Arc::new);
        match current.as_ref() {
            Some(gateway) => PersonBrowser::Gateway(gateway.clone()),
            None if self.stock_browser_running() => PersonBrowser::Stock,
            None => PersonBrowser::NotRunning,
        }
    }

    fn stock_browser_running(&self) -> bool {
        let Some(record) = self.launch_record() else {
            return false;
        };
        !record.patched && launched_browser_alive(record.pid)
    }

    fn launch_record(&self) -> Option<LaunchRecord> {
        let path = self.runtime_dir.join(LAUNCH_RECORD);
        let metadata = std::fs::symlink_metadata(&path).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // Written by this user's launcher, private: anything else is not its record.
            if !metadata.is_file()
                || metadata.uid() != nix::unistd::geteuid().as_raw()
                || metadata.mode() & 0o077 != 0
                || metadata.len() > 4096
            {
                return None;
            }
        }
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }
}

#[cfg(target_os = "linux")]
fn launched_browser_alive(pid: u32) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|cmdline| {
        cmdline
            .split(|byte| *byte == 0)
            .any(|arg| arg == PERSON_ROLE_ARG)
    })
}

#[cfg(not(target_os = "linux"))]
fn launched_browser_alive(_pid: u32) -> bool {
    false
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn record(dir: &std::path::Path, pid: u32, patched: bool) {
        let path = dir.join(LAUNCH_RECORD);
        std::fs::write(&path, format!(r#"{{"pid":{pid},"patched":{patched}}}"#)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn state(gateway: PersonBrowser) -> &'static str {
        match gateway {
            PersonBrowser::Gateway(_) => "gateway",
            PersonBrowser::Stock => "stock",
            PersonBrowser::NotRunning => "not_running",
        }
    }

    #[tokio::test]
    async fn the_launch_record_tells_a_stock_browser_from_a_closed_one() {
        let dir = tempfile::tempdir().unwrap();
        let person = PersonGateway::new(dir.path().to_owned());
        assert_eq!(state(person.get().await), "not_running");

        // A process started the way the launcher starts the browser.
        let mut browser = std::process::Command::new("sh")
            // A loop, not one command: the shell would exec it and lose the argument.
            .args([
                "-c",
                "while :; do sleep 1; done",
                "sh",
                "--openside-gateway-role=person",
            ])
            .spawn()
            .unwrap();
        record(dir.path(), browser.id(), false);
        assert_eq!(state(person.get().await), "stock");
        // A patched browser without its gateway yet is not running for settings, not stock.
        record(dir.path(), browser.id(), true);
        assert_eq!(state(person.get().await), "not_running");
        // A record readable by others is not trusted.
        record(dir.path(), browser.id(), false);
        std::fs::set_permissions(
            dir.path().join(LAUNCH_RECORD),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert_eq!(state(person.get().await), "not_running");
        record(dir.path(), browser.id(), false);
        browser.kill().unwrap();
        browser.wait().unwrap();
        assert_eq!(state(person.get().await), "not_running");
        // A live process that is not the launched browser (a reused pid) does not count.
        record(dir.path(), std::process::id(), false);
        assert_eq!(state(person.get().await), "not_running");
    }
}
