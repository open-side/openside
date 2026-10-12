---
name: openside-browser
description: Use the user's openside browser agent and browser (real logins from their vault, approvals and hand-off to the user's phone). Delegate a whole web task with `openside run`, or drive the browser step by step with `openside repl` / `openside browser`. Use when a task needs a real browser, a login, or a page you cannot fetch directly.
license: MIT OR Apache-2.0
compatibility: Needs the `openside` CLI and a reachable `opensided` daemon (local, or a paired remote with `--remote NAME`).
metadata:
  openside-bundled: "1"
---
# Browsing through openside

openside owns a browser profile with the user's logins. Secrets stay in its vault, risky steps
wait for the user's approval, and when a person is needed (sign-in, code, choice, CAPTCHA) the
user gets a ticket in the openside web app or on their phone. Login values are omitted from
tool results; scripts you run on a page can still read its filled fields. Do not read or
return those values.

## Delegate or drive?

- **Delegate** (`openside run`) when the task has several steps or needs judgement: search,
  compare, fill a form, sign in. The openside agent plans and browses; you get its answer.
- **Drive** (`openside repl`, `openside browser`) when you know the exact steps, need raw page
  data, or want to check something quickly. One `repl` script can run many actions in one call.

## Delegate a task

```sh
openside run --wait "find the release notes for version X on example.org and summarise them"
openside run "book the cheapest nonstop flight to Busan next Friday"   # prints SESSION MESSAGE
openside tasks <session-id>                     # its Tasks, their status and results
openside steer <session-id> "only morning flights"   # add --task <task-id> for one Task
openside cancel <session-id>                    # or --task <task-id>
```

- `--wait` returns when the run ends and prints the agent's replies; it keeps waiting while the
  user answers tickets. `--timeout SECS` bounds it. `--follow` streams progress instead.
- `--json` prints one JSON object per line (`--wait` ends with `{"type":"result",...}`).
- `--session ID` continues an existing session. `--mode read-only|guard|full` and
  `--browser-space isolated` (separate cookies) apply to new sessions.

## Drive the browser

```sh
S=$(openside session new)
openside repl "$S" <<'JS'
await page.goto("https://example.org/pricing");
await page.getByRole("link", { name: "Enterprise" }).click();
await page.snapshot()
JS
```

- Playwright syntax with `page`, `expect` and `console`; the last expression is printed.
  `page.snapshot()` returns the accessibility tree with `eN` refs, usable as
  `page.locator("aria-ref=eN")`.
- Single tools, with the same names and arguments the openside agent uses
  (`openside browser tools` lists them with schemas):

```sh
openside browser open "$S" url=https://example.org          # prints the tab_id
openside browser snapshot "$S" tab_id=T1 scope=viewport
openside browser act "$S" tab_id=T1 target=e12 action=click
openside browser screenshot "$S" tab_id=T1                  # prints the artifact path
openside browser login "$S" tab_id=T1        # the user signs in on their device; waits
openside browser human-step "$S" '{"tab_id":"T1","kind":"takeover","summary":"Solve the CAPTCHA"}'
```

When a step needs approval or a person, the command prints the ticket and waits until the user
answers in the openside app (approvals rerun the step). `--no-wait` returns the ticket instead.

## Tickets

```sh
openside ticket list
openside ticket show <ticket-id>
openside ticket resolve <ticket-id> <value>   # user-provided non-secret answers, or false to deny
```

Never put passwords, tokens or one-time codes in a goal, a script or `ticket resolve`. The user
enters secrets themselves in the openside app.

The daemon requires a paired `person_device` for confirm approval, choice answers,
authenticator enrollment and saving a login. Every `agent_client` and the local socket
receive `403 person_approval_required`, even for tickets created by another agent.
Ask the user to respond on their paired web/phone/extension/channel device, then use MCP
`ticket_wait`; browser and run commands already wait for human tickets. MCP refuses to
resolve confirm approvals, choice, login and secret-input tickets before sending values.
Non-secret info answers and confirm denial remain allowed. Never retry approval with
another agent token or use a person token. Only a person should issue and hold
`openside token create --person` credentials for the extension or their other clients.

## Remote daemon and MCP

- `--remote NAME` (or `OPENSIDE_REMOTE=NAME`) sends any command to a paired remote daemon. What
  works there follows the pairing token's scopes. `openside connect` saves a profile and
  `openside remote profiles list` shows the saved ones.
- Ask the daemon's operator to issue your link with `openside pair --agent`, then redeem it
  with `openside connect`. The link fixes the client role; `connect` cannot change it.
  Do not use a person's device token for agent work.
- `openside mcp` exposes the same abilities as MCP tools: `agent_run`, `agent_result`,
  `agent_status`, `agent_steer`, `agent_cancel`, the browser tools, `browser_repl` and
  `ticket_wait`.

## Troubleshooting

`openside doctor` reports missing browsers, credentials and configuration problems without
starting anything.
