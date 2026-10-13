//! `openside skills ...`: the skill library through `/v1/skills`, plus two commands that
//! work on local directories without a daemon: `install` puts openside's own browser
//! skill into another agent's skills directory, and `import` copies a skill from one
//! into openside's library.

use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

use clap::Subcommand;
use openside_proto::{CreateSkillRequest, SkillDetail, SkillList, SkillSource, UpdateSkillRequest};
use openside_skills::{agent_skills_dir, bundled, InstallOptions, BROWSER_SKILL};
use serde_json::{json, Value};

use crate::{client::Client, emit, output_error};

#[derive(Subcommand)]
pub(crate) enum SkillsCommand {
    /// List skills; with a query, the ones the `/name` picker would offer.
    List {
        query: Option<String>,
        /// Also list the project skills of this session's workspace.
        #[arg(long, value_name = "SESSION_ID")]
        session: Option<String>,
    },
    /// Show a skill's instructions and files.
    Show {
        name: String,
        #[arg(long, value_name = "SESSION_ID")]
        session: Option<String>,
    },
    /// Create a skill in the library.
    New {
        name: String,
        #[arg(long, value_name = "TEXT")]
        description: String,
        /// Instructions (the Markdown below the frontmatter).
        #[arg(long, value_name = "TEXT", default_value = "")]
        body: String,
    },
    /// Offer a skill to agents again.
    Enable { name: String },
    /// Stop offering a skill to agents without deleting it.
    Disable { name: String },
    /// Delete a skill from the library.
    Rm { name: String },
    /// Install openside's browser skill for another agent (for example `claude`, `codex`),
    /// so that agent can hand browsing to `openside`. Works without a daemon.
    Install {
        /// Agent name; its skills directory comes from `[skills.agents]`,
        /// `OPENSIDE_SKILLS_<AGENT>_DIR`, or that agent's own convention.
        agent: String,
        /// Install into this skills directory instead.
        #[arg(long, value_name = "PATH")]
        to_dir: Option<PathBuf>,
        /// Replace an installed copy.
        #[arg(long)]
        force: bool,
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
    },
    /// Copy a skill into openside's library, from a directory or from another agent's
    /// skills directory. Works without a daemon.
    Import {
        /// A skill directory, or a skill name when `--from` or `--from-dir` is given.
        source: String,
        /// Agent whose skills directory holds `source`.
        #[arg(long, value_name = "AGENT", conflicts_with = "from_dir")]
        from: Option<String>,
        /// Skills directory that holds `source`.
        #[arg(long, value_name = "PATH")]
        from_dir: Option<PathBuf>,
        /// Keep executable files; without this a skill that has any is refused.
        #[arg(long)]
        allow_scripts: bool,
        /// Replace a library skill of the same name.
        #[arg(long)]
        force: bool,
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
    },
}

impl SkillsCommand {
    pub(crate) fn is_local(&self) -> bool {
        matches!(self, Self::Install { .. } | Self::Import { .. })
    }
}

fn path_name(name: &str, session: Option<&String>) -> String {
    let mut path = format!("/v1/skills/{}", urlencoding(name));
    if let Some(session) = session {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("session", session)
            .finish();
        path = format!("{path}?{query}");
    }
    path
}

fn urlencoding(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn source(source: SkillSource) -> &'static str {
    match source {
        SkillSource::User => "user",
        SkillSource::Project => "project",
    }
}

fn print_detail<W: Write>(json: bool, stdout: &mut W, skill: &SkillDetail) -> Result<(), String> {
    if json {
        return emit(stdout, skill);
    }
    writeln!(
        stdout,
        "{} ({}, {})\n{}\n",
        skill.name,
        source(skill.source),
        if skill.enabled { "enabled" } else { "disabled" },
        skill.description
    )
    .map_err(output_error)?;
    if !skill.body.is_empty() {
        writeln!(stdout, "{}", skill.body.trim_end()).map_err(output_error)?;
    }
    for resource in &skill.resources {
        writeln!(stdout, "file\t{}\t{}", resource.path, resource.size).map_err(output_error)?;
    }
    Ok(())
}

pub(crate) async fn execute<W: Write>(
    command: &SkillsCommand,
    json: bool,
    client: &Client,
    stdout: &mut W,
) -> Result<(), String> {
    match command {
        SkillsCommand::List { query, session } => {
            let mut params = url::form_urlencoded::Serializer::new(String::new());
            if let Some(query) = query {
                params.append_pair("q", query);
            }
            if let Some(session) = session {
                params.append_pair("session", session);
            }
            let params = params.finish();
            let path = if params.is_empty() {
                "/v1/skills".to_owned()
            } else {
                format!("/v1/skills?{params}")
            };
            let list: SkillList = client.get(&path).await?;
            if json {
                return emit(stdout, &list);
            }
            if let Some(ui) = crate::ui::pretty() {
                return write!(stdout, "{}", crate::ui::skills(ui, &list)).map_err(output_error);
            }
            for skill in &list.skills {
                writeln!(
                    stdout,
                    "{}\t{}\t{}\t{}",
                    skill.name,
                    source(skill.source),
                    if skill.enabled { "enabled" } else { "disabled" },
                    skill.description
                )
                .map_err(output_error)?;
            }
            for problem in &list.problems {
                writeln!(
                    stdout,
                    "invalid\t{}\t{}\t{}",
                    problem.name,
                    source(problem.source),
                    problem.error
                )
                .map_err(output_error)?;
            }
            Ok(())
        }
        SkillsCommand::Show { name, session } => {
            let skill: SkillDetail = client.get(&path_name(name, session.as_ref())).await?;
            print_detail(json, stdout, &skill)
        }
        SkillsCommand::New {
            name,
            description,
            body,
        } => {
            let skill: SkillDetail = client
                .json(
                    "POST",
                    "/v1/skills",
                    Some(&CreateSkillRequest {
                        name: name.clone(),
                        description: description.clone(),
                        body: body.clone(),
                        license: None,
                        resources: Vec::new(),
                    }),
                )
                .await?;
            print_detail(json, stdout, &skill)
        }
        SkillsCommand::Enable { name } | SkillsCommand::Disable { name } => {
            let enabled = matches!(command, SkillsCommand::Enable { .. });
            let skill: SkillDetail = client
                .json(
                    "PATCH",
                    &path_name(name, None),
                    Some(&UpdateSkillRequest {
                        enabled: Some(enabled),
                        ..UpdateSkillRequest::default()
                    }),
                )
                .await?;
            print_detail(json, stdout, &skill)
        }
        SkillsCommand::Rm { name } => {
            let removed: Value = client
                .json::<_, Value>("DELETE", &path_name(name, None), None)
                .await?;
            if json {
                return emit(stdout, &removed);
            }
            writeln!(stdout, "Deleted skill {name}").map_err(output_error)
        }
        SkillsCommand::Install { .. } | SkillsCommand::Import { .. } => {
            unreachable!("install and import run without an API client")
        }
    }
}

struct Local {
    library: PathBuf,
    agents: BTreeMap<String, PathBuf>,
}

fn local_config(config: Option<&Path>) -> Result<Local, String> {
    let loaded = openside_config::load(config).map_err(|error| error.to_string())?;
    Ok(Local {
        library: loaded.config.skills.library_dir(&loaded.paths.data_dir),
        agents: loaded.config.skills.agents.clone(),
    })
}

fn agent_dir(local: &Local, agent: &str) -> Result<PathBuf, String> {
    agent_skills_dir(agent, &local.agents, &|key| std::env::var_os(key)).ok_or_else(|| {
        format!(
            "no skills directory is known for agent `{agent}`; set `[skills.agents] {agent} = \"<dir>\"` \
             in the configuration or the OPENSIDE_SKILLS_{}_DIR environment variable, or pass --to-dir",
            agent.to_ascii_uppercase().replace('-', "_")
        )
    })
}

pub(crate) fn local<W: Write>(
    command: &SkillsCommand,
    json: bool,
    stdout: &mut W,
) -> Result<(), String> {
    match command {
        SkillsCommand::Install {
            agent,
            to_dir,
            force,
            config,
        } => {
            let target = match to_dir {
                Some(dir) => dir.clone(),
                None => agent_dir(&local_config(config.as_deref())?, agent)?,
            };
            let files = bundled(BROWSER_SKILL).ok_or("the bundled browser skill is missing")?;
            let installed =
                openside_skills::install::write_skill(&target, BROWSER_SKILL, files, *force)
                    .map_err(|error| match error {
                        openside_skills::Error::Exists(name) => format!(
                            "{name} is already installed in {}; use --force to replace it",
                            target.display()
                        ),
                        other => other.to_string(),
                    })?;
            if json {
                return emit(
                    stdout,
                    &json!({"agent": agent, "skill": installed.name, "path": installed.path, "files": installed.files}),
                );
            }
            writeln!(
                stdout,
                "Installed skill {} for {agent} at {}",
                installed.name,
                installed.path.display()
            )
            .map_err(output_error)
        }
        SkillsCommand::Import {
            source,
            from,
            from_dir,
            allow_scripts,
            force,
            config,
        } => {
            let local = local_config(config.as_deref())?;
            // With `--from`/`--from-dir` the source is a name looked up in that directory;
            // it must not be able to climb out of it.
            if from.is_some() || from_dir.is_some() {
                openside_skills::validate_name(source).map_err(|error| error.to_string())?;
            }
            let src = match (from, from_dir) {
                (Some(agent), _) => agent_dir(&local, agent)?.join(source),
                (None, Some(dir)) => dir.join(source),
                (None, None) => PathBuf::from(source),
            };
            let installed = openside_skills::Store::new(&local.library)
                .import(
                    &src,
                    InstallOptions {
                        allow_scripts: *allow_scripts,
                        force: *force,
                    },
                )
                .map_err(|error| match error {
                    openside_skills::Error::Exists(name) => {
                        format!(
                            "skill {name} already exists in the library; use --force to replace it"
                        )
                    }
                    other => other.to_string(),
                })?;
            if json {
                return emit(
                    stdout,
                    &json!({"skill": installed.name, "path": installed.path, "files": installed.files}),
                );
            }
            writeln!(
                stdout,
                "Imported skill {} ({} files) into {}",
                installed.name,
                installed.files,
                installed.path.display()
            )
            .map_err(output_error)
        }
        _ => unreachable!("only install and import run locally"),
    }
}
