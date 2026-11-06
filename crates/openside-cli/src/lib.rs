//! Non-interactive client of the openside public API v1.
//!
//! `openside [options] [prompt...]` (the same as `openside exec`) creates a session, sends the
//! prompt and follows its SSE stream. The commands mirror a browser-agent CLI: `account`
//! (team agents as `u0`, `u1`, ...), `login`/`logout` (pairing with a remote daemon), `host`
//! (`local` or a saved remote, selected with `--host`), `memory`, `skills`, `exec`, `repl`,
//! `mcp`, `guide`, `update`, `session` and `settings` (`cli.toml` next to the daemon's
//! configuration). openside's own administration follows: `models`, `key`, `channels`,
//! `team`, `ticket`, `routine`, `token`, `vault`, `connections`, `sync`, `policy`, `browser`,
//! `import`, `doctor`, `pair`, `remote`, `device` and `service`. `docs/reference/cli.md` is
//! the generated reference.
//!
//! Session commands are `session list` (saved and unsaved sessions), `session resume <id>
//! [prompt...] [--last-event-id <seq>]`, `session stop|steer|queue|archive|delete`,
//! `session new` and `session tasks`. A finished `exec` or `repl` session is archived unless
//! the daemon's `save_sessions` preference is on (`settings save-sessions true`). Tickets support `ticket list [--all]`, `show <id>`,
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
//! Apart from `login`, a `-` prompt, `repl`, `vault init|unlock|add`, and `key set`, these are
//! the only stdin-reading commands, and they never print the supplied value.
//! `-m/--model`, `-p/--provider`, `--permission ask|guard|full-access|read-only`, `--account`,
//! `--profile shared|isolated|incognito` and `--workspace PATH` select session creation
//! settings; workspace paths are canonicalized to absolute paths. `--effort` and a non-default
//! `--speed` are accepted by the parser but refused until the daemon supports them.
//! `memory model fetch <owner/name>` runs without a daemon and is the only command that
//! reaches the internet: it downloads a static embedding model over HTTPS (redirects
//! allowed) into `memory.embedding.model_dir`.
//! `mcp` serves the public API as an MCP server on stdin/stdout (`docs/specs/mcp.md`); it uses
//! the same connection and token as every other command, so the token's scopes bound what an
//! MCP client can do. `--json` has no effect: stdout carries only the MCP protocol.
//! `remote setup|status` configures and inspects this daemon's remote transports
//! (`docs/REMOTE.md`).
//!
//! The default connection is the daemon socket resolved from the shared configuration
//! (`daemon.socket`/`OPENSIDE_SOCKET`, else `<runtime dir>/opensided.sock`), or the host
//! chosen with `openside host use`. `--host NAME`/`OPENSIDE_HOST` selects a remote saved by
//! `openside login` (or `local`); `--socket` overrides the socket. `--url` accepts only HTTP
//! loopback addresses and needs `--token` or `OPENSIDE_TOKEN`; redirects, proxies, and DNS
//! are never used.
//! `--json` emits raw API objects/events and a final `{"type":"summary", ...}`
//! object, one per line. `openside "task"` does this by default when stdout is not a
//! terminal (`openside "x" | cat`); other commands need `--json` explicitly. Ticket
//! instructions go to stderr, including in JSON mode.
//! No command requires a TTY. `exec` (and a bare prompt, and `session resume ID PROMPT`) keeps
//! following a session that settles with pending tickets: it prints each ticket and how to
//! resolve it, then exits only once the run settles with no ticket left or is stopped.
//! `session resume ID` without a prompt exits successfully with status `waiting_human`
//! instead. A bare prompt that is a single http(s) URL opens that page in a new tab of the
//! openside browser (`browser_open_tab`) instead of starting a task.
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

mod account;
mod agent;
mod browser;
mod channels;
mod client;
mod config;
mod connections;
mod cursor;
mod guide;
mod host;
mod import;
mod mcp;
mod memory;
mod model_accounts;
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
mod settings;
mod skills;
mod sse;
mod sync;
mod team;
mod token;
mod tool_call;
mod ui;
mod update;
mod vault;
mod wait;

use std::{
    collections::BTreeSet,
    ffi::OsString,
    io::{Read, Write},
    path::PathBuf,
};

use clap::{ArgAction, Args as ClapArgs, Parser, Subcommand, ValueEnum};
use openside_config::doctor::{self, DoctorOptions, Status};
use openside_config::StoredVault;
use openside_proto::{
    BrowserSpace, CreateSessionRequest, Event, EventKind, LoginFieldKind, Message, MessageMode,
    MessageOutcome, PolicyMode, RejectTicketRequest, ResolveTicketRequest, Session, SessionList,
    SessionProfile, SessionSpeed, SessionStatus, SubmitMessageRequest, ThinkingEffort, Ticket,
    TicketChoice, TicketKind, TicketList, TicketLogin,
};
use openside_vault::StoredFormat;
use serde::Serialize;
use serde_json::{json, Value};

use client::Client;
use cursor::Cursor;
use openside_proto::CliProfile;
use settings::{AccountRef, AccountSelector, ProfileExt};
use sse::Decoder;

const ROOT_AFTER_HELP: &str = "\
Model options:
  Without -m, sessions use the default model from `openside models`.
  Pass --model provider:model (or provider/model), or --model NAME --provider PROVIDER. A provider in --model wins over --provider.
  --permission ask and --permission guard are the same (Guard, the default); read-only never acts on pages.

A prompt that is only an http(s) URL opens that page in a new openside tab instead of starting a task.
When a step needs a person, the ticket and how to answer it are printed and the run waits until it is answered.

Examples:
  openside \"Find the unread message from Alex in my inbox\"
  openside https://example.com
  openside -m anthropic:claude-sonnet-4-5 \"Draft a reply to the latest support email\"
  openside --permission full-access \"Fill in the expense form from the receipts tab\"
  openside --account u1 \"Check today's failed payments\"
  openside --host laptop \"Find a hotel near the venue for next weekend\"
  openside guide";

const TASK_AFTER_HELP: &str = "\
Model options:
  Without -m, sessions use the default model from `openside models`.
  Pass --model provider:model (or provider/model), or --model NAME --provider PROVIDER. A provider in --model wins over --provider.
  --permission ask and --permission guard are the same (Guard, the default); read-only never acts on pages.

When a step needs a person, the ticket and how to answer it are printed and the run waits until it is answered.

Examples:
  openside exec \"Find the unread message from Alex in my inbox\"
  openside exec --profile isolated \"Check the pricing page while signed out\"
  openside exec --account u1:isolated \"Compare the two plans\"";

const REPL_AFTER_HELP: &str = "\
Notes:
  Playwright-style JavaScript, 120s per call. page starts unset; call openTab(url) or getTabs() first, or pass --tab ID.
  Variables and attached tabs carry over between calls in the same session (--session ID, or one stdin line per call).
  The last expression's value is printed, then console.log output. Read `openside guide repl` first.

Examples:
  openside repl \"await openTab('https://example.com'); await page.title()\"
  openside repl --session <id> \"await page.snapshot()\"
  openside repl --session <id> -
  openside repl --host laptop --profile isolated";

const MCP_AFTER_HELP: &str = "\
Tools: exec, repl, browser_profiles, memory_search, then session, ticket and agent_* tools and the daemon's browser tools.

Examples:
  openside mcp
  openside mcp --account u1
  openside mcp --host laptop --profile isolated";

const SESSION_AFTER_HELP: &str = "\
steer interrupts the current step. queue waits and runs next.
resume with a prompt runs it like exec, waiting through tickets; without one it follows the session until it settles or waits on a ticket.

Examples:
  openside session list
  openside session resume <id> \"Check the third result\"
  openside session stop <id>
  openside session steer <id> \"Stop searching and open the inbox\"
  openside session queue <id> \"Then export the list to CSV\"";

const HOST_AFTER_HELP: &str = "\
Hosts are `local` (this computer's daemon) and the remote daemons saved by `openside login`.

Examples:
  openside host list
  openside host profiles --host laptop
  openside host use laptop
  openside host status";

const ACCOUNT_AFTER_HELP: &str = "\
Accounts are the daemon's team agents: u0 is the chief, u1, u2, ... the teammates (`openside team list`).

Examples:
  openside account list
  openside account status u1
  openside account use u1";

#[derive(Parser)]
#[command(
    name = "openside",
    version,
    about = "Run a browsing task in openside from the terminal.",
    override_usage = "openside [options] [prompt...]\n       openside [options] <command> [args...]",
    disable_version_flag = true,
    arg_required_else_help = true,
    after_help = ROOT_AFTER_HELP,
    color = clap::ColorChoice::Never,
    styles = ui::clap_styles()
)]
struct Args {
    #[arg(short = 'V', long, action = ArgAction::Version, help = "Print version")]
    version: Option<bool>,
    #[arg(
        long,
        value_name = "id",
        help = "Account selector: u0 (the chief), u1, ... or u1:<profile-id>"
    )]
    account: Option<String>,
    #[arg(
        long,
        global = true,
        env = "OPENSIDE_HOST",
        conflicts_with_all = ["socket", "url"],
        value_name = "host",
        help = "Run on a session host: 'local', or a host saved by `openside login` (name or device id)"
    )]
    host: Option<String>,
    #[arg(
        long,
        value_name = "id",
        help = "Browser profile on the selected host (see `openside host profiles`)"
    )]
    profile: Option<String>,
    #[command(flatten)]
    task: TaskOptions,
    #[arg(long, help = "Same as `openside update`")]
    update: bool,
    #[command(flatten)]
    connection: Connection,
    #[arg(
        value_name = "prompt",
        help = "Task prompt for the browser agent, or a URL to open; `-` reads it from stdin"
    )]
    prompt: Vec<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

/// How the CLI reaches the daemon besides `--host`.
#[derive(ClapArgs)]
#[command(next_help_heading = "Connection options")]
struct Connection {
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
}

/// Model and permission settings of a new session.
#[derive(ClapArgs, Default)]
struct TaskOptions {
    #[arg(
        short,
        long,
        value_name = "model",
        help = "Model id, or provider:model (e.g. anthropic:claude-sonnet-4-5)"
    )]
    model: Option<String>,
    #[arg(short, long, value_name = "provider", help = "Model provider name")]
    provider: Option<String>,
    #[arg(
        short,
        long,
        value_enum,
        value_name = "speed",
        help = "Model speed. With an agent-client token, fast needs the daemon host's local socket."
    )]
    speed: Option<Speed>,
    #[arg(
        long,
        value_enum,
        value_name = "effort",
        help = "Thinking effort; ultrabrowse runs the session in openside's research profile. With an agent-client token, xhigh, max and ultrabrowse need the daemon host's local socket."
    )]
    effort: Option<Effort>,
    #[arg(
        long,
        value_enum,
        value_name = "mode",
        help = "Session permission: ask/guard (ask a person before sensitive steps), full-access, or read-only"
    )]
    permission: Option<Permission>,
    #[arg(long, value_name = "PATH", help = "Workspace folder for the session")]
    workspace: Option<PathBuf>,
}

/// Which account and browser profile a new session uses.
#[derive(ClapArgs, Default)]
struct Target {
    #[arg(
        long,
        value_name = "id",
        help = "Account selector: u0 (the chief), u1, ... or u1:<profile-id>"
    )]
    account: Option<String>,
    #[arg(
        long,
        value_name = "id",
        help = "Browser profile on the selected host (see `openside host profiles`)"
    )]
    profile: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Speed {
    Default,
    Fast,
}

#[derive(Clone, Copy, ValueEnum)]
enum Effort {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Ultrabrowse,
}

#[derive(Clone, Copy, ValueEnum)]
enum Permission {
    Ask,
    Guard,
    FullAccess,
    ReadOnly,
}

impl From<Permission> for PolicyMode {
    fn from(permission: Permission) -> Self {
        match permission {
            Permission::Ask | Permission::Guard => Self::Guard,
            Permission::FullAccess => Self::Full,
            Permission::ReadOnly => Self::ReadOnly,
        }
    }
}

/// `--mode` of `routine add` and `policy test`: the daemon's policy modes by name.
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

#[derive(Subcommand)]
enum Command {
    /// List and switch openside CLI accounts.
    #[command(after_help = ACCOUNT_AFTER_HELP)]
    Account {
        #[command(subcommand)]
        command: account::AccountCommand,
    },
    /// Sign the CLI in to another computer's openside with a pairing link from `openside pair`
    Login(remotes::LoginArgs),
    /// Sign the CLI out of a host and revoke its device token
    Logout,
    /// Inspect and select session hosts (this computer or a paired remote)
    #[command(after_help = HOST_AFTER_HELP)]
    Host {
        #[command(subcommand)]
        command: host::HostCommand,
    },
    /// Search and read openside's memory
    Memory {
        #[command(subcommand)]
        command: memory::MemoryCommand,
    },
    /// Browse and install skills, including the openside-browser skill for coding agents
    Skills {
        #[command(subcommand)]
        command: skills::SkillsCommand,
    },
    /// Run a browser agent session.
    #[command(after_help = TASK_AFTER_HELP)]
    Exec {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        task: TaskOptions,
        #[arg(
            value_name = "prompt",
            required = true,
            help = "Task prompt for the browser agent; `-` reads it from stdin"
        )]
        prompt: Vec<String>,
    },
    /// Run Playwright-style JavaScript in an openside agent tab.
    #[command(after_help = REPL_AFTER_HELP)]
    Repl {
        #[command(flatten)]
        target: Target,
        /// Session whose agent tab runs the code; default: a new session for this command.
        #[arg(long, value_name = "id")]
        session: Option<String>,
        /// Agent tab to drive; optional when the session owns at most one tab.
        #[arg(long, value_name = "id")]
        tab: Option<String>,
        /// JavaScript to run (`-` reads it from stdin); without it, every stdin line is a snippet
        #[arg(value_name = "code")]
        code: Vec<String>,
    },
    /// Start the MCP server over stdio.
    #[command(after_help = MCP_AFTER_HELP)]
    Mcp {
        #[command(flatten)]
        target: Target,
    },
    /// Show how to use openside for browser tasks.
    Guide {
        #[arg(value_enum, value_name = "topic")]
        topic: Option<guide::Topic>,
    },
    /// Show the installed version and how to update it.
    Update {
        /// Do not ask for confirmation.
        #[arg(short, long)]
        yes: bool,
    },
    /// Inspect and control browser sessions.
    #[command(after_help = SESSION_AFTER_HELP)]
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Configure openside CLI settings.
    #[command(after_help = settings::SETTINGS_AFTER_HELP)]
    Settings {
        #[command(subcommand)]
        command: settings::SettingsCommand,
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
    /// List and answer tickets: the steps that need a person.
    Ticket {
        #[command(subcommand)]
        command: TicketCommand,
    },
    /// Manage scheduled routines.
    Routine {
        #[command(subcommand)]
        command: routines::RoutineCommand,
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
    /// Read or change any config.toml key through the daemon (validated; local-only keys need
    /// this computer).
    #[command(after_help = config::CONFIG_AFTER_HELP)]
    Config {
        #[command(subcommand)]
        command: config::ConfigCommand,
    },
    /// Call the session's browser and human-step tools directly, like the in-app agent.
    Browser {
        #[command(subcommand)]
        command: browser::BrowserCommand,
    },
    /// Import history, bookmarks and cookies from a Chrome, Chromium or Firefox profile.
    Import(import::ImportArgs),
    /// Diagnose the local installation without a daemon.
    Doctor {
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
    },
    /// Pair a phone or browser: print a one-time QR code valid for 3 minutes (local socket).
    Pair(pair::PairArgs),
    /// This daemon's remote access (Tailscale, Cloudflare Tunnel or a relay).
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
    /// List sessions, including unsaved sessions.
    List,
    /// Continue an existing session.
    Resume {
        #[arg(value_name = "id")]
        id: String,
        #[arg(value_name = "prompt")]
        prompt: Vec<String>,
        #[arg(
            long,
            value_name = "SEQ",
            help = "Override the persisted last-seen SSE sequence"
        )]
        last_event_id: Option<u64>,
    },
    /// Stop a running session, or one of its Tasks with --task.
    Stop {
        #[arg(value_name = "id")]
        id: String,
        /// Cancel only this Task; the agent and the other Tasks keep running.
        #[arg(long, value_name = "TASK_ID")]
        task: Option<String>,
    },
    /// Interrupt a session with a new prompt.
    Steer {
        #[arg(value_name = "id")]
        id: String,
        #[arg(value_name = "prompt", required = true)]
        prompt: Vec<String>,
        /// Steer this Task instead of the session's agent (ids from `openside session tasks`).
        #[arg(long, value_name = "TASK_ID")]
        task: Option<String>,
    },
    /// Queue a follow-up prompt after the current step.
    Queue {
        #[arg(value_name = "id")]
        id: String,
        #[arg(value_name = "prompt", required = true)]
        prompt: Vec<String>,
    },
    /// Archive a session.
    Archive {
        #[arg(value_name = "id")]
        id: String,
    },
    /// Delete a session.
    Delete {
        #[arg(value_name = "id")]
        id: String,
    },
    /// Create an empty session to drive with `browser` or `repl`; prints its id.
    New {
        #[arg(long)]
        title: Option<String>,
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        task: TaskOptions,
    },
    /// List a session's Tasks and subagents with their status and results.
    Tasks {
        #[arg(value_name = "id")]
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

#[derive(Default)]
struct Summary {
    session_id: Option<String>,
    last_seq: Option<u64>,
    status: &'static str,
    failed: bool,
    /// Ctrl-C detached from a followed run, which keeps going on the daemon: exit 130.
    interrupted: bool,
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
    // A code pasted as an extra argument must not come back in clap's error text.
    let oauth_input = argv.windows(3).any(|words| {
        words[0] == "accounts" && (words[1] == "add" || words[1] == "login") && words[2] == "oauth"
    });
    let legacy_remote = uses_legacy_remote_flag(&argv);
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
    let mut args = match parsed {
        Ok(args) => args,
        Err(error) => {
            let usage = error.use_stderr();
            let result = if usage {
                let shown = if key_input {
                    writeln!(stderr, "openside: invalid key command; use `openside key set <provider>` and supply the key on stdin")
                } else if channel_input {
                    writeln!(stderr, "openside: invalid channel command; use `openside channels add <kind> <id> --label <label>` and supply credentials on hidden stdin")
                } else if oauth_input {
                    writeln!(stderr, "openside: invalid OAuth command; use `openside models accounts add oauth --provider <provider> --label <label>` or `openside models accounts login oauth <provider>/<account_id>`; paste codes only at the hidden prompt")
                } else if let Some(ui) = ui::pretty() {
                    write!(stderr, "{}", ui::render_help_error(&error, ui))
                } else {
                    write!(stderr, "{error}")
                };
                if legacy_remote && error.kind() == clap::error::ErrorKind::UnknownArgument {
                    shown.and_then(|()| writeln!(stderr, "{LEGACY_REMOTE_HINT}"))
                } else {
                    shown
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
    let is_task = matches!(args.command, None | Some(Command::Exec { .. }));
    args.connection.json |=
        json_when_task && is_task && !args.update && args.connection.color != ui::ColorWhen::Always;
    let json = args.connection.json;
    if args.update || matches!(args.command, Some(Command::Update { .. })) {
        return finish(
            update::print(json, stdout),
            &args,
            Summary::default(),
            stdout,
            stderr,
        );
    }
    if args.host.is_none()
        && std::env::var_os("OPENSIDE_REMOTE").is_some_and(|value| !value.is_empty())
    {
        return usage_error(
            "OPENSIDE_REMOTE is no longer read; set OPENSIDE_HOST to the host name instead",
            json,
            stdout,
            stderr,
        );
    }
    if args.command.is_none() && args.prompt.is_empty() {
        return usage_error(
            "provide a prompt or a command (see `openside --help`)",
            json,
            stdout,
            stderr,
        );
    }
    // Subcommand names win over the bare prompt, so root-only task options are checked here:
    // clap's args_conflicts_with_subcommands would also count the global connection flags.
    if args.command.is_some() && root_task_flags(&args) {
        return usage_error(
            "--account, --profile, -m, -p, -s, --effort, --permission and --workspace before a \
             command apply only to a bare prompt; put them after `exec`",
            json,
            stdout,
            stderr,
        );
    }
    if let Err(error) = task_options(&args).map_or(Ok(()), check_task_options) {
        return usage_error(&error, json, stdout, stderr);
    }
    let saved = match settings::load() {
        Ok(saved) => saved,
        Err(error) => return usage_error(&error, json, stdout, stderr),
    };
    let (account, profile) = target_of(&args);
    let chosen = match choose(account, profile, &saved) {
        Ok(chosen) => chosen,
        Err(error) => return usage_error(&error, json, stdout, stderr),
    };
    let host = match selected_host(&args, &saved) {
        Ok(host) => host,
        Err(error) => return usage_error(&error, json, stdout, stderr),
    };
    let mut summary = Summary::default();
    let result = match &args.command {
        Some(Command::Doctor { config }) => {
            run_doctor(config.as_deref(), json, stdout, &mut summary).await
        }
        Some(Command::Import(import_args)) => import::run(import_args, json, stdout).await,
        Some(Command::Service { command }) => service::execute(command, stdout),
        Some(Command::Login(login)) => remotes::login(login, json, stdin, stdout, stderr).await,
        Some(Command::Logout) => host::logout(host.as_deref(), json, stdout, stderr).await,
        Some(Command::Host { command }) if !matches!(command, host::HostCommand::Status) => {
            host::local(command, host.as_deref(), chosen.profile, json, stdout)
        }
        Some(Command::Guide { topic }) => guide::print(*topic, stdout),
        Some(Command::Settings {
            command: command @ settings::SettingsCommand::SetDefaultProfile { id },
        }) if command.is_local() => settings::set_default_profile(id, None, stdout),
        Some(Command::Remote {
            command: remote::RemoteCommand::Setup(setup),
        }) => remote::setup(setup, json, stdout).await,
        Some(Command::Memory {
            command: memory::MemoryCommand::Model { command },
        }) => memory::model(command, json, stdout).await,
        Some(Command::Memory {
            command: memory::MemoryCommand::Path,
        }) => match host {
            None => memory::path(json, stdout),
            Some(_) => Err(
                "memory path shows this computer's database; it does not apply with --host".into(),
            ),
        },
        Some(Command::Skills { command }) if command.is_local() => {
            skills::local(command, json, stdout)
        }
        _ => {
            let client = match Client::from_flags(
                args.connection.socket.as_deref(),
                args.connection.url.as_deref(),
                args.connection.token.as_deref(),
                host.as_deref(),
            )
            .await
            {
                Ok(client) => client,
                Err(error) => return usage_error(&error, json, stdout, stderr),
            };
            if matches!(args.command, Some(Command::Mcp { .. })) {
                let account = match agent_id(&client, chosen.account.as_ref()).await {
                    Ok(account) => account,
                    Err(error) => {
                        let _ = writeln!(stderr, "openside: {error}");
                        return 1;
                    }
                };
                let defaults = mcp::Defaults {
                    account: account.as_deref(),
                    profile: chosen.profile,
                };
                return mcp::run(client, args.connection.token.clone(), defaults, stderr).await;
            }
            let context = Context {
                client: &client,
                host: host.as_deref(),
                chosen: &chosen,
            };
            execute(&args, &context, stdin, stdout, stderr, &mut summary).await
        }
    };
    finish(result, &args, summary, stdout, stderr)
}

/// Report a usage or connection-configuration error: exit code 2.
fn usage_error<W: Write, E: Write>(
    message: &str,
    json: bool,
    stdout: &mut W,
    stderr: &mut E,
) -> u8 {
    let _ = writeln!(stderr, "openside: {message}");
    if json {
        let _ = emit(
            stdout,
            &json!({"type":"summary","status":"error","exit_code":2}),
        );
    }
    2
}

fn finish<W: Write, E: Write>(
    result: Result<(), String>,
    args: &Args,
    mut summary: Summary,
    stdout: &mut W,
    stderr: &mut E,
) -> u8 {
    let exit_code = if let Err(error) = result {
        let detail = match args
            .connection
            .token
            .as_deref()
            .filter(|token| !token.is_empty())
        {
            Some(token) => error.replace(token, "[REDACTED]"),
            None => error,
        };
        let _ = writeln!(stderr, "openside: {detail}");
        summary.status = "error";
        1
    } else if summary.interrupted {
        summary.status = "interrupted";
        130
    } else {
        // Commands that run without a daemon never set a status of their own.
        if summary.status.is_empty() {
            summary.status = "ok";
        }
        u8::from(summary.failed)
    };
    if args.connection.json
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

const LEGACY_REMOTE_HINT: &str =
    "hint: `--remote` is now `--host` (and OPENSIDE_REMOTE is now OPENSIDE_HOST)";

/// Whether the removed global `--remote` flag appears before any `--` separator.
fn uses_legacy_remote_flag(argv: &[OsString]) -> bool {
    argv.iter()
        .skip(1)
        .take_while(|arg| *arg != "--")
        .any(|arg| {
            arg == "--remote" || arg.to_str().is_some_and(|arg| arg.starts_with("--remote="))
        })
}

/// What every command that talks to a daemon shares.
struct Context<'a> {
    client: &'a Client,
    /// The remote host, or `None` for this computer's daemon.
    host: Option<&'a str>,
    chosen: &'a Chosen,
}

async fn execute<R: Read, W: Write, E: Write>(
    args: &Args,
    context: &Context<'_>,
    stdin: &mut R,
    stdout: &mut W,
    stderr: &mut E,
    summary: &mut Summary,
) -> Result<(), String> {
    let client = context.client;
    summary.status = "ok";
    match &args.command {
        None => {
            if let Some(url) = bare_url(&args.prompt) {
                return open_url(args, context, url, stdout, stderr, summary).await;
            }
            let run = Exec {
                task: &args.task,
                prompt: &args.prompt,
            };
            exec(args, context, run, stdin, stdout, stderr, summary).await
        }
        Some(Command::Exec { task, prompt, .. }) => {
            exec(
                args,
                context,
                Exec { task, prompt },
                stdin,
                stdout,
                stderr,
                summary,
            )
            .await
        }
        Some(Command::Account { command }) => {
            account::execute(command, client, args.connection.json, stdout).await
        }
        Some(Command::Host { .. }) => {
            host::status(context.host, client, args.connection.json, stdout).await
        }
        Some(Command::Settings { command }) => {
            let account = match command.account()? {
                Some(selector) => Some(account::resolve(client, &selector).await?),
                None => None,
            };
            settings::execute(command, client, account, stdout, stderr).await
        }
        Some(Command::Models { command }) => {
            models::run(
                command.as_ref(),
                client,
                args.connection.json,
                stdout,
                stderr,
                &mut summary.interrupted,
            )
            .await
        }
        Some(Command::Key { command }) => {
            models::key(command, client, args.connection.json, stdin, stdout, stderr).await
        }
        Some(Command::Channels { command }) => {
            channels::run(command, client, args.connection.json, stdin, stdout, stderr).await
        }
        Some(Command::Team { command }) => {
            team::execute(command.as_ref(), args.connection.json, client, stdout).await
        }
        Some(Command::Browser { command }) => {
            browser::execute(
                command,
                args.connection.json,
                client,
                stdout,
                stderr,
                summary,
            )
            .await
        }
        Some(Command::Session { command }) => match command {
            SessionCommand::List => {
                let mut list: SessionList = client.get("/v1/sessions").await?;
                let unsaved: SessionList = client.get("/v1/sessions?archived=true").await?;
                list.sessions.extend(unsaved.sessions);
                if args.connection.json {
                    emit(stdout, &list)?;
                } else if let Some(ui) = ui::pretty() {
                    write!(stdout, "{}", ui::sessions(ui, &list)).map_err(output_error)?;
                } else {
                    for session in &list.sessions {
                        writeln!(
                            stdout,
                            "{}\t{}\t{}\t{}",
                            session.id,
                            status_name(session.status),
                            if session.archived { "unsaved" } else { "saved" },
                            session.title
                        )
                        .map_err(output_error)?;
                    }
                }
                Ok(())
            }
            SessionCommand::New { title, task, .. } => {
                let create = create_request(context, task, title.clone()).await?;
                let session: Session = client.json("POST", "/v1/sessions", Some(&create)).await?;
                summary.session_id = Some(session.id.to_string());
                summary.last_seq = Some(session.last_seq);
                if args.connection.json {
                    emit(stdout, &session)
                } else {
                    writeln!(stdout, "{}", session.id).map_err(output_error)
                }
            }
            SessionCommand::Stop {
                id,
                task: Some(task),
            } => {
                agent::cancel(
                    client,
                    id,
                    Some(task),
                    args.connection.json,
                    stdout,
                    summary,
                )
                .await
            }
            SessionCommand::Stop { id, task: None } => {
                let session: Session = client
                    .json::<_, Value>("POST", &client::session_path(id, "stop"), None)
                    .await?;
                summary.session_id = Some(session.id.to_string());
                summary.last_seq = Some(session.last_seq);
                if args.connection.json {
                    emit(stdout, &session)?;
                } else {
                    writeln!(stdout, "Stopped session {}", session.id).map_err(output_error)?;
                }
                Ok(())
            }
            SessionCommand::Steer { id, prompt, task } => {
                summary.session_id = Some(id.clone());
                let text = prompt.join(" ");
                agent::deliver(
                    client,
                    id,
                    task.as_deref(),
                    &text,
                    MessageMode::Steer,
                    args.connection.json,
                    stdout,
                )
                .await
            }
            SessionCommand::Queue { id, prompt } => {
                summary.session_id = Some(id.clone());
                let text = prompt.join(" ");
                agent::deliver(
                    client,
                    id,
                    None,
                    &text,
                    MessageMode::Queue,
                    args.connection.json,
                    stdout,
                )
                .await
            }
            SessionCommand::Archive { id } => {
                summary.session_id = Some(id.clone());
                let session = archive(client, id).await?;
                if args.connection.json {
                    emit(stdout, &session)
                } else {
                    writeln!(stdout, "ok").map_err(output_error)
                }
            }
            SessionCommand::Delete { id } => {
                summary.session_id = Some(id.clone());
                client.send("DELETE", &client::session_path(id, "")).await?;
                if args.connection.json {
                    emit(stdout, &json!({"deleted": id}))
                } else {
                    writeln!(stdout, "ok").map_err(output_error)
                }
            }
            SessionCommand::Tasks { id } => {
                agent::tasks(client, id, args.connection.json, stdout, summary).await
            }
            SessionCommand::Resume {
                id,
                prompt,
                last_event_id,
            } => {
                let text = (!prompt.is_empty()).then(|| prompt.join(" "));
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
                if let Some(text) = &text {
                    send_message(client, id, text).await?;
                } else if last_seq == session.last_seq && session.status != SessionStatus::Running {
                    summary.status = status_name(session.status);
                    print_waiting(args.connection.json, stdout, id, summary.status)?;
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
                follow_or_detach(
                    args,
                    client,
                    id,
                    ReplayPoint {
                        last_seen: last_seq,
                        terminal_after,
                        end_at,
                        // A new prompt continues the session the way `exec` runs one.
                        wait_tickets: text.is_some(),
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
                if args.connection.json {
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
                print_ticket(args.connection.json, client, stdout, &ticket, summary)?;
                if !args.connection.json && ticket.card.is_none() {
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
                print_ticket(args.connection.json, client, stdout, &ticket, summary)
            }
            TicketCommand::Done { id } => {
                let ticket: Ticket = client
                    .json::<_, Value>("POST", &client::ticket_path(id, "done"), None)
                    .await?;
                print_ticket(args.connection.json, client, stdout, &ticket, summary)
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
                print_ticket(args.connection.json, client, stdout, &ticket, summary)
            }
        },
        Some(Command::Routine { command }) => {
            routines::execute(command, args.connection.json, client, stdout).await
        }
        Some(Command::Memory { command }) => {
            memory::execute(command, args.connection.json, client, stdout).await
        }
        Some(Command::Skills { command }) => {
            skills::execute(command, args.connection.json, client, stdout).await
        }
        Some(Command::Token { command }) => {
            token::execute(command, args.connection.json, client, stdout).await
        }
        Some(Command::Policy { command }) => {
            policy::execute(command, args.connection.json, client, stdout).await
        }
        Some(Command::Config { command }) => {
            config::execute(command, args.connection.json, client, stdout).await
        }
        Some(Command::Vault { command }) => {
            vault::execute(command, args.connection.json, client, stdin, stdout, stderr).await
        }
        Some(Command::Connections { command }) => {
            connections::execute(
                command,
                args.connection.json,
                client,
                stdout,
                stderr,
                summary,
            )
            .await
        }
        Some(Command::Sync { command }) => {
            sync::execute(command, args.connection.json, client, stdout).await
        }
        Some(Command::Repl {
            session, tab, code, ..
        }) => {
            let scratch = match session {
                Some(_) => None,
                None => {
                    let mut create = create_request(
                        context,
                        &TaskOptions::default(),
                        Some("openside repl".into()),
                    )
                    .await?;
                    create.workspace = None;
                    let created: Session =
                        client.json("POST", "/v1/sessions", Some(&create)).await?;
                    writeln!(stderr, "Session: {}", created.id).map_err(output_error)?;
                    Some(created.id.to_string())
                }
            };
            let id = session
                .as_deref()
                .or(scratch.as_deref())
                .unwrap_or_default();
            let code = (!code.is_empty()).then(|| code.join(" "));
            let result = repl::execute(
                repl::ReplOptions {
                    session: id,
                    tab: tab.as_deref(),
                    code: code.as_deref(),
                    json: args.connection.json,
                },
                client,
                stdin,
                stdout,
                stderr,
                summary,
            )
            .await;
            if scratch.is_some() {
                forget_session(context, id, stderr).await;
            }
            result
        }
        Some(Command::Pair(pair_args)) => {
            pair::pair(pair_args, args.connection.json, client, stdout).await
        }
        Some(Command::Device { command }) => {
            pair::device(command, args.connection.json, client, stdout).await
        }
        Some(Command::Remote { command }) => match command {
            remote::RemoteCommand::Status => {
                remote::status(client, args.connection.json, stdout).await
            }
            remote::RemoteCommand::LocalAccess => {
                remote::local_access(client, args.connection.json, stdout).await
            }
            remote::RemoteCommand::Setup(_) => {
                unreachable!("remote setup runs without an API client")
            }
        },
        Some(
            Command::Mcp { .. }
            | Command::Doctor { .. }
            | Command::Import(_)
            | Command::Service { .. }
            | Command::Login(_)
            | Command::Logout
            | Command::Guide { .. }
            | Command::Update { .. },
        ) => unreachable!("runs without the CLI's API client"),
    }
}

/// The prompt and new-session options of `openside [prompt...]` and `openside exec`.
struct Exec<'a> {
    task: &'a TaskOptions,
    prompt: &'a [String],
}

async fn exec<R: Read, W: Write, E: Write>(
    args: &Args,
    context: &Context<'_>,
    run: Exec<'_>,
    stdin: &mut R,
    stdout: &mut W,
    stderr: &mut E,
    summary: &mut Summary,
) -> Result<(), String> {
    let client = context.client;
    let prompt = if run.prompt == ["-"] {
        let mut text = String::new();
        stdin
            .read_to_string(&mut text)
            .map_err(|_| "could not read the prompt from stdin".to_owned())?;
        text
    } else {
        run.prompt.join(" ")
    };
    if prompt.trim().is_empty() {
        return Err("the prompt is empty".into());
    }
    let create = create_request(context, run.task, None).await?;
    let session: Session = client.json("POST", "/v1/sessions", Some(&create)).await?;
    let id = session.id.to_string();
    summary.session_id = Some(id.clone());
    send_message(client, &id, &prompt).await?;
    let followed = follow_or_detach(
        args,
        client,
        &id,
        ReplayPoint {
            last_seen: 0,
            terminal_after: session.last_seq,
            end_at: None,
            wait_tickets: true,
        },
        stdout,
        stderr,
        summary,
    )
    .await;
    // A detached run is still going; archiving it would close its isolated tabs.
    if followed.is_ok() && !summary.interrupted && matches!(summary.status, "settled" | "stopped") {
        forget_session(context, &id, stderr).await;
    }
    followed
}

/// The prompt when it is exactly one absolute http(s) URL and nothing else.
fn bare_url(prompt: &[String]) -> Option<&str> {
    let [only] = prompt else {
        return None;
    };
    let text = only.trim();
    let url = url::Url::parse(text).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host().is_some()).then_some(text)
}

/// `openside <url>`: open the page in a new openside browser tab, like a person typing it in
/// the address bar, without starting an agent run. The session that owns the tab is archived
/// like an `exec` session, except in an isolated or incognito profile, where archiving would
/// close the tab that was just opened.
async fn open_url<W: Write, E: Write>(
    args: &Args,
    context: &Context<'_>,
    url: &str,
    stdout: &mut W,
    stderr: &mut E,
    summary: &mut Summary,
) -> Result<(), String> {
    let client = context.client;
    let mut create = create_request(context, &args.task, Some(format!("Open {url}"))).await?;
    create.workspace = None;
    let shared = create.browser_space == BrowserSpace::Shared && !create.incognito;
    let session: Session = client.json("POST", "/v1/sessions", Some(&create)).await?;
    let id = session.id.to_string();
    summary.session_id = Some(id.clone());
    let arguments = json!({ "url": url });
    let tool_call::Outcome { value, ok } = tool_call::drive(
        client,
        None,
        Some(None),
        |approval| tool_call::tool(client, &id, "browser_open_tab", &arguments, approval),
        |ticket, approval| {
            // Best effort: a closed stderr must not abort a call a person is about to answer.
            let _ = if approval {
                writeln!(
                    stderr,
                    "Approval needed (ticket {ticket}). Ask a person to approve on another paired web or phone device; this client cannot approve its own work.\nWaiting; the tab opens once approved."
                )
            } else {
                writeln!(
                    stderr,
                    "Waiting for a person to answer ticket {ticket} in the openside app (or: {}).",
                    client.ticket_command(ticket, "resolve", None)
                )
            };
            let _ = stderr.flush();
        },
    )
    .await?;
    if shared {
        forget_session(context, &id, stderr).await;
    }
    if !ok {
        summary.failed = true;
        summary.status = "error";
        if args.connection.json {
            return emit(stdout, &value);
        }
        return Err(format!("could not open {url}: {value}"));
    }
    if args.connection.json {
        return emit(stdout, &value);
    }
    match value["result"]["tab_id"].as_str() {
        Some(tab) => writeln!(stdout, "Opened {url} in tab {tab}"),
        None => writeln!(stdout, "Opened {url}"),
    }
    .map_err(output_error)
}

/// Archive a session this command created once it is done, unless the daemon's
/// `save_sessions` preference keeps CLI sessions. A failure only warns: the run itself
/// succeeded.
async fn forget_session<E: Write>(context: &Context<'_>, id: &str, stderr: &mut E) {
    if settings::keeps_sessions(context.client).await {
        return;
    }
    if let Err(error) = archive(context.client, id).await {
        let _ = writeln!(
            stderr,
            "openside: warning: could not archive session {id}: {error}"
        );
    }
}

async fn archive(client: &Client, id: &str) -> Result<Session, String> {
    client
        .json(
            "PATCH",
            &client::session_path(id, ""),
            Some(&json!({"archived": true})),
        )
        .await
}

/// The account and browser profile new sessions use, from the flags and the CLI settings.
struct Chosen {
    account: Option<AccountSelector>,
    profile: CliProfile,
}

/// `--account` beats the saved account; `--profile` beats the profile in `--account u1:<id>`,
/// which beats the saved profile. An explicit account without a profile uses its first profile.
fn choose(
    account: Option<&str>,
    profile: Option<&str>,
    saved: &settings::CliSettings,
) -> Result<Chosen, String> {
    let flag = account.map(AccountSelector::parse).transpose()?;
    let profile = match profile {
        Some(id) => settings::parse_profile(id)?,
        None => match &flag {
            Some(selector) => selector.profile.unwrap_or(CliProfile::Shared),
            None => saved.profile.unwrap_or(CliProfile::Shared),
        },
    };
    let account = flag.or_else(|| {
        saved.account.clone().map(|id| AccountSelector {
            account: AccountRef::Id(id),
            profile: None,
        })
    });
    Ok(Chosen { account, profile })
}

/// The agent id an account selector names; `uN` needs the daemon's roster.
async fn agent_id(
    client: &Client,
    selector: Option<&AccountSelector>,
) -> Result<Option<String>, String> {
    match selector {
        None => Ok(None),
        Some(AccountSelector {
            account: AccountRef::Id(id),
            ..
        }) => Ok(Some(id.clone())),
        Some(selector) => account::resolve(client, selector).await.map(Some),
    }
}

/// `--host` (or `OPENSIDE_HOST`), else the saved default unless `--socket`/`--url` picks the
/// connection; `None` is this computer's daemon.
fn selected_host(args: &Args, saved: &settings::CliSettings) -> Result<Option<String>, String> {
    match &args.host {
        Some(host) => host::canonical(host),
        None if args.connection.socket.is_some() || args.connection.url.is_some() => Ok(None),
        None => match &saved.host {
            Some(host) => host::canonical(host),
            None => Ok(None),
        },
    }
}

/// The account and profile flags of the command being run.
fn target_of(args: &Args) -> (Option<&str>, Option<&str>) {
    let target = match &args.command {
        None => return (args.account.as_deref(), args.profile.as_deref()),
        Some(
            Command::Exec { target, .. }
            | Command::Repl { target, .. }
            | Command::Mcp { target }
            | Command::Session {
                command: SessionCommand::New { target, .. },
            },
        ) => target,
        Some(_) => return (None, None),
    };
    (target.account.as_deref(), target.profile.as_deref())
}

/// The new-session model options of the command being run, if it takes them.
fn task_options(args: &Args) -> Option<&TaskOptions> {
    match &args.command {
        None => Some(&args.task),
        Some(
            Command::Exec { task, .. }
            | Command::Session {
                command: SessionCommand::New { task, .. },
            },
        ) => Some(task),
        Some(_) => None,
    }
}

fn root_task_flags(args: &Args) -> bool {
    let task = &args.task;
    args.account.is_some()
        || args.profile.is_some()
        || task.model.is_some()
        || task.provider.is_some()
        || task.speed.is_some()
        || task.effort.is_some()
        || task.permission.is_some()
        || task.workspace.is_some()
}

fn check_task_options(task: &TaskOptions) -> Result<(), String> {
    model_selector(task).map(|_| ())
}

/// `--effort` as the daemon's per-session effort and agent profile. `ultrabrowse` is openside's
/// research profile rather than a reasoning level: it selects `profile: ultrabrowse` at `max`.
fn session_effort(task: &TaskOptions) -> (Option<ThinkingEffort>, Option<SessionProfile>) {
    let effort = match task.effort {
        None => None,
        Some(Effort::Off) => Some(ThinkingEffort::Off),
        Some(Effort::Minimal) => Some(ThinkingEffort::Minimal),
        Some(Effort::Low) => Some(ThinkingEffort::Low),
        Some(Effort::Medium) => Some(ThinkingEffort::Medium),
        Some(Effort::High) => Some(ThinkingEffort::High),
        Some(Effort::Xhigh) => Some(ThinkingEffort::Xhigh),
        Some(Effort::Max) => Some(ThinkingEffort::Max),
        Some(Effort::Ultrabrowse) => {
            return (Some(ThinkingEffort::Max), Some(SessionProfile::Ultrabrowse))
        }
    };
    (effort, None)
}

/// `-m provider:model`, `-m provider/model`, or `-m model -p provider`; daemon model ids are
/// `provider:model`.
fn model_selector(task: &TaskOptions) -> Result<Option<String>, String> {
    match (task.model.as_deref(), task.provider.as_deref()) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err("--provider needs --model".into()),
        (Some(model), provider) => {
            if model.is_empty() {
                return Err("--model must not be empty".into());
            }
            Ok(Some(
                if let Some((provider, name)) = model.split_once('/') {
                    format!("{provider}:{name}")
                } else if model.contains(':') {
                    model.to_owned()
                } else if let Some(provider) = provider {
                    format!("{provider}:{model}")
                } else {
                    model.to_owned()
                },
            ))
        }
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

/// A new-session request from the model options, the chosen account and browser profile;
/// workspace paths are made absolute.
async fn create_request(
    context: &Context<'_>,
    task: &TaskOptions,
    title: Option<String>,
) -> Result<CreateSessionRequest, String> {
    let (effort, profile) = session_effort(task);
    let mut create = CreateSessionRequest {
        agent_id: agent_id(context.client, context.chosen.account.as_ref()).await?,
        title,
        model: model_selector(task)?,
        mode: task.permission.map(Into::into),
        workspace: task
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
        project_id: None,
        incognito: false,
        browser_space: BrowserSpace::Shared,
        effort,
        speed: task.speed.map(|speed| match speed {
            Speed::Default => SessionSpeed::Default,
            Speed::Fast => SessionSpeed::Fast,
        }),
        profile,
        confirmation: None,
    };
    context.chosen.profile.apply(&mut create);
    Ok(create)
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
    /// `exec`: a settlement that leaves tickets pending is not the end; keep following until
    /// the resumed run settles with none left or the session stops.
    wait_tickets: bool,
}

/// What Ctrl-C prints while following: the run is not stopped, only this terminal detaches.
fn detach_notice(prefix: &str, id: &str) -> String {
    format!(
        "Session {id} keeps running. Resume: {prefix} session resume {id} · Stop: {prefix} session stop {id}"
    )
}

/// [`follow`], except that Ctrl-C detaches instead of killing the process mid-stream: the run
/// keeps going on the daemon, the notice says how to get back, and the exit code is 130.
async fn follow_or_detach<W: Write, E: Write>(
    args: &Args,
    client: &Client,
    id: &str,
    replay: ReplayPoint,
    stdout: &mut W,
    stderr: &mut E,
    summary: &mut Summary,
) -> Result<(), String> {
    {
        // Dropping the follow future first lets its stderr guard erase the live status line.
        let run = std::pin::pin!(follow(
            args,
            client,
            id,
            replay,
            &mut *stdout,
            &mut *stderr,
            &mut *summary
        ));
        tokio::select! {
            result = run => return result,
            // Without a SIGINT handler the branch is disabled and the default action stays.
            Ok(()) = tokio::signal::ctrl_c() => {}
        }
    }
    // Installing the handler replaced the default action; restore "a second Ctrl-C exits now"
    // for whatever remains (the notice, the JSON summary, flushing).
    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            std::process::exit(130);
        }
    });
    summary.interrupted = true;
    writeln!(stderr, "\n{}", detach_notice(&client.short_prefix(), id)).map_err(output_error)
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
        .filter(|_| !args.connection.json)
        .map(|ui| ui::live::Live::new(*ui));
    if let Some(live) = live.as_mut() {
        live.begin(id, stderr)?;
    }
    let mut last_seq = replay.last_seen;
    let cursor = Cursor::new(client.endpoint_key(), id)?;
    summary.last_seq = Some(last_seq);
    let mut pause = wait::Pause::default();
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
            pause.open_tickets.insert(ticket.id.to_string());
            seeded.push(ticket);
        }
    }
    let mut response = client.events(id, last_seq).await?;
    let mut decoder = Decoder::default();
    let mut saw_delta = false;
    let mut ends_with_newline = true;
    // Set once `exec` settles on a pause (tickets pending, or a deferred run not yet resumed);
    // from then on a closed stream is reopened.
    let mut parked = false;
    loop {
        let Some(frame) = ui::live::next_frame(&mut response, &mut live, stderr).await else {
            if !parked {
                break;
            }
            response = wait::reopen(client, id, last_seq).await?;
            decoder = Decoder::default();
            continue;
        };
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
            if args.connection.json {
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
                    shown_tickets.insert(ticket.id.to_string());
                    // On a terminal the notice shares the screen with a half-written reply line.
                    if !args.connection.json && !ends_with_newline {
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
                EventKind::MessageCompleted {
                    outcome: MessageOutcome::Deferred,
                    ..
                } => {
                    summary.status = "waiting_human";
                }
                _ => {}
            }
            pause.apply(&event.kind);
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
                    EventKind::SessionSettled {} => match pause.settled() {
                        wait::Settled::Finished => Some("settled"),
                        wait::Settled::AwaitingPerson => Some("waiting_human"),
                        wait::Settled::Resuming => Some("resuming"),
                    },
                    EventKind::SessionStopped {} => Some("stopped"),
                    _ => None,
                }
            };
            let terminal = terminal.filter(|_| event.seq > replay.terminal_after);
            // The ticket was answered before the pause settled: the run resumes on its own,
            // so keep following (and reopen the stream if the daemon closes it meanwhile).
            if terminal == Some("resuming") {
                parked = true;
                continue;
            }
            if let (Some("waiting_human"), true) = (terminal, replay.wait_tickets) {
                parked = true;
                summary.status = "waiting_human";
                if !args.connection.json && !ends_with_newline {
                    writeln!(stdout).map_err(output_error)?;
                    ends_with_newline = true;
                    stdout.flush().map_err(output_error)?;
                }
                let ids = pause
                    .open_tickets
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(
                    stderr,
                    "Waiting for a person to answer ticket {ids}; the run continues once it is resolved. List open tickets with `openside ticket list`; `openside session resume {id}` follows this session later."
                )
                .map_err(output_error)?;
                stderr.flush().map_err(output_error)?;
                continue;
            }
            if let Some(status) = terminal {
                summary.status = status;
                if !args.connection.json && !ends_with_newline {
                    writeln!(stdout).map_err(output_error)?;
                }
                if !args.connection.json {
                    for ticket in &seeded {
                        if pause.open_tickets.contains(ticket.id.as_str())
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
                    print_waiting(args.connection.json, stdout, id, status)?;
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
