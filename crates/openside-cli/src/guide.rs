//! `openside guide [repl]`: how a coding agent (or a person) should drive openside from a
//! terminal. Plain Markdown on stdout, so an agent can read it as instructions.

use std::io::Write;

use clap::ValueEnum;

use crate::output_error;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum Topic {
    /// Print the detailed REPL guide
    Repl,
}

const OVERVIEW: &str = r#"# openside

openside is a browser with an agent built in. The agent works in the user's own browser
profile, with their sign-ins (through the openside vault), their history and their memory, and
asks the user on their phone or the web app when a step needs a person.

There are two ways to use it from a terminal:
- `openside exec` hands a task to the openside agent. Use it for almost everything; it works
  like starting a subagent that already knows the user's sites.
- `openside repl` runs Playwright-syntax JavaScript in an agent tab. Use it only when you need
  to read the page or drive it step by step yourself.

`openside --help` and `openside <command> --help` list the current options and examples.

## Browser tasks

### A. `openside exec` (recommended)

The task runs in its own session. The command streams the agent's progress to stderr and its
replies to stdout, waits through any step a person has to approve, and prints the session id.
For long tasks, tell the user what the agent is doing about once a minute.

```bash
openside "Reply to Jack in the #develop Slack channel: <text>"
openside -m anthropic:claude-sonnet-4-5 "Find the cheapest direct flight to Lisbon next Friday"
openside --account u1 "Summarize my unread notifications"
openside --permission full-access "Fill in the expense report from the receipts tab"
openside --profile isolated "Check the page while signed out"
```

#### Following up

Continue, redirect or stop a session with:

```bash
openside session resume <id>
openside session resume <id> "Also check the return flight"
openside session steer <id> "Use the airline's own site instead"
openside session queue <id> "Then export the list to CSV"
openside session stop <id>
openside session archive <id>
openside session delete <id>
```

`steer` changes course at the agent's next step; `queue` runs after the current turn. Both
print `ok` and return at once. `openside session list` also shows sessions that were not saved
to the chat list.

#### Saving sessions

By default a finished `exec` or `repl` session is archived, so it stays out of the web app's
chat list; `openside settings save-sessions true` keeps them. `openside settings
set-default-profile <profile>` (or `u1:<profile>`) picks the browser profile new sessions use;
`default` goes back to the shared profile. For one command, pass `--profile <profile>` or
`--account u1:<profile>`.

### B. `openside repl`

Use it to inspect a page or run a precise sequence of actions in a tab.

**Run `openside guide repl` and read it before using `openside repl`.**

## Memory

openside keeps notes about the user and their browsing history. Search it before asking the
user for context they may already have given:

```bash
openside memory search "<query>" --json
openside memory list --json
openside memory show <id>
openside memory path
```

Do not edit the memory database yourself. To have openside remember something, ask through
`openside exec` or use `openside memory add`. Under `openside mcp`, use the `memory_search`
tool instead of the command.

## Another computer

When openside runs on a different machine than this terminal (for example, this is a Linux
server and openside runs on the user's laptop), pair this machine once and then pick that host:

```bash
openside login                       # paste the link from `openside pair` on that machine
openside host list
openside host profiles --host <name>
openside exec --host <name> "..."
openside repl --host <name>
openside host use <name>             # make it the default for later commands
```

That machine needs a remote transport (`openside remote setup tailscale|cloudflare|relay`)
and has to be online. Profiles are `shared` (default), `isolated` and `incognito`; choose one
with `--profile` on `exec`, `repl` or `mcp`. Sessions keep the profile they started with.
Under MCP, call `browser_profiles` and pass its `profileId` as `profile_id` to `repl` or to a
new `exec`; leave it out when continuing an `exec` session with `session_id`.
"#;

const REPL: &str = r#"# `openside repl`

Runs Playwright-syntax JavaScript in a session's agent tabs. Calls go through the same policy,
approvals and audit log as the agent's own `browser_repl` tool.

```bash
openside repl "await openTab('https://example.com'); await page.title()"
openside repl --session <id> "await page.snapshot()"
openside repl --session <id> --tab <tab-id> -     # one snippet from stdin
openside repl --session <id>                      # one snippet per stdin line
```

Without `--session`, the command starts a session (using `--account` and `--profile`) and
prints its id on stderr; pass that id with `--session` to keep working in the same tabs.

## Environment

- QuickJS, modern JavaScript with top-level `await`. No `import`, `require`, file system,
  network or process access in the script itself.
- Globals: `page` (unset until you open or pick a tab), `openTab(url)`, `getTabs()`, `expect`
  (Playwright assertions) and `console`.
- Calls in one session share a context: `const`/`let` bindings and attached tabs carry over
  until the session's REPL has been idle for 10 minutes.
- Limits: 120 seconds per call, 5 seconds default per action (`timeout` option), 32 MiB heap.
- The value of the last expression is printed, then anything written with `console.log`.

## Tabs

- `openTab(url)` opens a tab, waits until it is interactive and makes it `page`; `getTabs()`
  lists the tabs attached to this REPL.
- `--tab <id>` makes one of the session's agent tabs `page` for the first snippet of this
  command; later snippets keep whatever `page` became.
- The `--json` result carries `tab_id` (the tab `page` points at) and `tabs`.

## Reading the page

Start with `await page.snapshot()`: an accessibility outline with `[ref=eN]` markers. Act on
those refs with `page.locator('aria-ref=eN')`; refs are shared with the agent's own snapshots
of the same tab. Read details with locators (`getByRole`, `getByText`, `getByLabel`,
`locator(css)`) and `innerText()`, `textContent()`, `getAttribute()`, `count()`. Use
`page.screenshot()` when you need to see layout.

`page.evaluate`, `$eval` and `$$eval` run code inside the page when the daemon's policy allows
it; prefer locators.

## Actions

Locators wait until the element is attached, visible, enabled and stable, and fail on more than
one match (strict mode). Clicks and typing are real input events.

```js
await page.goto('https://example.com/login');
await page.getByLabel('Email').fill('me@example.com');
await page.getByRole('button', { name: 'Continue' }).click();
await expect(page.getByText('Welcome')).toBeVisible();
```

Frames: `page.frameLocator(selector)` and `locator.contentFrame()`.

## Sign-in and approvals

Do not type passwords from the terminal. Ask the openside agent (`openside exec`) to sign in;
it uses the user's vault. A step that needs approval prints a ticket id and waits until a
person answers it in the openside web app or on their phone, then reruns the snippet from the
start.

## Limits compared with a full Playwright session

There is no file download API in the REPL; ask `openside exec` for downloads.
"#;

pub(crate) fn print<W: Write>(topic: Option<Topic>, stdout: &mut W) -> Result<(), String> {
    let text = match topic {
        None => OVERVIEW,
        Some(Topic::Repl) => REPL,
    };
    writeln!(stdout, "openside CLI {}\n", env!("CARGO_PKG_VERSION")).map_err(output_error)?;
    write!(stdout, "{text}").map_err(output_error)
}
