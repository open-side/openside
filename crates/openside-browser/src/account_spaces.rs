//! Persistent opt-in profiles. Running instances are deliberately never restored.
use crate::{Browser, BrowserConfig, BrowserError, BrowserEvent};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, Mutex};

/// An unused profile is closed after ten minutes without owned tabs.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(600);
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;
struct Running {
    site: String,
    browser: Arc<Browser>,
    used: Instant,
}
/// Serializes profile launches, replacement, idle shutdown and deletion.
pub struct AccountSpaces {
    running: Mutex<HashMap<String, Running>>,
    clock: Clock,
}
impl Default for AccountSpaces {
    fn default() -> Self {
        Self::with_clock(Arc::new(Instant::now))
    }
}
impl AccountSpaces {
    /// Inject a monotonic clock so lifecycle deadlines can be tested without sleeping.
    pub fn with_clock(clock: Clock) -> Self {
        Self {
            running: Mutex::new(HashMap::new()),
            clock,
        }
    }
    /// Validate the untrusted identifier before resolving a child profile path.
    pub fn profile(root: &Path, id: &str) -> Result<PathBuf, BrowserError> {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(BrowserError::InvalidEndpoint(
                "Invalid account space identifier",
            ));
        }
        Ok(root.join(id))
    }
    /// Launch on demand, closing any other account space for this site first.
    pub async fn open(
        &self,
        root: &Path,
        id: &str,
        site: &str,
        mut config: BrowserConfig,
    ) -> Result<(Arc<Browser>, Option<mpsc::UnboundedReceiver<BrowserEvent>>), BrowserError> {
        let directory = Self::profile(root, id)?;
        let mut running = self.running.lock().await;
        if let Some(entry) = running.get_mut(id) {
            if entry.site != site {
                return Err(BrowserError::InvalidEndpoint("Account space site mismatch"));
            }
            if entry.browser.is_connected() {
                entry.used = (self.clock)();
                return Ok((entry.browser.clone(), None));
            }
        }
        let replaced: Vec<_> = running
            .iter()
            .filter(|(key, entry)| entry.site == site || key.as_str() == id)
            .map(|(key, _)| key.clone())
            .collect();
        for key in replaced {
            if let Some(entry) = running.get(&key) {
                terminate(&entry.browser).await?;
            }
            running.remove(&key);
        }
        if let Some(parent) = root.parent() {
            private_directory(parent).await?;
        }
        private_directory(root).await?;
        private_directory(&directory).await?;
        config.profile_dir = Some(directory);
        config.detached = false;
        config.gateway = None;
        let (browser, events) = Browser::launch(config).await?;
        let browser = Arc::new(browser);
        running.insert(
            id.to_owned(),
            Running {
                site: site.to_owned(),
                browser: browser.clone(),
                used: (self.clock)(),
            },
        );
        Ok((browser, Some(events)))
    }
    /// Route only an owned tab, refreshing its tool-use deadline.
    pub async fn for_tab(&self, session: &str, tab: &str) -> Option<Arc<Browser>> {
        let mut running = self.running.lock().await;
        for entry in running.values_mut() {
            if entry
                .browser
                .tabs()
                .await
                .iter()
                .any(|item| item.id == tab && item.owner_session_id.as_deref() == Some(session))
            {
                entry.used = (self.clock)();
                return Some(entry.browser.clone());
            }
        }
        None
    }
    /// Resolve the persistent profile identity without exposing profile contents.
    pub async fn space_for_tab(&self, tab: &str) -> Option<String> {
        for (id, entry) in self.running.lock().await.iter() {
            if entry.browser.tabs().await.iter().any(|item| item.id == tab) {
                return Some(id.clone());
            }
        }
        None
    }
    /// List this session's account tabs without launching or refreshing idle profiles.
    pub async fn tabs(&self, session: &str) -> Vec<crate::TabInfo> {
        let mut tabs = Vec::new();
        for entry in self.running.lock().await.values() {
            tabs.extend(
                entry
                    .browser
                    .tabs()
                    .await
                    .into_iter()
                    .filter(|tab| tab.owner_session_id.as_deref() == Some(session)),
            );
        }
        tabs
    }
    /// Close only profiles whose owned tabs and tool activity are both idle.
    pub async fn close_idle(&self) -> Result<(), BrowserError> {
        let mut running = self.running.lock().await;
        let now = (self.clock)();
        let mut idle = Vec::new();
        for (id, entry) in running.iter_mut() {
            if entry
                .browser
                .tabs()
                .await
                .iter()
                .any(|tab| tab.owner_session_id.is_some())
            {
                entry.used = now;
            } else if now.saturating_duration_since(entry.used) >= IDLE_TIMEOUT {
                idle.push(id.clone());
            }
        }
        for id in idle {
            if let Some(entry) = running.get(&id) {
                terminate(&entry.browser).await?;
            }
            running.remove(&id);
        }
        Ok(())
    }
    /// End running processes while retaining their persistent profiles.
    pub async fn close_all(&self) -> Result<(), BrowserError> {
        let mut running = self.running.lock().await;
        for entry in running.values() {
            terminate(&entry.browser).await?;
        }
        running.clear();
        Ok(())
    }
    /// End the process before deleting the entire profile.
    pub async fn revoke(&self, root: &Path, id: &str) -> Result<(), BrowserError> {
        let directory = Self::profile(root, id)?;
        let mut running = self.running.lock().await;
        if let Some(entry) = running.get(id) {
            terminate(&entry.browser).await?;
        }
        running.remove(id);
        for path in root.parent().into_iter().chain(std::iter::once(root)) {
            match tokio::fs::symlink_metadata(path).await {
                Ok(metadata) if metadata.is_dir() => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                _ => {
                    return Err(BrowserError::InvalidEndpoint(
                        "Account profile root is not a directory",
                    ))
                }
            }
        }
        match tokio::fs::symlink_metadata(&directory).await {
            Ok(metadata) if metadata.is_dir() => tokio::fs::remove_dir_all(directory).await?,
            Ok(_) => {
                return Err(BrowserError::InvalidEndpoint(
                    "Account profile is not a directory",
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }
}
async fn private_directory(path: &Path) -> Result<(), BrowserError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .await?;
        if !tokio::fs::symlink_metadata(path).await?.is_dir() {
            return Err(BrowserError::InvalidEndpoint(
                "Account profile is not a directory",
            ));
        }
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await?;
    }
    #[cfg(not(unix))]
    tokio::fs::create_dir_all(path).await?;
    Ok(())
}
async fn terminate(browser: &Browser) -> Result<(), BrowserError> {
    browser.shutdown_shared().await
}
