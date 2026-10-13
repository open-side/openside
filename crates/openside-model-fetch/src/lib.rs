//! Download a static embedding model from a Hugging Face compatible source, verify it against
//! the checksums the source publishes, and install it atomically (`docs/specs/memory.md`).
//!
//! Shared by `openside memory model fetch` and the daemon's `/v1/memory/model` job. This is
//! deliberately not part of `openside-memory`: the store and the embedder stay free of network
//! code, and nothing here runs unless a person asks for a download.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use sha1::Sha1;
use sha2::{Digest as _, Sha256};
pub use tokio_util::sync::CancellationToken;

/// Default model source.
pub const DEFAULT_ENDPOINT: &str = "https://huggingface.co";
/// The recommended model: a small WordPiece static embedding model (about 30 MB).
pub const DEFAULT_REPO: &str = "minishlab/potion-base-8M";
/// Branch resolved to a commit before anything is downloaded.
pub const DEFAULT_REVISION: &str = "main";

const MAX_FILE_BYTES: u64 = 2 << 30;
const MAX_LISTING_BYTES: usize = 4 << 20;

/// Where a model comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// Base URL of the model hub; https, or http on loopback for test servers.
    pub endpoint: String,
    /// `owner/name`.
    pub repo: String,
    /// Branch, tag or commit.
    pub revision: String,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.to_owned(),
            repo: DEFAULT_REPO.to_owned(),
            revision: DEFAULT_REVISION.to_owned(),
        }
    }
}

impl Source {
    /// Check what can be checked without the network: a well-formed https (or loopback http)
    /// endpoint, an `owner/name` repository and a plain revision name. Returns the endpoint URL.
    pub fn validate(&self) -> Result<url::Url, Failure> {
        let base = url::Url::parse(&self.endpoint)
            .map_err(|_| Failure::Invalid("the endpoint must be a URL".into()))?;
        if !allowed_transport(&base) {
            return Err(Failure::Invalid(
                "the endpoint must use https (plain http only for loopback test servers)".into(),
            ));
        }
        let parts: Vec<&str> = self.repo.split('/').collect();
        if parts.len() != 2 || !parts.iter().all(|part| valid_part(part)) {
            return Err(Failure::Invalid(
                "repository must look like owner/name".into(),
            ));
        }
        if !valid_part(&self.revision) {
            return Err(Failure::Invalid(
                "the revision must be a branch, tag or commit name".into(),
            ));
        }
        Ok(base)
    }
}

/// Stage of a running fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Reading the source's file listing and checksums.
    Resolving,
    /// Transferring model files.
    Downloading,
    /// Comparing checksums and loading the staged model.
    Verifying,
    /// Swapping the verified model in.
    Installing,
}

/// Snapshot passed to the progress callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    /// Current stage.
    pub phase: Phase,
    /// File being transferred.
    pub file: Option<String>,
    /// Files fully transferred.
    pub files_done: u32,
    /// Files this model needs.
    pub files_total: u32,
    /// Bytes transferred so far across all files.
    pub bytes_done: u64,
    /// Bytes expected in total; zero when the source did not say.
    pub bytes_total: u64,
}

/// Why a fetch did not install a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The repository, revision or endpoint is not acceptable.
    Invalid(String),
    /// A destination is outside the endpoint and `network.allow`, or not https.
    Refused(String),
    /// The transfer failed.
    Network(String),
    /// The source did not publish a checksum for a required file.
    ChecksumUnavailable(String),
    /// A downloaded file does not match the published checksum.
    ChecksumMismatch(String),
    /// The files do not form a supported static embedding model.
    Unsupported(String),
    /// The verified model could not be put in place.
    Install(String),
    /// Cancelled by the caller; the installed model is untouched.
    Cancelled,
}

impl Failure {
    /// Stable machine-readable code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid_source",
            Self::Refused(_) => "egress_refused",
            Self::Network(_) => "download_failed",
            Self::ChecksumUnavailable(_) => "checksum_unavailable",
            Self::ChecksumMismatch(_) => "checksum_mismatch",
            Self::Unsupported(_) => "unsupported_model",
            Self::Install(_) => "install_failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message)
            | Self::Refused(message)
            | Self::Network(message)
            | Self::ChecksumUnavailable(message)
            | Self::ChecksumMismatch(message)
            | Self::Unsupported(message)
            | Self::Install(message) => f.write_str(message),
            Self::Cancelled => f.write_str("the download was cancelled"),
        }
    }
}

impl std::error::Error for Failure {}

/// A model that is now the active one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// Content-derived id of the embedder, as `GET /v1/memory/health` reports it.
    pub model_id: String,
    /// Commit the files came from.
    pub revision: String,
}

/// Plain http is only for loopback test servers; everything else, including every redirect
/// hop, must be https.
fn allowed_transport(url: &url::Url) -> bool {
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    url.scheme() == "https" || (url.scheme() == "http" && loopback)
}

/// Hosts the default endpoint redirects file downloads to.
const DEFAULT_ENDPOINT_CDNS: &[&str] = &["https://*.huggingface.co", "https://*.hf.co"];

/// Outbound allow-list for one fetch: the chosen endpoint's origin, the default endpoint's
/// download CDNs, and `network.allow`; every request and redirect hop must pass it.
fn fetch_egress(
    base: &url::Url,
    network_allow: &[String],
) -> Result<openside_policy::Egress, Failure> {
    let origin = openside_policy::Origin::from_url(base)
        .ok_or_else(|| Failure::Invalid("the endpoint must be an http(s) URL".into()))?;
    let endpoint = format!("{}://{}:{}", origin.scheme(), origin.host(), origin.port());
    let cdns = if base.as_str().trim_end_matches('/') == DEFAULT_ENDPOINT {
        DEFAULT_ENDPOINT_CDNS
    } else {
        &[]
    };
    openside_policy::Egress::new(
        std::iter::once(endpoint.as_str())
            .chain(cdns.iter().copied())
            .chain(network_allow.iter().map(String::as_str)),
    )
    .map_err(Failure::Invalid)
}

fn valid_part(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// Checksum the source published for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Digest {
    /// SHA-256 of the content (files stored as LFS objects).
    Sha256(String),
    /// Git blob id (SHA-1 over `blob <len>\0` and the content) of a plain file.
    GitBlob(String),
}

struct Expected {
    name: &'static str,
    size: Option<u64>,
    digest: Digest,
}

struct Listing {
    revision: String,
    files: Vec<Expected>,
}

fn is_hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Read the commit and per-file checksums from `GET /api/models/<repo>/revision/<rev>`.
fn parse_listing(body: &[u8], revision: &str) -> Result<Listing, Failure> {
    let unavailable = |name: &str| {
        Failure::ChecksumUnavailable(format!(
            "the source does not publish a checksum for {name}, so the download is refused"
        ))
    };
    let document: serde_json::Value = serde_json::from_slice(body).map_err(|_| {
        Failure::ChecksumUnavailable(
            "the source did not return a file listing with checksums, so the download is refused"
                .into(),
        )
    })?;
    let siblings = document
        .get("siblings")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| unavailable("the model files"))?;
    let mut files = Vec::new();
    for name in openside_config::EMBEDDING_MODEL_FILES {
        let sibling = siblings
            .iter()
            .find(|sibling| {
                sibling.get("rfilename").and_then(serde_json::Value::as_str) == Some(name)
            })
            .ok_or_else(|| {
                Failure::Unsupported(format!(
                    "the repository has no {name}, so it is not a supported static embedding model"
                ))
            })?;
        let size = sibling.get("size").and_then(serde_json::Value::as_u64);
        let lfs = sibling
            .get("lfs")
            .and_then(|lfs| lfs.get("sha256"))
            .and_then(serde_json::Value::as_str)
            .filter(|digest| is_hex(digest, 64));
        let blob = sibling
            .get("blobId")
            .and_then(serde_json::Value::as_str)
            .filter(|digest| is_hex(digest, 40));
        let digest = match (lfs, blob) {
            (Some(digest), _) => Digest::Sha256(digest.to_ascii_lowercase()),
            (None, Some(blob)) => Digest::GitBlob(blob.to_ascii_lowercase()),
            (None, None) => return Err(unavailable(name)),
        };
        files.push(Expected { name, size, digest });
    }
    // Download the commit the checksums describe, not a branch that may have moved since.
    let pinned = document
        .get("sha")
        .and_then(serde_json::Value::as_str)
        .filter(|sha| is_hex(sha, 40))
        .unwrap_or(revision);
    Ok(Listing {
        revision: pinned.to_owned(),
        files,
    })
}

fn describe(error: &reqwest::Error, what: &str) -> Failure {
    // The top-level reqwest message omits why a redirect was refused.
    let mut message = format!("could not download {what}: {error}");
    let mut cause = std::error::Error::source(error);
    while let Some(inner) = cause {
        message.push_str(&format!(": {inner}"));
        cause = inner.source();
    }
    if error.is_redirect() {
        Failure::Refused(message)
    } else {
        Failure::Network(message)
    }
}

fn digest_matches(path: &Path, expected: &Digest) -> std::io::Result<bool> {
    let mut file = std::fs::File::open(path)?;
    let mut buffer = vec![0u8; 64 * 1024];
    match expected {
        Digest::Sha256(hex) => {
            let mut hasher = Sha256::new();
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            Ok(format!("{:x}", hasher.finalize()) == *hex)
        }
        Digest::GitBlob(hex) => {
            let length = file.metadata()?.len();
            let mut hasher = Sha1::new();
            hasher.update(format!("blob {length}\0").as_bytes());
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            Ok(format!("{:x}", hasher.finalize()) == *hex)
        }
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Failure> + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|_| Err(Failure::Install("the install task failed".into())))
}

/// Download the model of `source` into a staging directory, verify its checksums and that it
/// loads as a supported static model, then publish it as an immutable generation and point
/// `target` at it with one atomic rename. A failed or cancelled fetch leaves the installed
/// model as it was.
pub async fn fetch(
    source: &Source,
    network_allow: &[String],
    target: &Path,
    on_progress: &(dyn Fn(Progress) + Send + Sync),
    cancel: &CancellationToken,
) -> Result<Installed, Failure> {
    if cfg!(not(unix)) {
        return Err(Failure::Unsupported(
            "Atomic model installation is unsupported on this platform".into(),
        ));
    }
    let endpoint = source.endpoint.trim_end_matches('/');
    let base = source.validate()?;
    let egress = fetch_egress(&base, network_allow)?;
    let owned_target = target.to_owned();
    let (slot, staging) = blocking(move || {
        let install_error = |error: std::io::Error| {
            Failure::Install(format!(
                "could not install the model into {}: {error}",
                owned_target.display()
            ))
        };
        let slot = ModelSlot::new(&owned_target).map_err(Failure::Install)?;
        slot.locked(ModelSlot::recover).map_err(install_error)?;
        let staging = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(&slot.generations)
            .map_err(|error| {
                Failure::Install(format!("could not create a staging directory: {error}"))
            })?;
        Ok((slot, staging))
    })
    .await?;

    let hops = egress.clone();
    let client = reqwest::Client::builder()
        .user_agent(concat!("openside/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 10 {
                attempt.error("too many redirects")
            } else if !allowed_transport(attempt.url()) {
                attempt.error("redirect to a non-https destination refused")
            } else if let Err(error) = hops.check_url(attempt.url().as_str()) {
                attempt.error(format!("redirect refused: {error}"))
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|error| Failure::Network(format!("could not start the HTTP client: {error}")))?;

    let mut progress = Progress {
        phase: Phase::Resolving,
        file: None,
        files_done: 0,
        files_total: openside_config::EMBEDDING_MODEL_FILES.len() as u32,
        bytes_done: 0,
        bytes_total: 0,
    };
    on_progress(progress.clone());
    let listing_url = format!(
        "{endpoint}/api/models/{}/revision/{}?blobs=true",
        source.repo, source.revision
    );
    egress
        .check_url(&listing_url)
        .map_err(|error| Failure::Refused(error.to_string()))?;
    let listing = parse_listing(
        &get_limited(
            &client,
            &listing_url,
            "the file listing",
            MAX_LISTING_BYTES,
            cancel,
        )
        .await?,
        &source.revision,
    )?;
    progress.bytes_total = listing
        .files
        .iter()
        .map(|file| file.size.unwrap_or_default())
        .sum();
    progress.phase = Phase::Downloading;

    for file in &listing.files {
        let url = format!(
            "{endpoint}/{}/resolve/{}/{}",
            source.repo, listing.revision, file.name
        );
        egress
            .check_url(&url)
            .map_err(|error| Failure::Refused(error.to_string()))?;
        progress.file = Some(file.name.to_owned());
        on_progress(progress.clone());
        let base_done = progress.bytes_done;
        let written = download(
            &client,
            &url,
            &staging.path().join(file.name),
            file.name,
            cancel,
            &mut |written| {
                progress.bytes_done = base_done + written;
                on_progress(progress.clone());
            },
        )
        .await?;
        progress.bytes_done = base_done + written;
        progress.files_done += 1;
        on_progress(progress.clone());
    }

    progress.phase = Phase::Verifying;
    progress.file = None;
    on_progress(progress.clone());
    let staged = staging.path().to_owned();
    let checks: Vec<(&'static str, Digest)> = listing
        .files
        .iter()
        .map(|file| (file.name, file.digest.clone()))
        .collect();
    let repo = source.repo.clone();
    let model_id = blocking(move || {
        for (name, digest) in &checks {
            let matches = digest_matches(&staged.join(name), digest).map_err(|error| {
                Failure::Install(format!("could not read the downloaded {name}: {error}"))
            })?;
            if !matches {
                return Err(Failure::ChecksumMismatch(format!(
                    "{name} does not match the checksum published by the source; nothing was installed"
                )));
            }
        }
        let embedder = openside_memory::StaticEmbedder::load(&staged).map_err(|error| {
            Failure::Unsupported(format!(
                "{repo} is not a supported static embedding model: {error}"
            ))
        })?;
        Ok(openside_memory::Embedder::id(&embedder).to_owned())
    })
    .await?;
    if cancel.is_cancelled() {
        return Err(Failure::Cancelled);
    }

    progress.phase = Phase::Installing;
    on_progress(progress.clone());
    let owned_target = target.to_owned();
    blocking(move || {
        let install_error = |error: std::io::Error| {
            Failure::Install(format!(
                "could not install the model into {}: {error}",
                owned_target.display()
            ))
        };
        let staged = staging.keep();
        let generation = slot.generations.join(
            staged
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix(STAGING_PREFIX))
                .map(|suffix| format!("{GENERATION_PREFIX}{suffix}"))
                .unwrap_or_else(|| format!("{GENERATION_PREFIX}{}", std::process::id())),
        );
        std::fs::rename(&staged, &generation).map_err(install_error)?;
        slot.locked(|slot| slot.activate(&generation))
            .map_err(install_error)
    })
    .await?;
    Ok(Installed {
        model_id,
        revision: listing.revision,
    })
}

/// A small JSON document, read whole under a size cap.
async fn get_limited(
    client: &reqwest::Client,
    url: &str,
    what: &str,
    limit: usize,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, Failure> {
    let send = async {
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|error| describe(&error, what))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Failure::Invalid(
                "the source has no such repository or revision".into(),
            ));
        }
        if !response.status().is_success() {
            return Err(Failure::Network(format!(
                "could not download {what}: HTTP {}",
                response.status()
            )));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| describe(&error, what))?
        {
            if body.len() + chunk.len() > limit {
                return Err(Failure::Network(format!("{what} is unexpectedly large")));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    };
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(Failure::Cancelled),
        result = send => result,
    }
}

/// Stream one file to `path`; returns the bytes written. `on_bytes` gets the running total.
async fn download(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
    name: &str,
    cancel: &CancellationToken,
    on_bytes: &mut (dyn FnMut(u64) + Send),
) -> Result<u64, Failure> {
    let write_error = |error: std::io::Error| {
        Failure::Install(format!("could not write {}: {error}", path.display()))
    };
    let mut response = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(Failure::Cancelled),
        result = client.get(url).send() => result.map_err(|error| describe(&error, name))?,
    };
    if !response.status().is_success() {
        return Err(Failure::Network(format!(
            "could not download {name}: HTTP {}",
            response.status()
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_FILE_BYTES)
    {
        return Err(Failure::Unsupported(format!(
            "{name} is larger than a static embedding model"
        )));
    }
    let mut file = std::fs::File::create(path).map_err(write_error)?;
    let mut written = 0u64;
    loop {
        let chunk = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(Failure::Cancelled),
            chunk = response.chunk() => chunk.map_err(|error| describe(&error, name))?,
        };
        let Some(chunk) = chunk else { break };
        written += chunk.len() as u64;
        if written > MAX_FILE_BYTES {
            return Err(Failure::Unsupported(format!(
                "{name} is larger than a static embedding model"
            )));
        }
        file.write_all(&chunk).map_err(write_error)?;
        on_bytes(written);
    }
    file.sync_all().map_err(write_error)?;
    Ok(written)
}

const STAGING_PREFIX: &str = ".staging-";
const GENERATION_PREFIX: &str = "gen-";
/// A replaced generation stays this long so a loader that resolved it just before the swap
/// can still open both files.
const GENERATION_GRACE: Duration = Duration::from_secs(60);
/// Staging directories of crashed fetches; long enough for any live download to finish.
const STAGING_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

/// Tests set this to a model directory to make its next plain-directory migration stop
/// halfway, as a crash would.
#[cfg(any(test, feature = "testing"))]
pub static INTERRUPT_MIGRATION: parking_lot::Mutex<Option<PathBuf>> = parking_lot::Mutex::new(None);

/// On-disk layout around `model_dir` (file name `<name>`):
/// - `.<name>.generations/gen-*`: immutable, verified model directories;
/// - `<name>`: symlink to the active generation, replaced by a single `rename`, so a loader
///   that resolves it once reads both files from one generation;
/// - `.<name>.lock`: serializes installers;
/// - `.<name>.journal`: exists only while a plain (pre-generation) `model_dir` is moved aside,
///   the one step that is not a single rename; the next fetch completes it.
struct ModelSlot {
    target: PathBuf,
    parent: PathBuf,
    name: String,
    generations: PathBuf,
}

impl ModelSlot {
    fn new(target: &Path) -> Result<Self, String> {
        let parent = target
            .parent()
            .ok_or_else(|| "model directory has no parent".to_owned())?
            .to_owned();
        let name = target
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "model directory needs a UTF-8 file name".to_owned())?
            .to_owned();
        let generations = parent.join(format!(".{name}.generations"));
        std::fs::create_dir_all(&generations)
            .map_err(|error| format!("could not create {}: {error}", generations.display()))?;
        Ok(Self {
            target: target.to_owned(),
            parent,
            name,
            generations,
        })
    }

    fn sibling(&self, suffix: &str) -> PathBuf {
        self.parent.join(format!(".{}.{suffix}", self.name))
    }

    fn locked<T>(&self, work: impl FnOnce(&Self) -> std::io::Result<T>) -> std::io::Result<T> {
        let lock = std::fs::File::create(self.sibling("lock"))?;
        fs2::FileExt::lock_exclusive(&lock)?;
        work(self)
    }

    /// Finish an interrupted migration and drop temporary links. Caller holds the lock.
    fn recover(&self) -> std::io::Result<()> {
        let journal = self.sibling("journal");
        if let Ok(generation) = std::fs::read_to_string(&journal) {
            if std::fs::symlink_metadata(&self.target).is_err() {
                self.link(Path::new(generation.trim()))?;
            }
            std::fs::remove_file(&journal)?;
            sync_dir(&self.parent)?;
        }
        let link_prefix = format!(".{}.link-", self.name);
        for entry in std::fs::read_dir(&self.parent)? {
            let entry = entry?;
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(&link_prefix)
            {
                std::fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn link(&self, _relative: &Path) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Atomic model installation is unsupported on this platform",
        ))
    }

    #[cfg(unix)]
    fn link(&self, relative: &Path) -> std::io::Result<()> {
        let temporary = self.sibling(&format!("link-{}", std::process::id()));
        let _ = std::fs::remove_file(&temporary);
        std::os::unix::fs::symlink(relative, &temporary)?;
        std::fs::rename(&temporary, &self.target)?;
        sync_dir(&self.parent)
    }

    /// Point `target` at `generation`, then prune generations nobody can still be opening.
    /// Caller holds the lock.
    fn activate(&self, generation: &Path) -> std::io::Result<()> {
        let relative = Path::new(&format!(".{}.generations", self.name))
            .join(generation.file_name().unwrap_or_default());
        let replaced = std::fs::canonicalize(&self.target).ok();
        match std::fs::symlink_metadata(&self.target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => self.link(&relative)?,
            Err(error) => return Err(error),
            Ok(meta) if meta.file_type().is_symlink() => self.link(&relative)?,
            Ok(meta) if meta.is_dir() => {
                let journal = self.sibling("journal");
                let mut file = std::fs::File::create(&journal)?;
                file.write_all(relative.to_string_lossy().as_bytes())?;
                file.sync_all()?;
                sync_dir(&self.parent)?;
                let legacy = self.generations.join(format!(
                    "{GENERATION_PREFIX}legacy-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis()
                ));
                std::fs::rename(&self.target, &legacy)?;
                #[cfg(any(test, feature = "testing"))]
                if INTERRUPT_MIGRATION.lock().as_deref() == Some(self.target.as_path()) {
                    return Err(std::io::Error::other("simulated crash during migration"));
                }
                self.link(&relative)?;
                std::fs::remove_file(&journal)?;
                sync_dir(&self.parent)?;
            }
            Ok(_) => {
                return Err(std::io::Error::other(
                    "model directory path exists and is not a directory",
                ))
            }
        }
        if let Some(replaced) = replaced {
            // Restart the grace period of the generation a loader may have just resolved.
            if let Ok(dir) = std::fs::File::open(&replaced) {
                let _ = dir.set_modified(std::time::SystemTime::now());
            }
        }
        self.prune(generation)
    }

    fn prune(&self, active: &Path) -> std::io::Result<()> {
        for entry in std::fs::read_dir(&self.generations)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let grace = if name.starts_with(GENERATION_PREFIX) {
                GENERATION_GRACE
            } else if name.starts_with(STAGING_PREFIX) {
                STAGING_GRACE
            } else {
                continue;
            };
            let age = entry.metadata()?.modified()?.elapsed().unwrap_or_default();
            if entry.path() != active && age > grace {
                std::fs::remove_dir_all(entry.path())?;
            }
        }
        Ok(())
    }
}

fn sync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}
