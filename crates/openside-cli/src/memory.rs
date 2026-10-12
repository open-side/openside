//! `openside memory ...`: local memory through `/v1/memory`, plus the only command that
//! downloads embedding weights (`memory model fetch`). The daemon itself never fetches.

use std::{io::Write, path::PathBuf};

use clap::{Subcommand, ValueEnum};
use openside_proto::{
    CreateMemoryRequest, Memory, MemoryHistory, MemoryKind, MemorySearchResponse, Retention,
    UpdateMemoryRequest,
};
use serde_json::Value;

use crate::{client::Client, emit, output_error};

const DEFAULT_ENDPOINT: &str = "https://huggingface.co";
// Static embedding models are tens to hundreds of MiB; anything far larger is not one.
const MAX_FILE_BYTES: u64 = 2 << 30;

#[derive(Subcommand)]
pub(crate) enum MemoryCommand {
    /// Search memories; without a query, list the most recently updated ones.
    Search {
        query: Option<String>,
        #[arg(long, value_enum)]
        kind: Option<KindArg>,
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },
    /// Remember a note. Secrets (keys, tokens, passwords, card numbers) are rejected.
    Add {
        content: String,
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
        #[arg(long, value_enum)]
        retention: Option<RetentionArg>,
    },
    /// Show one memory.
    Show { id: String },
    /// Change a memory's content, tags or retention; each change adds a history version.
    Edit {
        id: String,
        #[arg(long, value_name = "TEXT")]
        content: Option<String>,
        #[arg(long = "tag", value_name = "TAG", conflicts_with = "clear_tags")]
        tags: Vec<String>,
        #[arg(long, help = "Remove every tag")]
        clear_tags: bool,
        #[arg(long, value_enum)]
        retention: Option<RetentionArg>,
    },
    /// Delete a memory; stored history snapshots are scrubbed.
    Rm { id: String },
    /// Show a memory's change history (time, actor, operation).
    History { id: String },
    /// Manage the local embedding model.
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
}

#[derive(Subcommand)]
pub(crate) enum ModelCommand {
    /// Download a static (model2vec-style) embedding model into memory.embedding.model_dir.
    Fetch {
        /// Hugging Face repository, `owner/name`.
        repo: String,
        #[arg(long, default_value = "main")]
        revision: String,
        #[arg(long, env = "OPENSIDE_HF_ENDPOINT", value_name = "URL", default_value = DEFAULT_ENDPOINT)]
        endpoint: String,
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum KindArg {
    Note,
    History,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum RetentionArg {
    Permanent,
    #[value(name = "days_90")]
    Days90,
    #[value(name = "days_30")]
    Days30,
}

impl From<RetentionArg> for Retention {
    fn from(value: RetentionArg) -> Self {
        match value {
            RetentionArg::Permanent => Self::Permanent,
            RetentionArg::Days90 => Self::Days90,
            RetentionArg::Days30 => Self::Days30,
        }
    }
}

pub(crate) async fn execute<W: Write>(
    command: &MemoryCommand,
    json: bool,
    client: &Client,
    stdout: &mut W,
) -> Result<(), String> {
    match command {
        MemoryCommand::Search { query, kind, limit } => {
            let mut params = url::form_urlencoded::Serializer::new(String::new());
            if let Some(query) = query {
                params.append_pair("q", query);
            }
            if let Some(kind) = kind {
                params.append_pair(
                    "kind",
                    match kind {
                        KindArg::Note => MemoryKind::Note.as_str(),
                        KindArg::History => MemoryKind::History.as_str(),
                    },
                );
            }
            if let Some(limit) = limit {
                params.append_pair("limit", &limit.to_string());
            }
            let params = params.finish();
            let path = if params.is_empty() {
                "/v1/memory".to_owned()
            } else {
                format!("/v1/memory?{params}")
            };
            let found: MemorySearchResponse = client.get(&path).await?;
            if json {
                return emit(stdout, &found);
            }
            for hit in &found.hits {
                writeln!(
                    stdout,
                    "{}\t{}\t{:.3}\t{}",
                    hit.memory.id,
                    hit.memory.kind.as_str(),
                    hit.score,
                    headline(&hit.memory)
                )
                .map_err(output_error)?;
            }
            Ok(())
        }
        MemoryCommand::Add {
            content,
            tags,
            retention,
        } => {
            let memory: Memory = client
                .json(
                    "POST",
                    "/v1/memory",
                    Some(&CreateMemoryRequest {
                        content: content.clone(),
                        tags: tags.clone(),
                        retention: retention.map(Into::into),
                    }),
                )
                .await?;
            print_memory(json, stdout, &memory)
        }
        MemoryCommand::Show { id } => {
            let memory: Memory = client.get(&memory_path(id, "")).await?;
            print_memory(json, stdout, &memory)
        }
        MemoryCommand::Edit {
            id,
            content,
            tags,
            clear_tags,
            retention,
        } => {
            let request = UpdateMemoryRequest {
                content: content.clone(),
                tags: (*clear_tags || !tags.is_empty()).then(|| tags.clone()),
                retention: retention.map(Into::into),
            };
            let memory: Memory = client
                .json("PATCH", &memory_path(id, ""), Some(&request))
                .await?;
            print_memory(json, stdout, &memory)
        }
        MemoryCommand::Rm { id } => {
            let memory: Memory = client
                .json::<_, Value>("DELETE", &memory_path(id, ""), None)
                .await?;
            if json {
                return emit(stdout, &memory);
            }
            writeln!(stdout, "Deleted memory {}", memory.id).map_err(output_error)
        }
        MemoryCommand::History { id } => {
            let history: MemoryHistory = client.get(&memory_path(id, "history")).await?;
            if json {
                return emit(stdout, &history);
            }
            for version in &history.versions {
                let content = version.content.as_deref().unwrap_or("[scrubbed]");
                writeln!(
                    stdout,
                    "v{}\t{}\t{}\t{}\t{}",
                    version.version,
                    version.at.to_rfc3339(),
                    serde_json::to_value(version.op)
                        .ok()
                        .and_then(|op| op.as_str().map(str::to_owned))
                        .unwrap_or_default(),
                    version.actor,
                    first_line(content)
                )
                .map_err(output_error)?;
            }
            Ok(())
        }
        MemoryCommand::Model { .. } => unreachable!("model commands run without an API client"),
    }
}

#[cfg(not(unix))]
pub(crate) async fn model<W: Write>(
    _command: &ModelCommand,
    _json: bool,
    _stdout: &mut W,
) -> Result<(), String> {
    Err("Atomic model installation is unsupported on this platform".into())
}

#[cfg(unix)]
pub(crate) async fn model<W: Write>(
    command: &ModelCommand,
    json: bool,
    stdout: &mut W,
) -> Result<(), String> {
    let ModelCommand::Fetch {
        repo,
        revision,
        endpoint,
        config,
    } = command;
    let loaded = openside_config::load(config.as_deref()).map_err(|error| error.to_string())?;
    let target = loaded.config.memory.model_dir(&loaded.paths.data_dir);
    let model_id = fetch(
        endpoint,
        &loaded.config.network.allow,
        repo,
        revision,
        &target,
    )
    .await?;
    if json {
        return emit(
            stdout,
            &serde_json::json!({"repo": repo, "revision": revision, "model_dir": target, "model_id": model_id}),
        );
    }
    writeln!(
        stdout,
        "Installed {repo}@{revision} ({model_id}) into {}. Restart opensided to enable hybrid memory search.",
        target.display()
    )
    .map_err(output_error)
}

/// Plain http is only for loopback test servers; everything else, including every redirect
/// hop, must be https.
fn allowed_transport(url: &url::Url) -> bool {
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    url.scheme() == "https" || (url.scheme() == "http" && loopback)
}

/// Hosts the default Hugging Face endpoint redirects file downloads to.
const DEFAULT_ENDPOINT_CDNS: &[&str] = &["https://*.huggingface.co", "https://*.hf.co"];

/// Outbound allow-list for one fetch: the chosen endpoint's origin, the default endpoint's
/// download CDNs, and `network.allow`; every request and redirect hop must pass it.
fn fetch_egress(
    base: &url::Url,
    network_allow: &[String],
) -> Result<openside_policy::Egress, String> {
    let origin =
        openside_policy::Origin::from_url(base).ok_or("--endpoint must be an http(s) URL")?;
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
}

/// Download into a staging directory, verify it loads as a supported static model, then
/// publish it as an immutable generation and point `target` at it with one atomic rename.
async fn fetch(
    endpoint: &str,
    network_allow: &[String],
    repo: &str,
    revision: &str,
    target: &std::path::Path,
) -> Result<String, String> {
    let base = url::Url::parse(endpoint).map_err(|_| "--endpoint must be a URL".to_owned())?;
    if !allowed_transport(&base) {
        return Err("--endpoint must use https (plain http only for loopback test servers)".into());
    }
    let egress = fetch_egress(&base, network_allow)?;
    let valid = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 || !parts.iter().all(|part| valid(part)) {
        return Err("repository must look like owner/name".into());
    }
    if !valid(revision) {
        return Err("--revision must be a branch, tag or commit name".into());
    }
    let slot = ModelSlot::new(target)?;
    let install_error = |error: std::io::Error| {
        format!(
            "could not install the model into {}: {error}",
            target.display()
        )
    };
    slot.locked(ModelSlot::recover).map_err(install_error)?;
    let staging = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(&slot.generations)
        .map_err(|error| format!("could not create a staging directory: {error}"))?;
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
        .map_err(|error| format!("could not start the HTTP client: {error}"))?;
    for file in openside_config::EMBEDDING_MODEL_FILES {
        let url = format!(
            "{}/{repo}/resolve/{revision}/{file}",
            endpoint.trim_end_matches('/')
        );
        egress.check_url(&url).map_err(|error| error.to_string())?;
        download(&client, &url, &staging.path().join(file)).await?;
    }
    let embedder = openside_memory::StaticEmbedder::load(staging.path())
        .map_err(|error| format!("{repo} is not a supported static embedding model: {error}"))?;
    let model_id = openside_memory::Embedder::id(&embedder).to_owned();
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
        .map_err(install_error)?;
    Ok(model_id)
}

const STAGING_PREFIX: &str = ".staging-";
const GENERATION_PREFIX: &str = "gen-";
/// A replaced generation stays this long so a loader that resolved it just before the swap
/// can still open both files.
const GENERATION_GRACE: std::time::Duration = std::time::Duration::from_secs(60);
/// Staging directories of crashed fetches; long enough for any live download to finish.
const STAGING_GRACE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

#[cfg(test)]
pub(crate) static INTERRUPT_MIGRATION: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

/// On-disk layout around `model_dir` (file name `<name>`):
/// - `.<name>.generations/gen-*`: immutable, verified model directories;
/// - `<name>`: symlink to the active generation, replaced by a single `rename`, so a loader
///   that resolves it once reads both files from one generation;
/// - `.<name>.lock`: serializes installers;
/// - `.<name>.journal`: exists only while a plain (pre-generation) `model_dir` is moved aside,
///   the one step that is not a single rename; the next fetch completes it.
struct ModelSlot {
    target: std::path::PathBuf,
    parent: std::path::PathBuf,
    name: String,
    generations: std::path::PathBuf,
}

impl ModelSlot {
    fn new(target: &std::path::Path) -> Result<Self, String> {
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

    fn sibling(&self, suffix: &str) -> std::path::PathBuf {
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
                self.link(std::path::Path::new(generation.trim()))?;
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
    fn link(&self, _relative: &std::path::Path) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Atomic model installation is unsupported on this platform",
        ))
    }

    #[cfg(unix)]
    fn link(&self, relative: &std::path::Path) -> std::io::Result<()> {
        let temporary = self.sibling(&format!("link-{}", std::process::id()));
        let _ = std::fs::remove_file(&temporary);
        std::os::unix::fs::symlink(relative, &temporary)?;
        std::fs::rename(&temporary, &self.target)?;
        sync_dir(&self.parent)
    }

    /// Point `target` at `generation`, then prune generations nobody can still be opening.
    /// Caller holds the lock.
    fn activate(&self, generation: &std::path::Path) -> std::io::Result<()> {
        let relative = std::path::Path::new(&format!(".{}.generations", self.name))
            .join(generation.file_name().unwrap_or_default());
        let replaced = std::fs::canonicalize(&self.target).ok();
        match std::fs::symlink_metadata(&self.target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => self.link(&relative)?,
            Err(error) => return Err(error),
            Ok(meta) if meta.file_type().is_symlink() => self.link(&relative)?,
            Ok(meta) if meta.is_dir() => {
                let journal = self.sibling("journal");
                let mut file = std::fs::File::create(&journal)?;
                std::io::Write::write_all(&mut file, relative.to_string_lossy().as_bytes())?;
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
                #[cfg(test)]
                if INTERRUPT_MIGRATION
                    .lock()
                    .map(|guard| guard.as_deref() == Some(self.target.as_path()))
                    .unwrap_or(false)
                {
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

    fn prune(&self, active: &std::path::Path) -> std::io::Result<()> {
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

fn sync_dir(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

async fn download(
    client: &reqwest::Client,
    url: &str,
    path: &std::path::Path,
) -> Result<(), String> {
    let write_error =
        |error: std::io::Error| format!("could not write {}: {error}", path.display());
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let mut response = client.get(url).send().await.map_err(|error| {
        // The top-level reqwest message omits why a redirect was refused.
        let mut message = format!("could not download {name}: {error}");
        let mut cause = std::error::Error::source(&error);
        while let Some(inner) = cause {
            message.push_str(&format!(": {inner}"));
            cause = inner.source();
        }
        message
    })?;
    if !response.status().is_success() {
        return Err(format!(
            "could not download {name}: HTTP {}",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_FILE_BYTES)
    {
        return Err(format!("{name} is larger than a static embedding model"));
    }
    let mut file = std::fs::File::create(path).map_err(write_error)?;
    let mut written = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("could not download {name}: {error}"))?
    {
        written += chunk.len() as u64;
        if written > MAX_FILE_BYTES {
            return Err(format!("{name} is larger than a static embedding model"));
        }
        file.write_all(&chunk).map_err(write_error)?;
    }
    file.sync_all().map_err(write_error)
}

fn print_memory<W: Write>(json: bool, stdout: &mut W, memory: &Memory) -> Result<(), String> {
    if json {
        return emit(stdout, memory);
    }
    let mut line = format!(
        "{}\t{}\tv{}\t{}",
        memory.id,
        memory.kind.as_str(),
        memory.version,
        memory.retention.as_str()
    );
    if let Some(expires) = memory.expires_at {
        line.push_str(&format!("\texpires {}", expires.to_rfc3339()));
    }
    if !memory.tags.is_empty() {
        line.push_str(&format!("\ttags {}", memory.tags.join(",")));
    }
    writeln!(stdout, "{line}").map_err(output_error)?;
    if let Some(url) = &memory.url {
        writeln!(stdout, "{url}").map_err(output_error)?;
    }
    writeln!(stdout, "{}", memory.content).map_err(output_error)
}

fn headline(memory: &Memory) -> String {
    match (&memory.title, &memory.url) {
        (Some(title), Some(url)) => format!("{title} <{url}>"),
        (None, Some(url)) => url.clone(),
        _ => first_line(&memory.content),
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    let mut short: String = line.chars().take(80).collect();
    if short.len() < line.len() {
        short.push('…');
    }
    short
}

fn memory_path(id: &str, suffix: &str) -> String {
    let encoded = crate::client::encode_segment(id);
    if suffix.is_empty() {
        format!("/v1/memory/{encoded}")
    } else {
        format!("/v1/memory/{encoded}/{suffix}")
    }
}
