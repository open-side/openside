//! `openside memory ...`: local memory through `/v1/memory`, plus `memory model fetch`, the
//! command-line way to download embedding weights (the daemon does it for the web app).

use std::{io::Write, path::PathBuf};

use clap::{Subcommand, ValueEnum};
use openside_proto::{
    CreateMemoryRequest, Memory, MemoryHistory, MemoryKind, MemorySearchResponse, Retention,
    UpdateMemoryRequest,
};
use serde_json::Value;

use crate::{client::Client, emit, output_error};

const DEFAULT_ENDPOINT: &str = openside_model_fetch::DEFAULT_ENDPOINT;

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
    #[value(name = "hours_24")]
    Hours24,
}

impl From<RetentionArg> for Retention {
    fn from(value: RetentionArg) -> Self {
        match value {
            RetentionArg::Permanent => Self::Permanent,
            RetentionArg::Days90 => Self::Days90,
            RetentionArg::Days30 => Self::Days30,
            RetentionArg::Hours24 => Self::Hours24,
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
    let source = openside_model_fetch::Source {
        endpoint: endpoint.clone(),
        repo: repo.clone(),
        revision: revision.clone(),
    };
    let installed = openside_model_fetch::fetch(
        &source,
        &loaded.config.network.allow,
        &target,
        &|_| {},
        &openside_model_fetch::CancellationToken::new(),
    )
    .await
    .map_err(|error| error.to_string())?;
    let model_id = installed.model_id;
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
