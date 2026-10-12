---
name: openside-browser
description: Delegate web browsing and research to the local openside daemon (an agent-owned browser with approvals, vault logins and human hand-off). Use when a task needs a real browser session, a login, or a page you cannot fetch directly.
license: MIT OR Apache-2.0
compatibility: Needs the `openside` CLI and a running `opensided` daemon on the same machine.
metadata:
  openside-bundled: "1"
---
# Browsing through openside

openside runs browser sessions in its own daemon-owned Chromium profile. Secrets stay in the
openside vault, risky steps wait for the user's approval, and a human can take over. Hand a
browsing task to it instead of driving a browser yourself.

## Run a task

```sh
openside --mode guard --workspace . "find the release notes for version X and summarise them"
```

- The command creates a session, streams the agent's progress and exits when the session settles.
- Add `--json` for one JSON object per line and a final `{"type":"summary", ...}` object.
- `--mode read-only` forbids changes, `guard` asks before leaving the workspace, `full` still asks for sensitive actions.
- Exit status `0` with status `waiting_human` means a step needs the user, not that the task failed.

## Follow and steer sessions

```sh
openside session list
openside session resume <session-id> "also check the pricing page"
openside session stop <session-id>
```

## Human steps

When the agent needs an approval, a secret, a device confirmation or a manual takeover, it opens a ticket.
List them and tell the user; never invent an answer.

```sh
openside ticket list
openside ticket show <ticket-id>
openside ticket resolve <ticket-id> <value>   # approvals and answers
openside ticket done <ticket-id>              # the user finished a manual step
openside ticket reject <ticket-id> --reason "not now"
```

Never put passwords, tokens or one-time codes in a task prompt or in `ticket resolve` arguments.
The user enters secrets themselves (`openside ticket resolve <id> -` reads one from stdin).

## Page scripting

```sh
openside repl <session-id> --eval 'await page.goto("https://example.com"); return await page.title()'
```

## Troubleshooting

`openside doctor` reports missing browsers, credentials and configuration problems without starting anything.
