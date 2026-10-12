# CLI reference

Generated from the `openside` command definitions by `cargo run -q -p openside-cli --example cli-docs`; do not edit by hand (`tools/gen-docs.sh` regenerates it).

## `openside`

Non-interactive openside API client

```text
Usage: openside [OPTIONS] [TASK] [COMMAND]
```

| Argument | Description |
|---|---|
| `--json` | Emit JSON objects, one per line, with a final summary (task: the default when stdout is not a terminal) |
| `--socket <PATH>` |  |
| `--url <URL>` | Loopback HTTP endpoint (requires a token) |
| `--token <TOKEN>` | Environment: `OPENSIDE_TOKEN`. |
| `[TASK]` |  |
| `--model <MODEL>` | Model for the new session |
| `--mode <MODE>` | Permission mode for the new session One of `read-only`, `guard`, `full`. |
| `--workspace <PATH>` | Workspace for the new session |

`--socket`, `--url`, `--token` and `--json` are global and apply to every subcommand.

### `openside accounts`

Show credential-source accounts, configured models, usage and rate-limit blocks

```text
Usage: openside accounts [OPTIONS]
```

### `openside session`

```text
Usage: openside session [OPTIONS] <COMMAND>
```

#### `openside session list`

```text
Usage: openside session list [OPTIONS]
```

#### `openside session resume`

```text
Usage: openside session resume [OPTIONS] <ID> [TEXT]
```

| Argument | Description |
|---|---|
| `<ID>` |  |
| `[TEXT]` |  |
| `--last-event-id <SEQ>` | Override the persisted last-seen SSE sequence |

#### `openside session stop`

```text
Usage: openside session stop [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

### `openside ticket`

```text
Usage: openside ticket [OPTIONS] <COMMAND>
```

#### `openside ticket list`

```text
Usage: openside ticket list [OPTIONS]
```

| Argument | Description |
|---|---|
| `--all` | Include tickets in every status |

#### `openside ticket show`

```text
Usage: openside ticket show [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside ticket resolve`

```text
Usage: openside ticket resolve [OPTIONS] <ID> [VALUE]
```

| Argument | Description |
|---|---|
| `<ID>` |  |
| `[VALUE]` | JSON or text; '-' reads secret_input from stdin; omitted on a secret_input ticket, asks at a hidden prompt |
| `--totp` | With '-' or the prompt: the input is the site's authenticator (otpauth:// URI or base32 setup key) to save in the vault instead of a code; request_otp tickets that offer registration only |

#### `openside ticket done`

```text
Usage: openside ticket done [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside ticket reject`

```text
Usage: openside ticket reject [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |
| `--reason <TEXT>` |  |

### `openside routine`

```text
Usage: openside routine [OPTIONS] <COMMAND>
```

#### `openside routine list`

List routines

```text
Usage: openside routine list [OPTIONS]
```

#### `openside routine show`

Show one routine

```text
Usage: openside routine show [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside routine add`

Create a routine that starts a new session or sends a heartbeat to an existing one

```text
Usage: openside routine add [OPTIONS] --name <NAME> --prompt <TEXT>
```

| Argument | Description |
|---|---|
| `--name <NAME>` | Required. |
| `--cron <EXPR>` | Five-field cron expression |
| `--timezone <ZONE>` | IANA time zone for --cron (default UTC) |
| `--interval-secs <SECS>` | Fixed interval instead of --cron |
| `--prompt <TEXT>` | Prompt sent on every run Required. |
| `--heartbeat <SESSION>` | Send the prompt to this existing session instead of starting a new one |
| `--model <MODEL>` |  |
| `--mode <MODE>` | One of `read-only`, `guard`, `full`. |
| `--workspace <PATH>` |  |
| `--catch-up` | Run once for the latest occurrence missed while the daemon was down |
| `--disabled` | Create the routine paused |

#### `openside routine run`

Run a routine now, outside its schedule

```text
Usage: openside routine run [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside routine runs`

Show recent runs of a routine, newest first

```text
Usage: openside routine runs [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |
| `--limit <N>` | Maximum number of runs to show |

#### `openside routine pause`

Stop scheduling a routine until it is resumed

```text
Usage: openside routine pause [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside routine resume`

Resume a paused routine; the next run is computed from now

```text
Usage: openside routine resume [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside routine rm`

Delete a routine and its run history

```text
Usage: openside routine rm [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside routine suggest`

Show routine drafts found in repeated requests; nothing is created

```text
Usage: openside routine suggest [OPTIONS]
```

### `openside memory`

```text
Usage: openside memory [OPTIONS] <COMMAND>
```

#### `openside memory search`

Search memories; without a query, list the most recently updated ones

```text
Usage: openside memory search [OPTIONS] [QUERY]
```

| Argument | Description |
|---|---|
| `[QUERY]` |  |
| `--kind <KIND>` | One of `note`, `history`. |
| `--limit <N>` |  |

#### `openside memory add`

Remember a note. Secrets (keys, tokens, passwords, card numbers) are rejected

```text
Usage: openside memory add [OPTIONS] <CONTENT>
```

| Argument | Description |
|---|---|
| `<CONTENT>` |  |
| `--tag <TAG>` |  |
| `--retention <RETENTION>` | One of `permanent`, `days_90`, `days_30`. |

#### `openside memory show`

Show one memory

```text
Usage: openside memory show [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside memory edit`

Change a memory's content, tags or retention; each change adds a history version

```text
Usage: openside memory edit [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |
| `--content <TEXT>` |  |
| `--tag <TAG>` |  |
| `--clear-tags` | Remove every tag |
| `--retention <RETENTION>` | One of `permanent`, `days_90`, `days_30`. |

#### `openside memory rm`

Delete a memory; stored history snapshots are scrubbed

```text
Usage: openside memory rm [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside memory history`

Show a memory's change history (time, actor, operation)

```text
Usage: openside memory history [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside memory model`

Manage the local embedding model

```text
Usage: openside memory model [OPTIONS] <COMMAND>
```

##### `openside memory model fetch`

Download a static (model2vec-style) embedding model into memory.embedding.model_dir

```text
Usage: openside memory model fetch [OPTIONS] <REPO>
```

| Argument | Description |
|---|---|
| `<REPO>` | Hugging Face repository, `owner/name` |
| `--revision <REVISION>` | Default: `main`. |
| `--endpoint <URL>` | Default: `https://huggingface.co`. Environment: `OPENSIDE_HF_ENDPOINT`. |
| `--config <PATH>` |  |

### `openside token`

Mint scoped device tokens

```text
Usage: openside token [OPTIONS] <COMMAND>
```

#### `openside token create`

Create a device token limited to the given scopes

```text
Usage: openside token create [OPTIONS] --scope <SCOPE>
```

| Argument | Description |
|---|---|
| `--scope <SCOPE>` | Scope to grant; repeat for several (read, steer, approve, human_step, browse, vault_use, otp_ingest, admin) Required. |
| `--name <NAME>` | Label shown for the device Default: `cli token`. |

### `openside vault`

List vault items, set per-item access policy, read the vault audit (admin)

```text
Usage: openside vault [OPTIONS] <COMMAND>
```

#### `openside vault status`

Show whether the vault is set up and unlocked, and how it is unlocked

```text
Usage: openside vault status [OPTIONS]
```

#### `openside vault init`

Create the vault: with an OS keyring nothing is asked; otherwise choose a passphrase

```text
Usage: openside vault init [OPTIONS]
```

#### `openside vault unlock`

Unlock the vault (asks for the passphrase of a passphrase vault)

```text
Usage: openside vault unlock [OPTIONS]
```

#### `openside vault lock`

Drop the decrypted vault from daemon memory until it is unlocked again

```text
Usage: openside vault lock [OPTIONS]
```

#### `openside vault add`

Add a login and/or an authenticator; values are asked for, never passed as arguments

```text
Usage: openside vault add [OPTIONS] --origin <ORIGIN> <ID>
```

| Argument | Description |
|---|---|
| `<ID>` | New item id (`[a-z0-9:_-]`, up to 64 characters) |
| `--origin <ORIGIN>` | Allowed origin, e.g. `https://example.com` or `https://*.example.com`; repeatable Required. |
| `--login` | Ask for a username and password (the default unless only --totp is given) |
| `--totp` | Ask for an authenticator setup key or `otpauth://` link |
| `--policy <POLICY>` | Per-item access policy; `default` inherits vault.default_policy One of `always`, `while_unlocked`, `never`, `default`. Default: `default`. |

#### `openside vault rm`

Delete an item

```text
Usage: openside vault rm [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside vault items`

List item ids, allowed origins and access policies (never values)

```text
Usage: openside vault items [OPTIONS]
```

#### `openside vault policy`

Set an item's access policy, or `default` to inherit vault.default_policy

```text
Usage: openside vault policy [OPTIONS] <ID> <POLICY>
```

| Argument | Description |
|---|---|
| `<ID>` |  |
| `<POLICY>` | One of `always`, `while_unlocked`, `never`, `default`. |

#### `openside vault audit`

Show audit records (time, session, tool call, item, origin, result), oldest first

```text
Usage: openside vault audit [OPTIONS]
```

| Argument | Description |
|---|---|
| `--since <TIME>` | Only records at or after this RFC 3339 time |
| `--item <ID>` |  |
| `--session <SESSION>` |  |
| `--limit <N>` |  |

### `openside sync`

End-to-end encrypted sync of pinned tabs and vault items between devices

```text
Usage: openside sync [OPTIONS] <COMMAND>
```

#### `openside sync setup`

Create a sync account on this device and print the pairing code for the next one

```text
Usage: openside sync setup [OPTIONS]
```

#### `openside sync pair`

Join the account of a pairing code (same passphrase in sync.passphrase_cmd)

```text
Usage: openside sync pair [OPTIONS] <CODE>
```

| Argument | Description |
|---|---|
| `<CODE>` | `osync1-…` code printed by `openside sync setup` |

#### `openside sync now`

Sync now: push local changes, pull and merge the other devices' changes

```text
Usage: openside sync now [OPTIONS]
```

#### `openside sync status`

Show whether sync is configured, set up and unlocked, and what is pending

```text
Usage: openside sync status [OPTIONS]
```

### `openside policy`

Show, replace or test the live daemon policy

```text
Usage: openside policy [OPTIONS] <COMMAND>
```

#### `openside policy show`

Print the live policy as JSON

```text
Usage: openside policy show [OPTIONS]
```

#### `openside policy set`

Replace the live policy with a JSON document (see `policy show`); applies immediately

```text
Usage: openside policy set [OPTIONS] <FILE>
```

| Argument | Description |
|---|---|
| `<FILE>` | File holding the policy JSON |

#### `openside policy test`

Evaluate one action without running it

```text
Usage: openside policy test [OPTIONS] <TARGET> <CAPABILITY>
```

| Argument | Description |
|---|---|
| `<TARGET>` | URL, origin or absolute path the action touches |
| `<CAPABILITY>` | fs_read, fs_write, exec, browse, fetch, vault_use or sensitive |
| `--mode <MODE>` | Session mode to evaluate under (default: the policy's default_mode) One of `read-only`, `guard`, `full`. |
| `--workspace <PATH>` | Approved workspace for guard mode |

### `openside skills`

```text
Usage: openside skills [OPTIONS] <COMMAND>
```

#### `openside skills list`

List skills; with a query, the ones the `/name` picker would offer

```text
Usage: openside skills list [OPTIONS] [QUERY]
```

| Argument | Description |
|---|---|
| `[QUERY]` |  |
| `--session <SESSION_ID>` | Also list the project skills of this session's workspace |

#### `openside skills show`

Show a skill's instructions and files

```text
Usage: openside skills show [OPTIONS] <NAME>
```

| Argument | Description |
|---|---|
| `<NAME>` |  |
| `--session <SESSION_ID>` |  |

#### `openside skills new`

Create a skill in the library

```text
Usage: openside skills new [OPTIONS] --description <TEXT> <NAME>
```

| Argument | Description |
|---|---|
| `<NAME>` |  |
| `--description <TEXT>` | Required. |
| `--body <TEXT>` | Instructions (the Markdown below the frontmatter) Default: ``. |

#### `openside skills enable`

Offer a skill to agents again

```text
Usage: openside skills enable [OPTIONS] <NAME>
```

| Argument | Description |
|---|---|
| `<NAME>` |  |

#### `openside skills disable`

Stop offering a skill to agents without deleting it

```text
Usage: openside skills disable [OPTIONS] <NAME>
```

| Argument | Description |
|---|---|
| `<NAME>` |  |

#### `openside skills rm`

Delete a skill from the library

```text
Usage: openside skills rm [OPTIONS] <NAME>
```

| Argument | Description |
|---|---|
| `<NAME>` |  |

#### `openside skills install`

Install openside's browser skill for another agent (for example `claude`, `codex`), so that agent can hand browsing to `openside`. Works without a daemon

```text
Usage: openside skills install [OPTIONS] <AGENT>
```

| Argument | Description |
|---|---|
| `<AGENT>` | Agent name; its skills directory comes from `[skills.agents]`, `OPENSIDE_SKILLS_<AGENT>_DIR`, or that agent's own convention |
| `--to-dir <PATH>` | Install into this skills directory instead |
| `--force` | Replace an installed copy |
| `--config <PATH>` |  |

#### `openside skills import`

Copy a skill into openside's library, from a directory or from another agent's skills directory. Works without a daemon

```text
Usage: openside skills import [OPTIONS] <SOURCE>
```

| Argument | Description |
|---|---|
| `<SOURCE>` | A skill directory, or a skill name when `--from` or `--from-dir` is given |
| `--from <AGENT>` | Agent whose skills directory holds `source` |
| `--from-dir <PATH>` | Skills directory that holds `source` |
| `--allow-scripts` | Keep executable files; without this a skill that has any is refused |
| `--force` | Replace a library skill of the same name |
| `--config <PATH>` |  |

### `openside repl`

Run Playwright-syntax JavaScript in a session's agent tab

```text
Usage: openside repl [OPTIONS] <SESSION>
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose agent tab runs the code |
| `--tab <TAB_ID>` | Agent tab to drive; optional when the session owns at most one tab |
| `--eval <CODE>` | Run this snippet instead of reading stdin |
| `-i, --interactive` | Run every stdin line as its own snippet |

### `openside mcp`

Serve the public API to MCP clients over stdio (docs/specs/mcp.md)

```text
Usage: openside mcp [OPTIONS]
```

### `openside import`

Import history, bookmarks and cookies from a Chrome, Chromium or Firefox profile

```text
Usage: openside import [OPTIONS] --from <BROWSER:DIR> --into <PROFILE>
```

| Argument | Description |
|---|---|
| `--from <BROWSER:DIR>` | Source profile as `<chrome\|chromium\|firefox>:<profile directory>` Required. |
| `--into <PROFILE>` | openside browser profile to write (1-64 characters of a-z, 0-9, `_`, `-`) Required. |
| `--what <CATEGORY>` | Comma-separated categories to import; default is all that the source has One of `history`, `bookmarks`, `cookies`. |
| `--dry-run` | Read and decrypt the source and report counts, but write nothing |
| `--keyring-password-cmd <COMMAND>` | Command that prints the browser's OS-keyring `Safe Storage` password, needed for v11 cookies. Run directly, without a shell; its output is never logged |
| `--config <PATH>` | Configuration file (data directory and browser location) |

### `openside doctor`

```text
Usage: openside doctor [OPTIONS]
```

| Argument | Description |
|---|---|
| `--config <PATH>` |  |

### `openside pair`

Pair a phone or browser: print a one-time QR code valid for 3 minutes (local socket)

```text
Usage: openside pair [OPTIONS]
```

| Argument | Description |
|---|---|
| `--scope <SCOPE>` | Scope the new device gets; repeat for several. Default: read, approve, human_step |
| `--name <NAME>` | Label shown in `openside device list` Default: `phone`. |

### `openside device`

List or revoke paired devices (admin)

```text
Usage: openside device [OPTIONS] <COMMAND>
```

#### `openside device list`

Paired devices with their scopes; tokens are never shown

```text
Usage: openside device list [OPTIONS]
```

#### `openside device revoke`

Revoke a device: its next request is refused and its streams close

```text
Usage: openside device revoke [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` | Device id from `openside device list` |

### `openside service`

Run opensided as a per-user systemd or launchd service

```text
Usage: openside service [OPTIONS] <COMMAND>
```

#### `openside service unit`

Print the service definition

```text
Usage: openside service unit [OPTIONS]
```

| Argument | Description |
|---|---|
| `--platform <PLATFORM>` | Service manager; default: launchd on macOS, systemd on other Unix. Windows has no default One of `systemd`, `launchd`. |
| `--daemon <PATH>` | Absolute path of opensided; default: next to this openside binary, then PATH |

#### `openside service install`

Write the service definition into the user's service directory

```text
Usage: openside service install [OPTIONS]
```

| Argument | Description |
|---|---|
| `--platform <PLATFORM>` | Service manager; default: launchd on macOS, systemd on other Unix. Windows has no default One of `systemd`, `launchd`. |
| `--daemon <PATH>` | Absolute path of opensided; default: next to this openside binary, then PATH |
| `--dir <DIR>` | Directory to write into instead of the platform's per-user default |

