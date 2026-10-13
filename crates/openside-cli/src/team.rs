use std::io::Write;

use clap::{Args, Subcommand};
use openside_proto::{
    Agent, AgentList, AgentPersona, AgentStatus, EventKind, EventPoll, HireDraft,
    RetireAgentRequest, Session, SuppressedProposalList, UpdateAgentRequest,
};
use serde_json::Value;

use crate::{
    client::{self, Client},
    emit, output_error,
    ui::clean,
};

#[derive(Subcommand)]
pub(crate) enum TeamCommand {
    /// List the roster, including hidden and retired teammates.
    List,
    /// Inspect a profile by its id or case-insensitive name; does not change visibility.
    Show { id: String },
    /// Hire a specialist; the role is also their initial responsibility.
    Hire(HireArgs),
    /// Retire a teammate and hand their work to the chief or a selected successor.
    Retire {
        id: String,
        #[arg(long, value_name = "ID", conflicts_with = "pause")]
        hand_to: Option<String>,
        #[arg(
            long,
            help = "Pause owned routines while handing ownership to the chief"
        )]
        pause: bool,
    },
    /// Undo a hire, restoring its recorded ownership and retiring the teammate.
    Undo { id: String },
    /// Restore a retired teammate without reclaiming work already handed over.
    Restore { id: String },
    /// Rename a profile without changing its identity or home conversation.
    Rename { id: String, name: String },
    /// Hide a profile from the sidebar without pausing work.
    Hide { id: String },
    /// Show a hidden profile in the sidebar again.
    Unhide { id: String },
    /// List or clear topics whose hire suggestions you turned off.
    Proposals {
        #[command(subcommand)]
        command: Option<ProposalCommand>,
    },
}

#[derive(Args)]
pub(crate) struct HireArgs {
    #[arg(long)]
    name: String,
    #[arg(long)]
    role: String,
    #[arg(
        long,
        value_name = "SESSION_ID",
        help = "Include a bounded recent conversation excerpt as onboarding context"
    )]
    from_session: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum ProposalCommand {
    /// Allow the selected topic to be suggested again.
    Allow { key: String },
}

pub(crate) async fn execute<W: Write>(
    command: Option<&TeamCommand>,
    json: bool,
    client: &Client,
    stdout: &mut W,
) -> Result<(), String> {
    match command {
        None | Some(TeamCommand::List) => {
            let roster: AgentList = client.get("/v1/agents").await?;
            if json {
                return emit(stdout, &roster);
            }
            writeln!(stdout, "ID\tNAME\tROLE\tSTATUS\tOWNS").map_err(output_error)?;
            for row in roster.agents {
                let agent = row.agent;
                write!(
                    stdout,
                    "{}\t{}\t{}\t{}{}\t",
                    clean(&agent.id),
                    clean(&agent.name),
                    clean(&agent.persona.role),
                    status(agent.status),
                    if agent.hidden { " (hidden)" } else { "" },
                )
                .map_err(output_error)?;
                for (index, line) in agent.persona.responsibilities.iter().enumerate() {
                    if index > 0 {
                        write!(stdout, "; ").map_err(output_error)?;
                    }
                    write!(stdout, "{}", clean(line)).map_err(output_error)?;
                }
                writeln!(stdout).map_err(output_error)?;
            }
            Ok(())
        }
        Some(TeamCommand::Show { id }) => {
            let roster: AgentList = client.get("/v1/agents").await?;
            let name = id.to_lowercase();
            let row = roster
                .agents
                .iter()
                .find(|row| row.agent.id == *id)
                .or_else(|| {
                    roster
                        .agents
                        .iter()
                        .find(|row| row.agent.name.to_lowercase() == name)
                })
                .ok_or_else(|| format!("agent not found: {}", clean(id)))?;
            let agent = &row.agent;
            let profile: Agent = client.get(&agent_path(&agent.id, "")).await?;
            print_agent(json, stdout, &profile)
        }
        Some(TeamCommand::Hire(args)) => {
            let history_summary = match &args.from_session {
                Some(id) => history_summary(client, id).await?,
                None => String::new(),
            };
            let draft = HireDraft {
                name: args.name.clone(),
                initials: None,
                color: None,
                persona: AgentPersona {
                    role: args.role.clone(),
                    responsibilities: vec![args.role.clone()],
                    ..AgentPersona::default()
                },
                skills: None,
                model: None,
                account_pool: None,
                channel: None,
                mode: None,
                routine_ids: Vec::new(),
                memory_ids: Vec::new(),
                history_summary,
                proposal_key: None,
            };
            let agent: Agent = client.json("POST", "/v1/agents", Some(&draft)).await?;
            print_agent(json, stdout, &agent)
        }
        Some(TeamCommand::Retire { id, hand_to, pause }) => {
            let agent: Agent = client
                .json(
                    "POST",
                    &agent_path(id, "retire"),
                    Some(&RetireAgentRequest {
                        successor: hand_to.clone(),
                        pause_routines: *pause,
                        revert_hire: false,
                    }),
                )
                .await?;
            print_agent(json, stdout, &agent)
        }
        Some(TeamCommand::Undo { id }) => {
            let agent: Agent = client
                .json(
                    "POST",
                    &agent_path(id, "retire"),
                    Some(&RetireAgentRequest {
                        revert_hire: true,
                        ..RetireAgentRequest::default()
                    }),
                )
                .await?;
            print_agent(json, stdout, &agent)
        }
        Some(TeamCommand::Restore { id }) => {
            let agent: Agent = client
                .json::<_, Value>("POST", &agent_path(id, "restore"), None)
                .await?;
            print_agent(json, stdout, &agent)
        }
        Some(TeamCommand::Rename { id, name }) => {
            patch(
                client,
                stdout,
                json,
                id,
                UpdateAgentRequest {
                    name: Some(name.clone()),
                    ..UpdateAgentRequest::default()
                },
            )
            .await
        }
        Some(TeamCommand::Hide { id }) => {
            patch(
                client,
                stdout,
                json,
                id,
                UpdateAgentRequest {
                    hidden: Some(true),
                    ..UpdateAgentRequest::default()
                },
            )
            .await
        }
        Some(TeamCommand::Unhide { id }) => {
            patch(
                client,
                stdout,
                json,
                id,
                UpdateAgentRequest {
                    hidden: Some(false),
                    ..UpdateAgentRequest::default()
                },
            )
            .await
        }
        Some(TeamCommand::Proposals { command: None }) => {
            let list: SuppressedProposalList = client.get("/v1/team/proposals/suppressed").await?;
            if json {
                return emit(stdout, &list);
            }
            if list.is_empty() {
                return writeln!(stdout, "No hire suggestions are turned off.")
                    .map_err(output_error);
            }
            for item in list {
                writeln!(
                    stdout,
                    "{}\t{}\t{}\t{}",
                    clean(&item.key),
                    clean(item.label.as_deref().unwrap_or(&item.key)),
                    clean(&item.status),
                    item.decided_at
                )
                .map_err(output_error)?;
            }
            Ok(())
        }
        Some(TeamCommand::Proposals {
            command: Some(ProposalCommand::Allow { key }),
        }) => {
            client
                .send(
                    "DELETE",
                    &format!("/v1/team/proposals/{}", client::encode_segment(key)),
                )
                .await?;
            if json {
                return Ok(());
            }
            writeln!(stdout, "Hire suggestions allowed for {}", clean(key)).map_err(output_error)
        }
    }
}

pub(crate) async fn chief_home(client: &Client) -> Result<String, String> {
    let chief: Agent = client.get("/v1/agents/chief").await?;
    Ok(chief.home_session_id.to_string())
}

fn agent_path(id: &str, suffix: &str) -> String {
    let id = client::encode_segment(id);
    if suffix.is_empty() {
        format!("/v1/agents/{id}")
    } else {
        format!("/v1/agents/{id}/{suffix}")
    }
}

async fn patch<W: Write>(
    client: &Client,
    stdout: &mut W,
    json: bool,
    id: &str,
    request: UpdateAgentRequest,
) -> Result<(), String> {
    let agent: Agent = client
        .json("PATCH", &agent_path(id, ""), Some(&request))
        .await?;
    print_agent(json, stdout, &agent)
}

fn print_agent<W: Write>(json: bool, stdout: &mut W, agent: &Agent) -> Result<(), String> {
    if json {
        return emit(stdout, agent);
    }
    writeln!(
        stdout,
        "{} ({})\nRole: {}\nStatus: {}\nHidden: {}\nHome: {}",
        clean(&agent.name),
        clean(&agent.id),
        clean(&agent.persona.role),
        status(agent.status),
        agent.hidden,
        clean(agent.home_session_id.as_str())
    )
    .map_err(output_error)?;
    for line in &agent.persona.responsibilities {
        writeln!(stdout, "Owns: {}", clean(line)).map_err(output_error)?;
    }
    Ok(())
}

fn status(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Active => "active",
        AgentStatus::Retired => "retired",
    }
}

async fn history_summary(client: &Client, id: &str) -> Result<String, String> {
    const MAX_BYTES: usize = 12_000;
    let session: Session = client.get(&client::session_path(id, "")).await?;
    let after = session.last_seq.saturating_sub(512);
    let page: EventPoll = client
        .get(&format!(
            "{}?after={after}&wait=0",
            client::session_path(id, "events/poll")
        ))
        .await?;
    let mut excerpt = String::new();
    for event in page
        .events
        .into_iter()
        .filter(|event| event.seq <= session.last_seq)
    {
        let (label, text) = match event.kind {
            EventKind::MessageQueued { text, .. } => ("Message", text),
            EventKind::AssistantFinal { text, .. } => ("Reply", text),
            _ => continue,
        };
        let mut start = text.len().saturating_sub(MAX_BYTES / 2);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        excerpt.push_str(label);
        excerpt.push_str(": ");
        excerpt.push_str(&text[start..]);
        excerpt.push('\n');
        if excerpt.len() > MAX_BYTES / 2 {
            let mut start = excerpt.len() - MAX_BYTES / 2;
            while !excerpt.is_char_boundary(start) {
                start += 1;
            }
            excerpt.drain(..start);
        }
    }
    let mut summary = format!(
        "Recent conversation excerpt from {} ({})\n",
        session.id, session.title
    );
    summary.push_str(&excerpt);
    let mut end = summary.len().min(MAX_BYTES);
    while !summary.is_char_boundary(end) {
        end -= 1;
    }
    summary.truncate(end);
    Ok(summary)
}
