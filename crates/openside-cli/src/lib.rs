//! Non-interactive client of the openside public API v1.
//!
//! `openside "task"` creates a session and follows its SSE stream. Session commands
//! are `session list`, `session resume <id> [text] [--last-event-id <seq>]`, and
//! `session stop <id>`. Tickets support `ticket list [--all]`, `show <id>`,
//! `resolve <id> <value>`, `done <id>`, and `reject <id> [--reason TEXT]`.
//! Resolution values are parsed as JSON when valid, otherwise as strings.
//! For `secret_input`, `resolve <id>` with no value asks for it at a hidden prompt on the
//! terminal (nothing is echoed), and `resolve <id> -` reads a string from stdin, removing one
//! trailing LF or CRLF; both keep secrets out of argv and shell history.
//! `--totp` with either form instead sends the input as `{"totp": ...}`, registering the site's
//! authenticator on a `request_otp` ticket that offers it.
//! A `login` ticket asks one prompt per page field (hidden for text fields, `[y/N]` for
//! checkboxes) and sends `{"values": ..., "save": ...}`; saving follows the site's `can_save`
//! unless `--no-save` (`--save` insists and fails where saving is not allowed).
//! `--alternative ID` and `--account ITEM_ID` answer it without typing. A `choice` ticket lists
//! the page's options and sends `{"option": id}` for the picked number. `-` reads the same
//! answers from piped stdin, one line per prompt.
//! Apart from `repl`, `vault init|unlock|add`, and `key set`, these are the only stdin-reading
//! commands, and they never print the supplied value.
//! `--model`, `--mode read-only|guard|full`, and `--workspace PATH` select session
//! creation settings; workspace paths are canonicalized to absolute paths.
//! `doctor [--config PATH] [--json]` diagnoses local installation without a daemon.
//! `models` lists grouped model readiness; `key set <provider>` reads a hidden stdin line and
//! stores the value in the Vault; `key list|rm <provider>` never prints stored values.
//! `routine list|show|add|run|runs|pause|resume|rm|suggest` manage scheduled routines
//! (`docs/specs/routines.md`); `add` takes `--cron EXPR [--timezone ZONE]` or
//! `--interval-secs N`, and `--heartbeat SESSION` to follow up an existing session.
//! `memory search|add|show|edit|rm|history` manage local memory (`docs/specs/memory.md`).
//! `token create --scope SCOPE...` mints a scoped device token (admin, normally over the
//! socket) and prints `device_id<TAB>token`. `policy show|set <file>|test <target>
//! <capability>` reads, replaces or evaluates the live daemon policy (`docs/specs/policy.md`).
//! `vault status|init|unlock|lock|add|rm|items|policy|audit` sets up, unlocks and administers
//! the vault; `init`, `unlock` and `add` read passphrases and values at prompts (one per line
//! from piped stdin) and nothing prints a stored value (`docs/specs/vault.md`).
//! `repl <session-id> [--tab ID] [--eval CODE | --interactive]` runs Playwright-syntax
//! JavaScript in one of the session's agent tabs (`docs/specs/browser-repl.md`); without
//! `--eval` it reads the snippet from stdin, or one snippet per line with `--interactive`.
//! `memory model fetch <owner/name>` runs without a daemon and is the only command that
//! reaches the internet: it downloads a static embedding model over HTTPS (redirects
//! allowed) into `memory.embedding.model_dir`.
//! `skills list|show|new|enable|disable|rm` manage the skill library (`docs/specs/skills.md`);
//! `skills install <agent>` puts openside's browser skill into another agent's skills
//! directory and `skills import` copies a skill into the library; both run without a daemon.
//! `mcp` serves the public API as an MCP server on stdin/stdout (`docs/specs/mcp.md`); it uses
//! the same connection and token as every other command, so the token's scopes bound what an
//! MCP client can do. `--json` has no effect: stdout carries only the MCP protocol.
//! `run GOAL [--session ID] [--wait|--follow]` delegates a goal to the openside browser agent
//! (detached by default; `--wait` waits through human tickets and prints the replies) and
//! `steer`, `cancel` and `tasks` manage the run and its Tasks. `browser <verb> <session> ARGS`
//! calls one of the session's browser or human-step tools directly, and `session new` creates
//! a session to drive (`docs/specs/mcp.md`).
//! `remote setup|status` configures and inspects this daemon's remote transports, `connect`
//! saves another daemon as a profile and `remote profiles list|rm` manages the saved profiles
//! that `--remote NAME` selects (`docs/REMOTE.md`).
//!
//! The default connection is the daemon socket resolved from the shared configuration
//! (`daemon.socket`/`OPENSIDE_SOCKET`, else `<runtime dir>/opensided.sock`).
//! `--socket` overrides it. `--url` accepts only HTTP loopback addresses and needs
//! `--token` or `OPENSIDE_TOKEN`; redirects, proxies, and DNS are never used.
//! `--json` emits raw API objects/events and a final `{"type":"summary", ...}`
//! object, one per line. `openside "task"` does this by default when stdout is not a
//! terminal (`openside "x" | cat`); other commands need `--json` explicitly. Ticket
//! instructions go to stderr, including in JSON mode.
//! No command requires a TTY. A settled session with pending tickets exits successfully
//! with status `waiting_human`.
//!
//! On a terminal the output is designed for people: a branded `--help`, aligned
//! tables, status glyphs, grouped `doctor` sections, and a live `openside "task"` view
//! (markdown text on stdout; tool rows, a spinner and the final summary on stderr).
//! When stdout is not a terminal, and for `--json`, output stays exactly the plain
//! tab-separated or JSONL text, with no ANSI codes. `--color auto|always|never`
//! overrides detection; `NO_COLOR` and `CLICOLOR=0` or `TERM=dumb` turn colour off,
//! `CLICOLOR_FORCE` turns it on when piped. `--color always` also asks a piped
//! `openside "task"` for the human view instead of JSONL.
//!
//! Resume cursors contain only a session ID and sequence, in mode-0600 files under
//! `$XDG_STATE_HOME/openside/cli` (or `$HOME/.local/state/openside/cli`). They are
//! isolated by endpoint and session; explicit `--last-event-id` overrides a cursor.
//! Replaying history does not stop at an older turn's settlement: resume follows
//! the current session state, and an optional new message waits for its new turn.
//! Exit codes are 0 for success/help, 1 for API, connection, protocol, runtime, or
//! output/persistence errors, and 2 for invalid usage or connection configuration.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod accounts;
mod agent;
mod browser;
mod channels;
mod client;
mod connections;
mod cursor;
mod import;
mod mcp;
mod memory;
mod models;
mod pair;
mod policy;
mod prompt;
mod reference;
mod remote;
mod remotes;
mod repl;
mod routines;
mod service;
mod skills;
mod sse;
mod sync;
mod team;
mod token;
mod tool_call;
mod ui;
mod vault;
mod wait;

use std::{
    collections::BTreeSet,
    ffi::OsString,
    io::{Read, Write},
    path::PathBuf,
};

use clap::{Parser, Subcommand, ValueEnum};
use openside_config::doctor::{self, DoctorOptions, Status};
use openside_config::StoredVault;
use openside_proto::{
    BrowserSpace, CreateSessionRequest, Event, EventKind, LoginFieldKind, Message, MessageMode,
    MessageOutcome, PolicyMode, RejectTicketRequest, ResolveTicketRequest, Session, SessionList,
    SessionStatus, SubmitMessageRequest, Ticket, TicketChoice, TicketKind, TicketList, TicketLogin,
};
use openside_vault::StoredFormat;
use serde::Serialize;
use serde_json::{json, Value};

use client::Client;
use cursor::Cursor;
use sse::Decoder;

#[derive(Parser)]
#[command(
    name = "openside",
    version,
    about = "Non-interactive openside API client",
    arg_required_else_help = true,
    color = clap::ColorChoice::Never,
    styles = ui::clap_styles()
)]
struct Args {
    #[arg(
        long,
        global = true,
        help = "Emit JSON objects, one per line, with a final summary (task: the default when stdout is not a terminal)"
    )]
    json: bool,
    #[arg(long, global = true, conflicts_with = "url", value_name = "PATH")]
    socket: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        env = "OPENSIDE_REMOTE",
        conflicts_with_all = ["socket", "url"],
        value_name = "NAME",
        help = "Remote daemon profile saved by `openside connect` (see `openside remote profiles list`)"
    )]
    remote: Option<String>,
    #[arg(
        long,
        global = true,
        value_name = "URL",
        help = "Loopback HTTP endpoint (requires a token)"
    )]
    url: Option<String>,
    #[arg(
        long,
        global = true,
        env = "OPENSIDE_TOKEN",
        hide_env_values = true,
        value_name = "TOKEN"
    )]
    token: Option<String>,
    #[arg(
        long,
        global = true,
        value_enum,
        default_value = "auto",
        value_name = "WHEN",
        help = "Colour the human output: auto (a terminal that supports it; NO_COLOR and TERM=dumb turn it off), always (also into a pipe; CLICOLOR_FORCE does the same) or never"
    )]
    color: ui::ColorWhen,
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(value_name = "TASK")]
    task: Option<String>,
    #[arg(long, value_name = "MODEL", help = "Model for the new session")]
    model: Option<String>,
    #[arg(long, value_enum, help = "Permission mode for the new session")]
    mode: Option<Mode>,
    #[arg(long, value_name = "PATH", help = "Workspace for the new session")]
    workspace: Option<PathBuf>,
    #[arg(
        long,
        value_enum,
        help = "Browser space for the new session: shared (default, the managed profile with its logins) or isolated (own cookies and storage, wiped when the session is archived or deleted)"
    )]
    browser_space: Option<Space>,
}

#[derive(Subcommand)]
enum Command {
    /// Show credential-source accounts, configured models, usage and rate-limit blocks.
    Accounts {
        #[command(subcommand)]
        command: Option<accounts::AccountCommand>,
    },
    /// List models grouped by provider, with credential readiness and source.
    Models {
        #[command(subcommand)]
        command: Option<models::ModelCommand>,
    },
    /// Manage encrypted provider API keys (set reads from stdin, never argv).
    Key {
        #[command(subcommand)]
        command: models::KeyCommand,
    },
    /// Manage independent chat channel accounts; secrets use hidden stdin only.
    Channels {
        #[command(subcommand)]
        command: channels::ChannelCommand,
    },
    /// Manage the chief and durable specialist teammates.
    Team {
        #[command(subcommand)]
        command: Option<team::TeamCommand>,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Ticket {
        #[command(subcommand)]
        command: TicketCommand,
    },
    Routine {
        #[command(subcommand)]
        command: routines::RoutineCommand,
    },
    Memory {
        #[command(subcommand)]
        command: memory::MemoryCommand,
    },
    /// Mint scoped device tokens.
    Token {
        #[command(subcommand)]
        command: token::TokenCommand,
    },
    /// List vault items, set per-item access policy, read the vault audit (admin).
    Vault {
        #[command(subcommand)]
        command: vault::VaultCommand,
    },
    /// Manage browser sign-ins, per-agent grants and audit records (admin).
    Connections {
        #[command(subcommand)]
        command: connections::ConnectionsCommand,
    },
    /// End-to-end encrypted sync of pinned tabs and vault items between devices.
    Sync {
        #[command(subcommand)]
        command: sync::SyncCommand,
    },
    /// Show, replace or test the live daemon policy.
    Policy {
        #[command(subcommand)]
        command: policy::PolicyCommand,
    },
    Skills {
        #[command(subcommand)]
        command: skills::SkillsCommand,
    },
    /// Delegate a goal to the openside browser agent; prints the session and message ids.
    #[command(visible_alias = "chat")]
    Run {
        /// What the agent should do; `-` reads it from stdin.
        goal: String,
        /// Run in this session instead of the chief's home conversation.
        #[arg(long, value_name = "SESSION_ID")]
        session: Option<String>,
        /// Wait until the run ends (through any tickets a person answers) and print the replies.
        #[arg(long, conflicts_with = "follow")]
        wait: bool,
        /// Stream the run's text, progress and tickets until the session settles.
        #[arg(long)]
        follow: bool,
        /// With --wait: give up after this many seconds (status `timeout`, exit 1).
        #[arg(long, value_name = "SECS", requires = "wait")]
        timeout: Option<u64>,
    },
    /// Steer a running agent, or one of its Tasks with --task.
    Steer {
        /// Session to steer.
        session: String,
        /// The guidance to send.
        text: String,
        /// Steer this Task instead of the session's agent (ids from `openside tasks`).
        #[arg(long, value_name = "TASK_ID")]
        task: Option<String>,
    },
    /// Cancel one Task with --task, otherwise stop the session's run.
    Cancel {
        /// Session whose run or Task to cancel.
        session: String,
        /// Cancel only this Task; the agent and the other Tasks keep running.
        #[arg(long, value_name = "TASK_ID")]
        task: Option<String>,
    },
    /// List a session's Tasks and subagents with their status and results.
    Tasks {
        /// Session to list.
        session: String,
    },
    /// Call the session's browser and human-step tools directly, like the in-app agent.
    Browser {
        #[command(subcommand)]
        command: browser::BrowserCommand,
    },
    /// Run Playwright-syntax JavaScript in a session's agent tab.
    Repl {
        /// Session whose agent tab runs the code.
        session: String,
        /// Agent tab to drive; optional when the session owns at most one tab.
        #[arg(long = "tab", value_name = "TAB_ID")]
        tab: Option<String>,
        /// Run this snippet instead of reading stdin.
        #[arg(long, value_name = "CODE", conflicts_with = "interactive")]
        eval: Option<String>,
        /// Run every stdin line as its own snippet.
        #[arg(long, short)]
        interactive: bool,
    },
    /// Serve the public API to MCP clients over stdio (docs/specs/mcp.md).
    Mcp,
    /// Import history, bookmarks and cookies from a Chrome, Chromium or Firefox profile.
    Import(import::ImportArgs),
    Doctor {
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
    },
    /// Pair a phone or browser: print a one-time QR code valid for 3 minutes (local socket).
    Pair(pair::PairArgs),
    /// Pair this computer with a remote daemon from its pairing link and save it as a
    /// `--remote` profile.
    Connect(remotes::ConnectArgs),
    /// This daemon's remote access (Tailscale, Cloudflare Tunnel or a relay) and the saved
    /// profiles for reaching other daemons.
    Remote {
        #[command(subcommand)]
        command: remote::RemoteCommand,
    },
    /// List or revoke paired devices (admin).
    Device {
        #[command(subcommand)]
        command: pair::DeviceCommand,
    },
    /// Run opensided as a per-user systemd or launchd service.
    Service {
        #[command(subcommand)]
        command: service::ServiceCommand,
    },
}

#[derive(Subcommand)]
enum SessionCommand {
    List,
    /// Create an empty session to drive with `browser` or `repl`; prints its id.
    New {
        #[arg(long)]
        title: Option<String>,
    },
    Resume {
        id: String,
        text: Option<String>,
        #[arg(
            long,
            value_name = "SEQ",
            help = "Override the persisted last-seen SSE sequence"
        )]
        last_event_id: Option<u64>,
    },
    Stop {
        id: String,
    },
}

#[derive(Subcommand)]
enum TicketCommand {
    List {
        #[arg(long, help = "Include tickets in every status")]
        all: bool,
    },
    Show {
        id: String,
    },
    Resolve {
        id: String,
        #[arg(
            allow_hyphen_values = true,
            help = "JSON or text; secret, login, OTP and payment values only through a hidden prompt or '-'; form/login/choice stdin uses one line per field"
        )]
        value: Option<String>,
        #[arg(
            long,
            help = "With '-' or the prompt: the input is the site's authenticator (otpauth:// URI or base32 setup key) to save in the vault instead of a code; request_otp tickets that offer registration only"
        )]
        totp: bool,
        #[arg(
            long,
            conflicts_with = "no_save",
            help = "Login tickets: save the login to the vault (the default when the site allows it; needs the approve scope)"
        )]
        save: bool,
        #[arg(long, help = "Login tickets: log in without saving to the vault")]
        no_save: bool,
        #[arg(
            long,
            value_name = "ID",
            conflicts_with_all = ["value", "account", "save", "no_save"],
            help = "Login tickets: use the page's other sign-in option ID (see `ticket show`) instead of typing"
        )]
        alternative: Option<String>,
        #[arg(
            long,
            value_name = "ITEM_ID",
            conflicts_with_all = ["value", "save", "no_save"],
            help = "Login tickets: log in with this saved vault account (see `ticket show`)"
        )]
        account: Option<String>,
    },
    Done {
        id: String,
    },
    Reject {
        id: String,
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    ReadOnly,
    Guard,
    Full,
}

impl From<Mode> for PolicyMode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::ReadOnly => Self::ReadOnly,
            Mode::Guard => Self::Guard,
            Mode::Full => Self::Full,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Space {
    Shared,
    Isolated,
}

impl From<Space> for BrowserSpace {
    fn from(space: Space) -> Self {
        match space {
            Space::Shared => Self::Shared,
            Space::Isolated => Self::Isolated,
        }
    }
}

#[derive(Default)]
struct Summary {
    session_id: Option<String>,
    last_seq: Option<u64>,
    status: &'static str,
    failed: bool,
}

/// Markdown reference of every command and option, generated from the clap definitions.
///
/// This is the content of `docs/reference/cli.md`; a test fails when the committed file differs.
pub fn reference_markdown() -> String {
    reference::markdown()
}

/// Tool names the `browser` verbs call, sorted; the daemon's `GET /v1/tools` catalog must list
/// exactly these (checked in openside-e2e).
pub fn browser_verb_tools() -> Vec<&'static str> {
    browser::verb_tools()
}

/// Runs the CLI using argv (including the executable name) and caller-owned writers.
///
/// Returns the documented process exit code without exiting the process. Only
/// `ticket resolve <id> -` for a secret-input ticket reads stdin. The caller must
/// provide a Tokio runtime and configure the process's XDG environment.
pub async fn run<I, T, W, E>(args: I, stdout: &mut W, stderr: &mut E) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
    W: Write,
    E: Write,
{
    run_with_input(args, &mut std::io::stdin(), stdout, stderr).await
}

/// Runs the CLI against the process's real stdin, stdout and stderr, as `main` does.
///
/// When stdout is not a terminal (`openside "x" | cat`, a CI log, a pipe to `jq`) a task
/// invocation emits JSONL exactly as if `--json` had been passed, so a script never has to
/// guess the format. On a terminal the same command prints human text.
pub async fn run_process<I, T>(args: I) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let argv: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let term = ui::Terminal::from_process();
    let detected = ui::Ui::detect(ui::scan_color(&argv), &ui::Env::from_process(), &term);
    if let Some(detected) = detected {
        ui::install(detected);
    }
    let mut stdout = ui::stream(std::io::stdout(), detected.is_some_and(|ui| ui.color));
    let mut stderr = ui::stream(std::io::stderr(), detected.is_some_and(|ui| ui.err_color));
    run_inner(
        argv,
        &mut std::io::stdin(),
        &mut stdout,
        &mut stderr,
        !term.stdout_tty,
    )
    .await
}

/// Runs the CLI with caller-owned stdin for non-interactive embedding.
pub async fn run_with_input<I, T, R, W, E>(
    args: I,
    stdin: &mut R,
    stdout: &mut W,
    stderr: &mut E,
) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
    R: Read,
    W: Write,
    E: Write,
{
    run_inner(args, stdin, stdout, stderr, false).await
}

async fn run_inner<I, T, R, W, E>(
    args: I,
    stdin: &mut R,
    stdout: &mut W,
    stderr: &mut E,
    json_when_task: bool,
) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
    R: Read,
    W: Write,
    E: Write,
{
    let argv: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let key_input = argv
        .windows(2)
        .any(|pair| pair[0] == "key" && pair[1] == "set");
    let channel_input = argv
        .windows(2)
        .any(|pair| pair[0] == "channels" && pair[1] == "add");
    let parsed = match ui::pretty() {
        Some(ui) => {
            use clap::{CommandFactory, FromArgMatches};
            Args::command()
                .term_width(ui.width)
                .try_get_matches_from(argv)
                .and_then(|mut matches| {
                    Args::from_arg_matches_mut(&mut matches)
                        .map_err(|error| error.format(&mut Args::command()))
                })
        }
        None => Args::try_parse_from(argv),
    };
    let args = match parsed {
        Ok(mut args) => {
            args.json |=
                json_when_task && args.task.is_some() && args.color != ui::ColorWhen::Always;
            args
        }
        Err(error) => {
            let usage = error.use_stderr();
            let result = if usage {
                if key_input {
                    writeln!(stderr, "openside: invalid key command; use `openside key set <provider>` and supply the key on stdin")
                } else if channel_input {
                    writeln!(stderr, "openside: invalid channel command; use `openside channels add <kind> <id> --label <label>` and supply credentials on hidden stdin")
                } else if let Some(ui) = ui::pretty() {
                    write!(stderr, "{}", ui::render_help_error(&error, ui))
                } else {
                    write!(stderr, "{error}")
                }
            } else if let Some(ui) = ui::pretty() {
                write!(stdout, "{}", ui::render_help_error(&error, ui))
            } else {
                write!(stdout, "{error}")
            };
            return if result.is_err() {
                1
            } else if usage {
                2
            } else {
                0
            };
        }
    };
    if args.task.is_some() == args.command.is_some() {
        let _ = writeln!(
            stderr,
            "openside: provide either a task or a subcommand, not both"
        );
        if args.json {
            let _ = emit(
                stdout,
                &json!({"type":"summary","status":"error","exit_code":2}),
            );
        }
        return 2;
    }
    let creates_session = matches!(
        args.command,
        None | Some(Command::Session {
            command: SessionCommand::New { .. }
        })
    );
    if !creates_session
        && (args.model.is_some()
            || args.mode.is_some()
            || args.workspace.is_some()
            || args.browser_space.is_some())
    {
        let _ = writeln!(
            stderr,
            "openside: --model, --mode, --workspace and --browser-space apply only to new sessions; use `openside session new --model MODEL --mode MODE --workspace PATH --browser-space SPACE` or a bare task"
        );
        if args.json {
            let _ = emit(
                stdout,
                &json!({"type":"summary","status":"error","exit_code":2}),
            );
        }
        return 2;
    }
    if matches!(args.command, Some(Command::Mcp)) {
        return mcp::run(
            args.socket.as_deref(),
            args.url.as_deref(),
            args.token,
            stderr,
            args.remote.clone(),
        )
        .await;
    }
    let mut summary = Summary::default();
    let result = if let Some(Command::Doctor { config }) = &args.command {
        run_doctor(config.as_deref(), args.json, stdout, &mut summary).await
    } else if let Some(Command::Import(import_args)) = &args.command {
        import::run(import_args, args.json, stdout).await
    } else if let Some(Command::Service { command }) = &args.command {
        service::execute(command, stdout)
    } else if let Some(Command::Connect(connect)) = &args.command {
        remotes::connect(connect, args.json, stdin, stdout, stderr).await
    } else if let Some(Command::Remote {
        command: remote::RemoteCommand::Profiles { command },
    }) = &args.command
    {
        remotes::remotes(command, args.json, stdout)
    } else if let Some(Command::Remote {
        command: remote::RemoteCommand::Setup(setup),
    }) = &args.command
    {
        remote::setup(setup, args.json, stdout).await
    } else if let Some(Command::Memory {
        command: memory::MemoryCommand::Model { command },
    }) = &args.command
    {
        memory::model(command, args.json, stdout).await
    } else if let Some(Command::Skills { command }) = args
        .command
        .as_ref()
        .filter(|command| matches!(command, Command::Skills { command } if command.is_local()))
    {
        skills::local(command, args.json, stdout)
    } else {
        let client = match Client::from_flags(
            args.socket.as_deref(),
            args.url.as_deref(),
            args.token.as_deref(),
            args.remote.as_deref(),
        )
        .await
        {
            Ok(client) => client,
            Err(error) => {
                let _ = writeln!(stderr, "openside: {error}");
                if args.json {
                    let _ = emit(
                        stdout,
                        &json!({"type":"summary","status":"error","exit_code":2}),
                    );
                }
                return 2;
            }
        };
        execute(&args, &client, stdin, stdout, stderr, &mut summary).await
    };
    let exit_code = if let Err(error) = result {
        let detail = match args.token.as_deref().filter(|token| !token.is_empty()) {
            Some(token) => error.replace(token, "[REDACTED]"),
            None => error,
        };
        let _ = writeln!(stderr, "openside: {detail}");
        summary.status = "error";
        1
    } else {
        // Commands that run without a daemon never set a status of their own.
        if summary.status.is_empty() {
            summary.status = "ok";
        }
        u8::from(summary.failed)
    };
    if args.json
        && emit(
            stdout,
            &json!({
                "type":"summary", "status":summary.status, "exit_code":exit_code,
                "session_id":summary.session_id, "last_seq":summary.last_seq,
            }),
        )
        .is_err()
    {
        return 1;
    }
    if stdout.flush().is_err() || stderr.flush().is_err() {
        return 1;
    }
    exit_code
}

async fn execute<R: Read, W: Write, E: Write>(
    args: &Args,
    client: &Client,
    stdin: &mut R,
    stdout: &mut W,
    stderr: &mut E,
    summary: &mut Summary,
) -> Result<(), String> {
    summary.status = "ok";
    match &args.command {
        Some(Command::Accounts { command }) => {
            accounts::run(command.as_ref(), client, args.json, stdout).await
        }
        Some(Command::Models { command }) => {
            models::run(command.as_ref(), client, args.json, stdout).await
        }
        Some(Command::Key { command }) => {
            models::key(command, client, args.json, stdin, stdout, stderr).await
        }
        Some(Command::Channels { command }) => {
            channels::run(command, client, args.json, stdin, stdout, stderr).await
        }
        Some(Command::Team { command }) => {
            team::execute(command.as_ref(), args.json, client, stdout).await
        }
        None => {
            let task = args
                .task
                .as_deref()
                .ok_or("a task or subcommand is required")?;
            let session: Session = client
                .json("POST", "/v1/sessions", Some(&create_request(args, None)?))
                .await?;
            summary.session_id = Some(session.id.to_string());
            send_message(client, session.id.as_str(), task).await?;
            follow(
                args,
                client,
                session.id.as_str(),
                ReplayPoint {
                    last_seen: 0,
                    terminal_after: session.last_seq,
                    end_at: None,
                },
                stdout,
                stderr,
                summary,
            )
            .await
        }
        Some(Command::Run {
            goal,
            session,
            wait,
            follow: stream,
            timeout,
        }) => {
            let goal = if goal == "-" {
                let mut text = String::new();
                stdin
                    .read_to_string(&mut text)
                    .map_err(|_| "could not read the goal from stdin".to_owned())?;
                text
            } else {
                goal.clone()
            };
            if goal.trim().is_empty() {
                return Err("the goal is empty".into());
            }
            let create = create_request(args, None)?;
            let default_home = match session {
                None => Some(team::chief_home(client).await?),
                Some(_) => None,
            };
            let target = session.as_deref().or(default_home.as_deref());
            if *stream {
                let started = agent::start(client, target, &create, &goal).await?;
                summary.session_id = Some(started.session.id.to_string());
                return follow(
                    args,
                    client,
                    started.session.id.as_str(),
                    ReplayPoint {
                        last_seen: started.session.last_seq,
                        terminal_after: started.session.last_seq,
                        end_at: None,
                    },
                    stdout,
                    stderr,
                    summary,
                )
                .await;
            }
            agent::run(
                agent::RunOptions {
                    goal: &goal,
                    session: target,
                    create,
                    wait: *wait,
                    timeout: timeout.map(std::time::Duration::from_secs),
                    json: args.json,
                },
                client,
                stdout,
                stderr,
                summary,
            )
            .await
        }
        Some(Command::Steer {
            session,
            text,
            task,
        }) => {
            agent::steer(
                client,
                session,
                task.as_deref(),
                text,
                args.json,
                stdout,
                summary,
            )
            .await
        }
        Some(Command::Cancel { session, task }) => {
            agent::cancel(client, session, task.as_deref(), args.json, stdout, summary).await
        }
        Some(Command::Tasks { session }) => {
            agent::tasks(client, session, args.json, stdout, summary).await
        }
        Some(Command::Browser { command }) => {
            browser::execute(command, args.json, client, stdout, stderr, summary).await
        }
        Some(Command::Session { command }) => match command {
            SessionCommand::List => {
                let list: SessionList = client.get("/v1/sessions").await?;
                if args.json {
                    emit(stdout, &list)?;
                } else if let Some(ui) = ui::pretty() {
                    write!(stdout, "{}", ui::sessions(ui, &list)).map_err(output_error)?;
                } else {
                    for session in &list.sessions {
                        writeln!(
                            stdout,
                            "{}\t{}\t{}",
                            session.id,
                            status_name(session.status),
                            session.title
                        )
                        .map_err(output_error)?;
                    }
                }
                Ok(())
            }
            SessionCommand::New { title } => {
                let session: Session = client
                    .json(
                        "POST",
                        "/v1/sessions",
                        Some(&create_request(args, title.clone())?),
                    )
                    .await?;
                summary.session_id = Some(session.id.to_string());
                summary.last_seq = Some(session.last_seq);
                if args.json {
                    emit(stdout, &session)
                } else {
                    writeln!(stdout, "{}", session.id).map_err(output_error)
                }
            }
            SessionCommand::Stop { id } => {
                let session: Session = client
                    .json::<_, Value>("POST", &client::session_path(id, "stop"), None)
                    .await?;
                summary.session_id = Some(session.id.to_string());
                summary.last_seq = Some(session.last_seq);
                if args.json {
                    emit(stdout, &session)?;
                } else {
                    writeln!(stdout, "Stopped session {}", session.id).map_err(output_error)?;
                }
                Ok(())
            }
            SessionCommand::Resume {
                id,
                text,
                last_event_id,
            } => {
                let session: Session = client.get(&client::session_path(id, "")).await?;
                let cursor = Cursor::new(client.endpoint_key(), id)?;
                let last_seq = match last_event_id {
                    Some(seq) => *seq,
                    None => cursor.load()?,
                };
                if last_seq > session.last_seq {
                    return Err("last-seen sequence is ahead of the session event log".into());
                }
                summary.session_id = Some(id.clone());
                summary.last_seq = Some(last_seq);
                if let Some(text) = text {
                    send_message(client, id, text).await?;
                } else if last_seq == session.last_seq && session.status != SessionStatus::Running {
                    summary.status = status_name(session.status);
                    print_waiting(args.json, stdout, id, summary.status)?;
                    return Ok(());
                }
                let terminal_after = if text.is_some() || session.status == SessionStatus::Running {
                    session.last_seq
                } else {
                    session.last_seq.saturating_sub(1)
                };
                let end_at = if text.is_none() && session.status != SessionStatus::Running {
                    Some((
                        session.last_seq,
                        match session.status {
                            SessionStatus::Idle => "settled",
                            status => status_name(status),
                        },
                    ))
                } else {
                    None
                };
                follow(
                    args,
                    client,
                    id,
                    ReplayPoint {
                        last_seen: last_seq,
                        terminal_after,
                        end_at,
                    },
                    stdout,
                    stderr,
                    summary,
                )
                .await
            }
        },
        Some(Command::Ticket { command }) => match command {
            TicketCommand::List { all } => {
                let path = if *all {
                    "/v1/tickets"
                } else {
                    "/v1/tickets?status=pending"
                };
                let list: TicketList = client.get(path).await?;
                if args.json {
                    emit(stdout, &list)?;
                } else if list.tickets.iter().any(|ticket| ticket.card.is_some()) {
                    for ticket in &list.tickets {
                        print_ticket(false, client, stdout, ticket, summary)?;
                    }
                } else if let Some(ui) = ui::pretty() {
                    write!(stdout, "{}", ui::tickets(ui, &list, ui::now_secs()))
                        .map_err(output_error)?;
                } else {
                    for ticket in &list.tickets {
                        writeln!(
                            stdout,
                            "{}\t{}\t{}\t{}",
                            ticket.id,
                            ticket.session_id,
                            ticket_kind(ticket.kind),
                            plain(&ticket.summary)
                        )
                        .map_err(output_error)?;
                    }
                }
                Ok(())
            }
            TicketCommand::Show { id } => {
                let ticket: Ticket = client.get(&client::ticket_path(id, "")).await?;
                print_ticket(args.json, client, stdout, &ticket, summary)?;
                if !args.json && ticket.card.is_none() {
                    print_ticket_details(stdout, &ticket)?;
                }
                Ok(())
            }
            TicketCommand::Resolve {
                id,
                value,
                totp,
                save,
                no_save,
                alternative,
                account,
            } => {
                let ask = value.is_none();
                let piped = value.as_deref() == Some("-");
                let login_flags = *save || *no_save || alternative.is_some() || account.is_some();
                if *totp && !ask && !piped {
                    return Err(
                        "--totp reads the authenticator from stdin or the hidden prompt; pass '-' or omit the value"
                            .to_owned(),
                    );
                }
                if login_flags && !ask && !piped {
                    return Err(
                        "--save, --no-save, --alternative and --account answer a login ticket at its prompts; omit the value"
                            .to_owned(),
                    );
                }
                // Fetch metadata even for argv values so private fields cannot bypass stdin handling.
                let fetched: Ticket = client.get(&client::ticket_path(id, "")).await?;
                let kind = Some(fetched.kind);
                let private = fetched.kind == TicketKind::SecretInput
                    || fetched.kind == TicketKind::Login
                    || fetched.form.as_ref().is_some_and(|form| {
                        form.payment || form.fields.iter().any(ui::cards::sensitive)
                    });
                if private && !ask && !piped {
                    return Err("private fields must use the hidden prompt or stdin; omit the value or pass '-'".to_owned());
                }
                if *totp && kind != Some(TicketKind::SecretInput) {
                    return Err("--totp only answers a secret_input ticket".to_owned());
                }
                if login_flags && kind != Some(TicketKind::Login) {
                    return Err(
                        "--save, --no-save, --alternative and --account only answer a login ticket"
                            .to_owned(),
                    );
                }
                let prompted = (ask || piped)
                    && (fetched.form.is_some()
                        || matches!(
                            kind,
                            Some(
                                TicketKind::SecretInput
                                    | TicketKind::Login
                                    | TicketKind::Choice
                                    | TicketKind::Info
                            )
                        ));
                if ask && !prompted {
                    return Err(
                        "a value is required; secret_input, login, choice, information and page form tickets support prompts"
                            .to_owned(),
                    );
                }
                let value = match (prompted.then_some(&fetched), fetched.form.as_ref()) {
                    (Some(_), Some(form)) => prompt::form_answer(form, stdin, stderr)?,
                    (Some(ticket), _) if ticket.kind == TicketKind::SecretInput => {
                        secret_answer(ticket, ask, *totp, stdin, stderr)?
                    }
                    (Some(ticket), _) if ticket.kind == TicketKind::Login => {
                        let login = ticket
                            .login
                            .as_ref()
                            .ok_or("this login ticket carries no form")?;
                        login_answer(
                            login,
                            LoginFlags {
                                save: *save,
                                no_save: *no_save,
                                alternative: alternative.as_deref(),
                                account: account.as_deref(),
                            },
                            stdin,
                            stderr,
                        )?
                    }
                    (Some(ticket), _) if ticket.kind == TicketKind::Info => {
                        prompt::information_answer(ticket, stdin, stderr)?
                    }
                    (Some(ticket), _) => {
                        let choice = ticket
                            .choice
                            .as_ref()
                            .ok_or("this choice ticket carries no options")?;
                        choice_answer(choice, stdin, stderr)?
                    }
                    (None, _) => {
                        let value = value.as_deref().unwrap_or_default();
                        serde_json::from_str(value)
                            .unwrap_or_else(|_| Value::String(value.to_owned()))
                    }
                };
                let saving = value.get("save") == Some(&Value::Bool(true));
                let result = client
                    .json(
                        "POST",
                        &client::ticket_path(id, "resolve"),
                        Some(&ResolveTicketRequest { hire: None, value }),
                    )
                    .await;
                let ticket: Ticket = match kind {
                    _ if fetched.form.is_some() => result.map_err(|_| "could not resolve page form (details withheld)".to_owned())?,
                    Some(TicketKind::SecretInput) => result.map_err(|error| {
                        if error.starts_with("API error person_approval_required ") {
                            "could not enroll authenticator: API error person_approval_required (HTTP 403); ask a person to respond on their paired device, then use MCP ticket_wait. Only the operator should issue a person token (openside token create --person)".to_owned()
                        } else {
                            "could not resolve secret-input ticket (details withheld)".to_owned()
                        }
                    })?,
                    Some(TicketKind::Login) => result.map_err(|error| {
                        // Keep the API code and status ("API error forbidden (HTTP 403)"), never
                        // the message, in case anything in it came from the typed values.
                        let head = error.split_once(": ").map_or(error.as_str(), |(head, _)| head);
                        if saving && head.contains("person_approval_required") {
                            return format!("could not save login: {head}; ask a person to respond on their paired device, then use MCP ticket_wait. Only the operator should issue a person token (openside token create --person), or rerun with --no-save");
                        }
                        if saving && head.contains("forbidden") {
                            format!("could not resolve login ticket: {head}; saving to the vault needs the approve scope on this device, rerun with --no-save")
                        } else {
                            format!("could not resolve login ticket: {head} (details withheld)")
                        }
                    })?,
                    _ => result?,
                };
                print_ticket(args.json, client, stdout, &ticket, summary)
            }
            TicketCommand::Done { id } => {
                let ticket: Ticket = client
                    .json::<_, Value>("POST", &client::ticket_path(id, "done"), None)
                    .await?;
                print_ticket(args.json, client, stdout, &ticket, summary)
            }
            TicketCommand::Reject { id, reason } => {
                let ticket: Ticket = client
                    .json(
                        "POST",
                        &client::ticket_path(id, "reject"),
                        Some(&RejectTicketRequest {
                            reason: reason.clone(),
                        }),
                    )
                    .await?;
                print_ticket(args.json, client, stdout, &ticket, summary)
            }
        },
        Some(Command::Routine { command }) => {
            routines::execute(command, args.json, client, stdout).await
        }
        Some(Command::Memory { command }) => {
            memory::execute(command, args.json, client, stdout).await
        }
        Some(Command::Skills { command }) => {
            skills::execute(command, args.json, client, stdout).await
        }
        Some(Command::Token { command }) => {
            token::execute(command, args.json, client, stdout).await
        }
        Some(Command::Policy { command }) => {
            policy::execute(command, args.json, client, stdout).await
        }
        Some(Command::Vault { command }) => {
            vault::execute(command, args.json, client, stdin, stdout, stderr).await
        }
        Some(Command::Connections { command }) => {
            connections::execute(command, args.json, client, stdout, stderr, summary).await
        }
        Some(Command::Sync { command }) => sync::execute(command, args.json, client, stdout).await,
        Some(Command::Repl {
            session,
            tab,
            eval,
            interactive,
        }) => {
            repl::execute(
                repl::ReplOptions {
                    session,
                    tab: tab.as_deref(),
                    eval: eval.as_deref(),
                    interactive: *interactive,
                    json: args.json,
                },
                client,
                stdin,
                stdout,
                stderr,
                summary,
            )
            .await
        }
        Some(Command::Mcp) => unreachable!("mcp serves on stdio before the CLI client runs"),
        Some(Command::Doctor { .. }) => unreachable!("doctor runs without an API client"),
        Some(Command::Import(_)) => unreachable!("import runs without an API client"),
        Some(Command::Pair(pair_args)) => pair::pair(pair_args, args.json, client, stdout).await,
        Some(Command::Device { command }) => pair::device(command, args.json, client, stdout).await,
        Some(Command::Service { .. }) => unreachable!("service runs without an API client"),
        Some(Command::Connect(_)) => unreachable!("connect runs without an API client"),
        Some(Command::Remote { command }) => match command {
            remote::RemoteCommand::Status => remote::status(client, args.json, stdout).await,
            remote::RemoteCommand::LocalAccess => {
                remote::local_access(client, args.json, stdout).await
            }
            remote::RemoteCommand::Setup(_) | remote::RemoteCommand::Profiles { .. } => {
                unreachable!("remote setup and profiles run without an API client")
            }
        },
    }
}

async fn run_doctor<W: Write>(
    config: Option<&std::path::Path>,
    json: bool,
    stdout: &mut W,
    summary: &mut Summary,
) -> Result<(), String> {
    let report = match openside_config::load(config) {
        Ok(loaded) => {
            let path = loaded.config.vault.file(&loaded.paths.data_dir);
            let probe = tokio::task::spawn_blocking(move || doctor::VaultProbe {
                stored: match openside_vault::stored_format(&path) {
                    Ok(None) => Some(StoredVault::Missing),
                    Ok(Some(StoredFormat::Passphrase)) => Some(StoredVault::Passphrase),
                    Ok(Some(StoredFormat::Keyring)) => Some(StoredVault::Keyring),
                    Err(_) => None,
                },
                keyring_reachable: openside_vault::keyring_reachable(),
            })
            .await
            .ok();
            doctor::run(&loaded, DoctorOptions { vault: probe }).await
        }
        Err(error) => doctor::DoctorReport {
            checks: vec![doctor::Check {
                id: "config".into(),
                status: Status::Fail,
                summary: error.to_string(),
                next_step: Some(
                    "Fix the configuration file or select a valid file with --config.".into(),
                ),
            }],
        },
    };
    summary.failed = report
        .checks
        .iter()
        .any(|check| check.status == Status::Fail);
    summary.status = if summary.failed { "error" } else { "ok" };
    if json {
        emit(stdout, &report)
    } else if let Some(ui) = ui::pretty() {
        write!(stdout, "{}", ui::doctor(ui, &report)).map_err(output_error)
    } else {
        write!(stdout, "{}", report.render_text()).map_err(output_error)
    }
}

fn print_ticket<W: Write>(
    json: bool,
    client: &Client,
    stdout: &mut W,
    ticket: &Ticket,
    summary: &mut Summary,
) -> Result<(), String> {
    summary.session_id = Some(ticket.session_id.to_string());
    if json {
        emit(stdout, ticket)
    } else if let Some(card) = &ticket.card {
        let ui = ui::pretty().copied().unwrap_or_else(ui::cards::plain_ui);
        let origin = client.web_origin();
        write!(
            stdout,
            "{}",
            ui::cards::render(
                &ui,
                card,
                &ui::cards::Context {
                    session_id: ticket.session_id.as_str(),
                    web_origin: origin.as_deref(),
                    client: Some(client),
                }
            )
        )
        .map_err(output_error)
    } else {
        writeln!(
            stdout,
            "{}\t{}\t{}",
            ticket.id,
            ticket_kind(ticket.kind),
            plain(&ticket.summary)
        )
        .map_err(output_error)?;
        if let Some(instructions) = &ticket.instructions {
            writeln!(stdout, "{}", plain(instructions)).map_err(output_error)?;
        }
        Ok(())
    }
}

/// What an approver needs to decide: the tool call behind a confirmation, the offered answers,
/// a login form's fields and other sign-ins, a choice's options and the deadline.
fn print_ticket_details<W: Write>(stdout: &mut W, ticket: &Ticket) -> Result<(), String> {
    if let Some(tool) = &ticket.tool {
        writeln!(stdout, "tool: {} {}", tool.name, tool.args).map_err(output_error)?;
    }
    if !ticket.options.is_empty() {
        let options: Vec<&str> = ticket
            .options
            .iter()
            .map(|option| option.label.as_str())
            .collect();
        writeln!(stdout, "options: {}", options.join(", ")).map_err(output_error)?;
    }
    if let Some(login) = &ticket.login {
        let fields: Vec<String> = login
            .fields
            .iter()
            .map(|field| plain(&field.label))
            .collect();
        writeln!(
            stdout,
            "login: {} ({}); fields: {}; save: {}",
            plain(&login.site_name),
            login.origin,
            fields.join(", "),
            if login.can_save {
                "allowed"
            } else {
                "not allowed"
            }
        )
        .map_err(output_error)?;
        for account in login.accounts.iter().flatten() {
            writeln!(
                stdout,
                "account {}: {}",
                account.item_id,
                plain(&account.username_hint)
            )
            .map_err(output_error)?;
        }
        for alternative in &login.alternatives {
            writeln!(
                stdout,
                "alternative {}: {}",
                alternative.id,
                plain(&alternative.label)
            )
            .map_err(output_error)?;
        }
    }
    if let Some(choice) = &ticket.choice {
        writeln!(stdout, "choice: {}", plain(&choice.question)).map_err(output_error)?;
        for option in &choice.options {
            writeln!(stdout, "option {}: {}", option.id, plain(&option.label))
                .map_err(output_error)?;
        }
    }
    writeln!(
        stdout,
        "status: {}, expires {}",
        format!("{:?}", ticket.status).to_lowercase(),
        ticket.expires_at.to_rfc3339()
    )
    .map_err(output_error)
}

/// Page text (labels, site names) printed to a terminal: control characters could carry escape
/// sequences, so they are dropped.
fn plain(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control())
        .collect()
}

/// A `secret_input` answer from the hidden prompt (`ask`) or all of stdin (`-`).
fn secret_answer<R: Read, E: Write>(
    ticket: &Ticket,
    ask: bool,
    totp: bool,
    stdin: &mut R,
    stderr: &mut E,
) -> Result<Value, String> {
    let input = if ask {
        let site = ticket
            .target
            .as_ref()
            .and_then(|target| target.origin.as_deref())
            .unwrap_or("this site");
        let what = if totp {
            "Authenticator setup key or otpauth:// link"
        } else if ticket.otp.is_some() {
            "Verification code"
        } else {
            "Secret"
        };
        prompt::read_hidden(&format!("{what} for {}", plain(site)), stdin, stderr)?
    } else {
        let mut input = String::new();
        stdin
            .read_to_string(&mut input)
            .map_err(|_| "could not read secret input from stdin".to_owned())?;
        if input.ends_with('\n') {
            input.pop();
            if input.ends_with('\r') {
                input.pop();
            }
        }
        input
    };
    Ok(if totp {
        serde_json::json!({ "totp": input })
    } else {
        Value::String(input)
    })
}

struct LoginFlags<'a> {
    save: bool,
    no_save: bool,
    alternative: Option<&'a str>,
    account: Option<&'a str>,
}

// Login identifiers are private too: canonical login cards mark every text field secret.
fn login_answer<R: Read, E: Write>(
    login: &TicketLogin,
    flags: LoginFlags<'_>,
    stdin: &mut R,
    stderr: &mut E,
) -> Result<Value, String> {
    if let Some(id) = flags.alternative {
        if !login
            .alternatives
            .iter()
            .any(|alternative| alternative.id == id)
        {
            return Err(format!(
                "no other sign-in {id} on this login ticket; see `ticket show`"
            ));
        }
        return Ok(json!({ "alternative": id }));
    }
    if let Some(id) = flags.account {
        if !login
            .accounts
            .iter()
            .flatten()
            .any(|account| account.item_id == id)
        {
            return Err(format!(
                "no saved account {id} on this login ticket; see `ticket show`"
            ));
        }
        return Ok(json!({ "account": id }));
    }
    if flags.save && !login.can_save {
        return Err("this site's login cannot be saved to the vault; omit --save".to_owned());
    }
    writeln!(
        stderr,
        "Log in to {} ({})",
        plain(&login.site_name),
        plain(&login.origin)
    )
    .map_err(output_error)?;
    let mut values = serde_json::Map::new();
    for field in &login.fields {
        let label = plain(&field.label);
        let value = if field.kind == LoginFieldKind::Checkbox {
            let answer = prompt::read_visible(&format!("{label} [y/N]"), stdin, stderr)?;
            Value::Bool(matches!(
                answer.trim().to_ascii_lowercase().as_str(),
                "y" | "yes"
            ))
        } else {
            let answer = prompt::read_hidden(&label, stdin, stderr)?;
            if field.required && answer.is_empty() {
                return Err(format!("{label} is required"));
            }
            Value::String(answer)
        };
        values.insert(field.id.clone(), value);
    }
    Ok(json!({ "values": values, "save": login.can_save && !flags.no_save }))
}

/// A `choice` answer: the options numbered on stderr, then the number (or the option id).
fn choice_answer<R: Read, E: Write>(
    choice: &TicketChoice,
    stdin: &mut R,
    stderr: &mut E,
) -> Result<Value, String> {
    writeln!(stderr, "{}", plain(&choice.question)).map_err(output_error)?;
    for (index, option) in choice.options.iter().enumerate() {
        match &option.description {
            Some(description) => writeln!(
                stderr,
                "  {}. {} ({})",
                index + 1,
                plain(&option.label),
                plain(description)
            ),
            None => writeln!(stderr, "  {}. {}", index + 1, plain(&option.label)),
        }
        .map_err(output_error)?;
    }
    let answer = prompt::read_visible("Option number", stdin, stderr)?;
    let answer = answer.trim();
    let picked = answer
        .parse::<usize>()
        .ok()
        .and_then(|number| number.checked_sub(1))
        .and_then(|index| choice.options.get(index))
        .or_else(|| choice.options.iter().find(|option| option.id == answer))
        .ok_or_else(|| format!("pick a number from 1 to {}", choice.options.len()))?;
    Ok(json!({ "option": picked.id }))
}

fn ticket_kind(kind: TicketKind) -> &'static str {
    match kind {
        TicketKind::Confirm => "confirm",
        TicketKind::SecretInput => "secret_input",
        TicketKind::DeviceApproval => "device_approval",
        TicketKind::Info => "info",
        TicketKind::Takeover => "takeover",
        TicketKind::Login => "login",
        TicketKind::Choice => "choice",
        TicketKind::Hire => "hire",
    }
}

fn print_waiting<W: Write>(
    json: bool,
    stdout: &mut W,
    id: &str,
    status: &str,
) -> Result<(), String> {
    if !json && status == "waiting_human" {
        writeln!(stdout, "Session {id} is waiting on tickets.").map_err(output_error)?;
    }
    Ok(())
}

/// The new-session request the global `--model`, `--mode`, `--workspace` and `--browser-space`
/// describe; workspace paths are made absolute.
fn create_request(args: &Args, title: Option<String>) -> Result<CreateSessionRequest, String> {
    Ok(CreateSessionRequest {
        agent_id: None,
        title,
        model: args.model.clone(),
        mode: args.mode.map(Into::into),
        workspace: args
            .workspace
            .as_ref()
            .map(|path| {
                path.canonicalize()
                    .map_err(|_| "could not canonicalize --workspace".to_owned())?
                    .into_os_string()
                    .into_string()
                    .map_err(|_| "--workspace must be valid UTF-8".to_owned())
            })
            .transpose()?,
        incognito: false,
        browser_space: args.browser_space.map(Into::into).unwrap_or_default(),
    })
}

async fn send_message(client: &Client, id: &str, text: &str) -> Result<(), String> {
    let _: Message = client
        .json(
            "POST",
            &client::session_path(id, "messages"),
            Some(&SubmitMessageRequest {
                message_id: None,
                text: text.into(),
                mode: MessageMode::Now,
                attachments: Vec::new(),
            }),
        )
        .await?;
    Ok(())
}

struct ReplayPoint {
    last_seen: u64,
    terminal_after: u64,
    // Metadata can follow a terminal marker, so idle replay ends at the observed log boundary.
    end_at: Option<(u64, &'static str)>,
}

async fn follow<W: Write, E: Write>(
    args: &Args,
    client: &Client,
    id: &str,
    replay: ReplayPoint,
    stdout: &mut W,
    stderr: &mut E,
    summary: &mut Summary,
) -> Result<(), String> {
    // Everything that reaches stderr goes through the guard so a redrawn status line is erased
    // before any other line, and on every exit path.
    let mut stderr_guard = ui::live::Stderr::new(stderr);
    let stderr = &mut stderr_guard;
    let mut live = ui::pretty()
        .filter(|_| !args.json)
        .map(|ui| ui::live::Live::new(*ui));
    if let Some(live) = live.as_mut() {
        live.begin(id, stderr)?;
    }
    let mut last_seq = replay.last_seen;
    let cursor = Cursor::new(client.endpoint_key(), id)?;
    summary.last_seq = Some(last_seq);
    let mut pending = BTreeSet::new();
    let mut seeded = Vec::new();
    let mut shown_tickets = BTreeSet::new();
    let web_origin = client.web_origin();
    let card_context = ui::cards::Context {
        session_id: id,
        web_origin: web_origin.as_deref(),
        client: Some(client),
    };
    if last_seq > 0 {
        let tickets: TicketList = client.get("/v1/tickets?status=pending").await?;
        for ticket in tickets
            .tickets
            .into_iter()
            .filter(|ticket| ticket.session_id.as_str() == id)
        {
            pending.insert(ticket.id.to_string());
            seeded.push(ticket);
        }
    }
    let mut response = client.events(id, last_seq).await?;
    let mut decoder = Decoder::default();
    let mut saw_delta = false;
    let mut ends_with_newline = true;
    while let Some(frame) = ui::live::next_frame(&mut response, &mut live, stderr).await {
        let frame = frame.map_err(|_| "event stream connection failed".to_owned())?;
        let Ok(bytes) = frame.into_data() else {
            continue;
        };
        for record in decoder.push(&bytes)? {
            let event: Event = serde_json::from_slice(&record.data)
                .map_err(|_| "invalid event JSON in SSE stream".to_owned())?;
            if event.session_id.as_str() != id || event.seq == 0 || record.id != Some(event.seq) {
                return Err("SSE event session/sequence does not match its envelope or id".into());
            }
            if event.seq <= last_seq {
                continue;
            }
            if args.json {
                emit(stdout, &event)?;
            } else if let Some(live) = live.as_mut() {
                if let Some(card) = ui::cards::event_card(&event.kind) {
                    live.card(card, &card_context, stdout, stderr)?;
                }
                live.event(&event, stdout, stderr)?;
            } else if let Some(card) = ui::cards::event_card(&event.kind) {
                if !ends_with_newline {
                    writeln!(stdout).map_err(output_error)?;
                }
                ends_with_newline = true;
                let rendered = ui::cards::render(&ui::cards::plain_ui(), card, &card_context);
                if card.ticket_id.is_some() {
                    write!(stderr, "{rendered}").map_err(output_error)?;
                } else {
                    write!(stdout, "{rendered}").map_err(output_error)?;
                }
            } else {
                match &event.kind {
                    EventKind::TurnStarted {} => {
                        saw_delta = false;
                        // Text before a tool call and the answer after it are separate turns.
                        if !ends_with_newline {
                            writeln!(stdout).map_err(output_error)?;
                            ends_with_newline = true;
                        }
                    }
                    EventKind::AssistantDelta { text } => {
                        write!(stdout, "{text}").map_err(output_error)?;
                        saw_delta = true;
                        if !text.is_empty() {
                            ends_with_newline = text.ends_with('\n');
                        }
                    }
                    EventKind::AssistantFinal { text, .. } if !saw_delta => {
                        write!(stdout, "{text}").map_err(output_error)?;
                        if !text.is_empty() {
                            ends_with_newline = text.ends_with('\n');
                        }
                    }
                    // Its own notice with the resolve command follows below.
                    EventKind::TicketOpened { .. } => {}
                    _ => {
                        if let Some(line) = agent::progress_line(&event) {
                            writeln!(stderr, "{line}").map_err(output_error)?;
                            stderr.flush().map_err(output_error)?;
                        }
                    }
                }
            }
            stdout.flush().map_err(output_error)?;
            last_seq = event.seq;
            summary.last_seq = Some(last_seq);
            cursor.save(last_seq)?;
            match &event.kind {
                EventKind::TicketOpened { ticket } => {
                    pending.insert(ticket.id.to_string());
                    shown_tickets.insert(ticket.id.to_string());
                    // On a terminal the notice shares the screen with a half-written reply line.
                    if !args.json && !ends_with_newline {
                        writeln!(stdout).map_err(output_error)?;
                        ends_with_newline = true;
                        stdout.flush().map_err(output_error)?;
                    }
                    if ticket.card.is_some() {
                        // The canonical card was already emitted or rendered above.
                    } else if let Some(live) = live.as_mut() {
                        live.ticket(
                            ticket,
                            &|action, value| {
                                client.ticket_command(ticket.id.as_str(), action, value)
                            },
                            resolve_hint(ticket),
                            stderr,
                        )?;
                    } else if let Some(approval) = &ticket.approval {
                        let number = approval
                            .number
                            .as_deref()
                            .map(|number| format!(" Tap {number} on your phone."))
                            .unwrap_or_default();
                        writeln!(
                            stderr,
                            "Ticket {} (approve on your phone): {}.{number}\nWaiting; openside continues by itself once the site sees the approval. To cancel: {}",
                            ticket.id, ticket.summary,
                            client.ticket_command(ticket.id.as_str(), "reject", None)
                        ).map_err(output_error)?;
                        stderr.flush().map_err(output_error)?;
                    } else if ticket.kind == TicketKind::Confirm {
                        writeln!(
                            stderr,
                            "Ticket {} (confirm): {}\nAsk a person to approve on another paired web or phone device; this client cannot approve its own work. Waiting for the ticket.",
                            ticket.id, ticket.summary,
                        ).map_err(output_error)?;
                        stderr.flush().map_err(output_error)?;
                    } else {
                        let (action, value) = resolve_hint(ticket)?;
                        writeln!(
                        stderr,
                        "Ticket {} ({}): {}\nResolve in another process: {}\nFor HTTP connections, supply OPENSIDE_TOKEN or --token to that process.",
                        ticket.id, ticket_kind(ticket.kind), ticket.summary,
                        client.ticket_command(ticket.id.as_str(), action, value.as_deref())
                    ).map_err(output_error)?;
                        if ticket.kind == TicketKind::SecretInput {
                            writeln!(
                                stderr,
                                "Pipe the secret to stdin; do not put it in argv or shell history."
                            )
                            .map_err(output_error)?;
                        }
                        stderr.flush().map_err(output_error)?;
                    }
                }
                EventKind::TicketResolved { ticket_id, .. }
                | EventKind::TicketRejected { ticket_id, .. }
                | EventKind::TicketExpired { ticket_id, .. } => {
                    pending.remove(ticket_id.as_str());
                }
                EventKind::MessageCompleted {
                    outcome: MessageOutcome::Deferred,
                    ..
                } => {
                    summary.status = "waiting_human";
                }
                _ => {}
            }
            if let EventKind::RuntimeError { code, message, .. } = &event.kind {
                if code == "sandbox_unavailable" {
                    writeln!(stderr, "openside: warning: {message}").map_err(output_error)?;
                } else {
                    return Err(format!("runtime error {code}: {message}"));
                }
            }
            let terminal = if let Some((seq, status)) = replay.end_at {
                (event.seq >= seq).then_some(status)
            } else {
                match event.kind {
                    EventKind::SessionSettled {} => Some(if pending.is_empty() {
                        "settled"
                    } else {
                        "waiting_human"
                    }),
                    EventKind::SessionStopped {} => Some("stopped"),
                    _ => None,
                }
            };
            if let Some(status) = terminal.filter(|_| event.seq > replay.terminal_after) {
                summary.status = status;
                if !args.json && !ends_with_newline {
                    writeln!(stdout).map_err(output_error)?;
                }
                if !args.json {
                    for ticket in &seeded {
                        if pending.contains(ticket.id.as_str())
                            && !shown_tickets.contains(ticket.id.as_str())
                        {
                            if let Some(card) = &ticket.card {
                                if let Some(live) = live.as_mut() {
                                    live.card(card, &card_context, stdout, stderr)?;
                                } else {
                                    write!(
                                        stderr,
                                        "{}",
                                        ui::cards::render(
                                            &ui::cards::plain_ui(),
                                            card,
                                            &card_context
                                        )
                                    )
                                    .map_err(output_error)?;
                                }
                            }
                        }
                    }
                }
                if let Some(live) = live.as_mut() {
                    live.finish(status, id, stdout, stderr)?;
                } else {
                    print_waiting(args.json, stdout, id, status)?;
                }
                return Ok(());
            }
        }
    }
    Err(
        "event stream ended before the session settled or stopped; use session resume to continue"
            .into(),
    )
}

/// How a person answers a ticket from another process: the `ticket` verb and its value.
fn resolve_hint(ticket: &Ticket) -> Result<(&'static str, Option<String>), String> {
    if ticket.form.is_some() {
        return Ok(("resolve", None));
    }
    Ok(match ticket.kind {
        TicketKind::Confirm => ("resolve", Some("true".to_owned())),
        TicketKind::Hire => ("resolve", Some("\"hire\"".to_owned())),
        TicketKind::SecretInput => ("resolve", Some("-".to_owned())),
        // Both ask at prompts: one per login field, or the numbered options.
        TicketKind::Login | TicketKind::Choice => ("resolve", None),
        TicketKind::DeviceApproval | TicketKind::Takeover => ("done", None),
        TicketKind::Info => (
            "resolve",
            Some(match ticket.options.first() {
                Some(option) => serde_json::to_string(&option.value)
                    .map_err(|_| "could not encode ticket option".to_owned())?,
                None => "your answer".to_owned(),
            }),
        ),
    })
}

fn emit<W: Write, T: Serialize>(stdout: &mut W, value: &T) -> Result<(), String> {
    serde_json::to_writer(&mut *stdout, value)
        .map_err(|_| "could not write JSON output".to_owned())?;
    writeln!(stdout).map_err(output_error)
}

fn output_error(_: std::io::Error) -> String {
    "could not write output".into()
}

fn status_name(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => "idle",
        SessionStatus::Running => "running",
        SessionStatus::Stopped => "stopped",
        SessionStatus::WaitingHuman => "waiting_human",
    }
}

#[cfg(all(test, unix))]
mod tests;
