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
| `--remote <NAME>` | Remote daemon profile saved by `openside connect` (see `openside remote profiles list`) Environment: `OPENSIDE_REMOTE`. |
| `--url <URL>` | Loopback HTTP endpoint (requires a token) |
| `--token <TOKEN>` | Environment: `OPENSIDE_TOKEN`. |
| `--color <WHEN>` | Colour the human output: auto (a terminal that supports it; NO_COLOR and TERM=dumb turn it off), always (also into a pipe; CLICOLOR_FORCE does the same) or never One of `auto`, `always`, `never`. Default: `auto`. |
| `[TASK]` |  |
| `--model <MODEL>` | Model for the new session |
| `--mode <MODE>` | Permission mode for the new session One of `read-only`, `guard`, `full`. |
| `--workspace <PATH>` | Workspace for the new session |
| `--browser-space <BROWSER_SPACE>` | Browser space for the new session: shared (default, the managed profile with its logins) or isolated (own cookies and storage, wiped when the session is archived or deleted) One of `shared`, `isolated`. |

`--socket`, `--url`, `--token` and `--json` are global and apply to every subcommand.

### `openside accounts`

Show credential-source accounts, configured models, usage and rate-limit blocks

```text
Usage: openside accounts [OPTIONS] [COMMAND]
```

#### `openside accounts add`

Create an isolated CLI home and print the external sign-in command

```text
Usage: openside accounts add [OPTIONS] --label <LABEL> <SOURCE>
```

| Argument | Description |
|---|---|
| `<SOURCE>` |  |
| `--label <LABEL>` | Required. |

#### `openside accounts show`

Show login readiness and the exact external sign-in command for this account

```text
Usage: openside accounts show [OPTIONS] <SOURCE> <ACCOUNT_ID>
```

| Argument | Description |
|---|---|
| `<SOURCE>` |  |
| `<ACCOUNT_ID>` |  |

#### `openside accounts enable`

Enable one existing subscription account

```text
Usage: openside accounts enable [OPTIONS] <SOURCE> <ACCOUNT_ID>
```

| Argument | Description |
|---|---|
| `<SOURCE>` |  |
| `<ACCOUNT_ID>` |  |

#### `openside accounts disable`

Disable one existing subscription account

```text
Usage: openside accounts disable [OPTIONS] <SOURCE> <ACCOUNT_ID>
```

| Argument | Description |
|---|---|
| `<SOURCE>` |  |
| `<ACCOUNT_ID>` |  |

#### `openside accounts rm`

Remove an openside account; never delete the default user CLI home

```text
Usage: openside accounts rm [OPTIONS] <SOURCE> <ACCOUNT_ID>
```

| Argument | Description |
|---|---|
| `<SOURCE>` |  |
| `<ACCOUNT_ID>` |  |

### `openside models`

List models grouped by provider, with credential readiness and source

```text
Usage: openside models [OPTIONS] [COMMAND]
```

#### `openside models set`

Set default/fallback selection; Team homes update on their next turn

```text
Usage: openside models set [OPTIONS] <MODEL>
```

| Argument | Description |
|---|---|
| `<MODEL>` |  |
| `--fallback <FALLBACKS>` |  |

#### `openside models pool`

List or replace explicit ordered provider account pools

```text
Usage: openside models pool [OPTIONS] <COMMAND>
```

##### `openside models pool list`

List provider pools without credential values

```text
Usage: openside models pool list [OPTIONS]
```

##### `openside models pool set`

Replace membership with source:account-id references; empty disables routing

```text
Usage: openside models pool set [OPTIONS] <PROVIDER> [ACCOUNTS]...
```

| Argument | Description |
|---|---|
| `<PROVIDER>` |  |
| `[ACCOUNTS]` |  |

#### `openside models account`

Prefer an authorized account for subsequent requests; omit account to clear

```text
Usage: openside models account [OPTIONS] <SESSION> [ACCOUNT]
```

| Argument | Description |
|---|---|
| `<SESSION>` |  |
| `[ACCOUNT]` |  |

### `openside key`

Manage encrypted provider API keys (set reads from stdin, never argv)

```text
Usage: openside key [OPTIONS] <COMMAND>
```

#### `openside key list`

List labelled provider accounts, masked suffixes and enabled state, never values

```text
Usage: openside key list [OPTIONS]
```

#### `openside key set`

Create a labelled account or replace --key-id; reads a hidden stdin line only

```text
Usage: openside key set [OPTIONS] <PROVIDER>
```

| Argument | Description |
|---|---|
| `<PROVIDER>` |  |
| `--label <LABEL>` |  |
| `--key-id <KEY_ID>` |  |

#### `openside key update`

Change account metadata without reading or replacing the key

```text
Usage: openside key update [OPTIONS] <PROVIDER> <KEY_ID>
```

| Argument | Description |
|---|---|
| `<PROVIDER>` |  |
| `<KEY_ID>` |  |
| `--label <LABEL>` |  |
| `--enabled <ENABLED>` | One of `true`, `false`. |
| `--reserve-percent <RESERVE_PERCENT>` |  |

#### `openside key rm`

Remove only this provider account

```text
Usage: openside key rm [OPTIONS] <PROVIDER> <KEY_ID>
```

| Argument | Description |
|---|---|
| `<PROVIDER>` |  |
| `<KEY_ID>` |  |

### `openside channels`

Manage independent chat channel accounts; secrets use hidden stdin only

```text
Usage: openside channels [OPTIONS] <COMMAND>
```

#### `openside channels list`

List account labels and effective status, never credentials

```text
Usage: openside channels list [OPTIONS]
```

#### `openside channels add`

Add a channel identity; credentials are read from hidden stdin only

```text
Usage: openside channels add [OPTIONS] --label <LABEL> <KIND> <ID>
```

| Argument | Description |
|---|---|
| `<KIND>` |  |
| `<ID>` |  |
| `--label <LABEL>` | Required. |
| `--enabled` |  |
| `--agent <AGENT>` |  |
| `--api-base <API_BASE>` |  |
| `--listen <LISTEN>` |  |
| `--public-key <PUBLIC_KEY>` |  |
| `--allow-chat <ALLOWED_CHATS>` |  |
| `--allow-user <ALLOWED_USERS>` |  |
| `--allow-workspace <ALLOWED_WORKSPACES>` |  |
| `--allow-channel <ALLOWED_CHANNELS>` |  |
| `--allow-guild <ALLOWED_GUILDS>` |  |

#### `openside channels rm`

Remove only this account, stopping its adapter and deleting its Vault credentials

```text
Usage: openside channels rm [OPTIONS] <KIND> <ID>
```

| Argument | Description |
|---|---|
| `<KIND>` |  |
| `<ID>` |  |

#### `openside channels test`

Perform a read-only connectivity check, without sending a chat message

```text
Usage: openside channels test [OPTIONS] <KIND> <ID>
```

| Argument | Description |
|---|---|
| `<KIND>` |  |
| `<ID>` |  |

### `openside team`

Manage the chief and durable specialist teammates

```text
Usage: openside team [OPTIONS] [COMMAND]
```

#### `openside team list`

List the roster, including hidden and retired teammates

```text
Usage: openside team list [OPTIONS]
```

#### `openside team show`

Inspect a profile by its id or case-insensitive name; does not change visibility

```text
Usage: openside team show [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside team hire`

Hire a specialist; the role is also their initial responsibility

```text
Usage: openside team hire [OPTIONS] --name <NAME> --role <ROLE>
```

| Argument | Description |
|---|---|
| `--name <NAME>` | Required. |
| `--role <ROLE>` | Required. |
| `--from-session <SESSION_ID>` | Include a bounded recent conversation excerpt as onboarding context |

#### `openside team retire`

Retire a teammate and hand their work to the chief or a selected successor

```text
Usage: openside team retire [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |
| `--hand-to <ID>` |  |
| `--pause` | Pause owned routines while handing ownership to the chief |

#### `openside team undo`

Undo a hire, restoring its recorded ownership and retiring the teammate

```text
Usage: openside team undo [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside team restore`

Restore a retired teammate without reclaiming work already handed over

```text
Usage: openside team restore [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside team rename`

Rename a profile without changing its identity or home conversation

```text
Usage: openside team rename [OPTIONS] <ID> <NAME>
```

| Argument | Description |
|---|---|
| `<ID>` |  |
| `<NAME>` |  |

#### `openside team hide`

Hide a profile from the sidebar without pausing work

```text
Usage: openside team hide [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside team unhide`

Show a hidden profile in the sidebar again

```text
Usage: openside team unhide [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside team proposals`

List or clear topics whose hire suggestions you turned off

```text
Usage: openside team proposals [OPTIONS] [COMMAND]
```

##### `openside team proposals allow`

Allow the selected topic to be suggested again

```text
Usage: openside team proposals allow [OPTIONS] <KEY>
```

| Argument | Description |
|---|---|
| `<KEY>` |  |

### `openside session`

```text
Usage: openside session [OPTIONS] <COMMAND>
```

#### `openside session list`

```text
Usage: openside session list [OPTIONS]
```

#### `openside session new`

Create an empty session to drive with `browser` or `repl`; prints its id

```text
Usage: openside session new [OPTIONS]
```

| Argument | Description |
|---|---|
| `--title <TITLE>` |  |

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
| `[VALUE]` | JSON or text; secret, login, OTP and payment values only through a hidden prompt or '-'; form/login/choice stdin uses one line per field |
| `--totp` | With '-' or the prompt: the input is the site's authenticator (otpauth:// URI or base32 setup key) to save in the vault instead of a code; request_otp tickets that offer registration only |
| `--save` | Login tickets: save the login to the vault (the default when the site allows it; needs the approve scope) |
| `--no-save` | Login tickets: log in without saving to the vault |
| `--alternative <ID>` | Login tickets: use the page's other sign-in option ID (see `ticket show`) instead of typing |
| `--account <ITEM_ID>` | Login tickets: log in with this saved vault account (see `ticket show`) |

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
| `--person` | Issue a person's web/phone/extension token, not an agent-client token. Run this on the daemon host; never give a person token to an agent |

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

### `openside connections`

Manage browser sign-ins, per-agent grants and audit records (admin)

```text
Usage: openside connections [OPTIONS] <COMMAND>
```

#### `openside connections list`

List sign-ins and separately marked candidates (metadata only)

```text
Usage: openside connections list [OPTIONS]
```

| Argument | Description |
|---|---|
| `--site <SITE>` | Exact registrable site domain, filtered without changing the API object shape |

#### `openside connections show`

Show a connection and its explicit per-agent grants

```text
Usage: openside connections show [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside connections check`

Check whether the browser is still signed in

```text
Usage: openside connections check [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside connections relogin`

Sign in again, using the Vault or an existing human-step ticket

```text
Usage: openside connections relogin [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |

#### `openside connections revoke`

Remove access and sign out every shared-profile account on the site

```text
Usage: openside connections revoke [OPTIONS] <ID>
```

| Argument | Description |
|---|---|
| `<ID>` |  |
| `--keep-signed-in` | Forget metadata and grants without clearing the shared browser's sign-in |

#### `openside connections grant`

Set a stable agent ID's connection access; ordinary action policy still applies

```text
Usage: openside connections grant [OPTIONS] <ID> <AGENT> <ACCESS>
```

| Argument | Description |
|---|---|
| `<ID>` |  |
| `<AGENT>` |  |
| `<ACCESS>` | One of `allowed`, `ask`, `denied`. |

#### `openside connections audit`

Show newest-first metadata audit records

```text
Usage: openside connections audit [OPTIONS]
```

| Argument | Description |
|---|---|
| `--connection <CONNECTION>` |  |
| `--agent <AGENT>` |  |
| `--action <ACTION>` | One of `detected`, `verified`, `expired`, `needs_login`, `relogin`, `open`, `fetch`, `grant`, `forget`, `sign_out`. |

#### `openside connections spaces`

Manage opt-in separate account profiles

```text
Usage: openside connections spaces [OPTIONS] <COMMAND>
```

##### `openside connections spaces add`

Create a separate account profile and start its sign-in flow

```text
Usage: openside connections spaces add [OPTIONS] <SITE>
```

| Argument | Description |
|---|---|
| `<SITE>` |  |

##### `openside connections spaces remove`

Stop a separate account profile and delete its sign-in and profile directory

```text
Usage: openside connections spaces remove [OPTIONS] <SPACE_ID>
```

| Argument | Description |
|---|---|
| `<SPACE_ID>` |  |

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

### `openside run`

Delegate a goal to the openside browser agent; prints the session and message ids

```text
Usage: openside run [OPTIONS] <GOAL>
```

| Argument | Description |
|---|---|
| `<GOAL>` | What the agent should do; `-` reads it from stdin |
| `--session <SESSION_ID>` | Run in this session instead of the chief's home conversation |
| `--wait` | Wait until the run ends (through any tickets a person answers) and print the replies |
| `--follow` | Stream the run's text, progress and tickets until the session settles |
| `--timeout <SECS>` | With --wait: give up after this many seconds (status `timeout`, exit 1) |

### `openside steer`

Steer a running agent, or one of its Tasks with --task

```text
Usage: openside steer [OPTIONS] <SESSION> <TEXT>
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session to steer |
| `<TEXT>` | The guidance to send |
| `--task <TASK_ID>` | Steer this Task instead of the session's agent (ids from `openside tasks`) |

### `openside cancel`

Cancel one Task with --task, otherwise stop the session's run

```text
Usage: openside cancel [OPTIONS] <SESSION>
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose run or Task to cancel |
| `--task <TASK_ID>` | Cancel only this Task; the agent and the other Tasks keep running |

### `openside tasks`

List a session's Tasks and subagents with their status and results

```text
Usage: openside tasks [OPTIONS] <SESSION>
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session to list |

### `openside browser`

Call the session's browser and human-step tools directly, like the in-app agent

```text
Usage: openside browser [OPTIONS] <COMMAND>
```

#### `openside browser tools`

List the directly callable tools with their argument schemas (GET /v1/tools)

```text
Usage: openside browser tools [OPTIONS]
```

#### `openside browser tabs`

browser_tabs: list the session's tabs

```text
Usage: openside browser tabs [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser open`

browser_open_tab: open a tab (url=...)

```text
Usage: openside browser open [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser navigate`

browser_navigate: go to a URL in a tab (tab_id=... url=...)

```text
Usage: openside browser navigate [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser snapshot`

browser_snapshot: accessibility snapshot with eN refs (tab_id=... [scope=viewport])

```text
Usage: openside browser snapshot [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser act`

browser_act: act on a snapshot ref (tab_id=... target=eN action=click; see `tools`)

```text
Usage: openside browser act [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser pointer`

browser_pointer: click, type or scroll at viewport coordinates

```text
Usage: openside browser pointer [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser screenshot`

browser_screenshot: observe a tab; output=true delivers it (`--out FILE` saves and delivers it)

```text
Usage: openside browser screenshot [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser close`

browser_close_tab: close a tab (tab_id=...)

```text
Usage: openside browser close [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser login`

request_login: sign in on the tab's login page; a person answers on their device

```text
Usage: openside browser login [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser choice`

request_choice: let a person pick among the page's options

```text
Usage: openside browser choice [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser wait-approval`

wait_for_approval: wait while a person approves the sign-in on their phone

```text
Usage: openside browser wait-approval [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser otp`

request_otp: fill a one-time code from the vault or from a person

```text
Usage: openside browser otp [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser register-authenticator`

register_authenticator: keep a site's 2FA setup key in the vault, with approval

```text
Usage: openside browser register-authenticator [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser human-step`

request_human_step: hand a step to a person (takeover, device approval, secret input)

```text
Usage: openside browser human-step [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser ask`

ask_user: ask a person a non-secret question

```text
Usage: openside browser ask [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser vault-login`

vault_login: fill a saved login (needs the vault_use scope over TCP)

```text
Usage: openside browser vault-login [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser vault-otp`

vault_otp: fill a saved TOTP code (needs the vault_use scope over TCP)

```text
Usage: openside browser vault-otp [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

#### `openside browser vault-passkey`

vault_passkey: register or use a vault passkey (needs the vault_use scope over TCP)

```text
Usage: openside browser vault-passkey [OPTIONS] <SESSION> [ARG]...
```

| Argument | Description |
|---|---|
| `<SESSION>` | Session whose tabs the tool works in |
| `[ARG]` | Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or `tab_id=T1 scope=viewport` |
| `--approval-ticket <TICKET_ID>` | Retry under this approved ticket (normally done automatically while waiting) |
| `--no-wait` | Return a pending ticket at once instead of waiting for the person's answer |
| `--timeout <SECS>` | Stop waiting for a person after this many seconds |
| `--out <FILE>` | Write the PNG the call saved (a screenshot) to this file |

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
| `--scope <SCOPE>` | Scope the new device gets; repeat for several. Default: read, approve, human_step for a person; read, steer, browse, approve with --agent |
| `--name <NAME>` | Label shown in `openside device list` Default: `phone`. |
| `--via <TRANSPORT>` | Transport the QR link uses: tailscale, cloudflare or relay. Default: the first enabled `remote.mode` One of `tailscale`, `cloudflare`, `relay`. |
| `--agent` | Pair an external CLI/MCP agent instead of a person's phone/browser. Its token cannot approve confirmations for sessions it drives |

### `openside connect`

Pair this computer with a remote daemon from its pairing link and save it as a `--remote` profile

```text
Usage: openside connect [OPTIONS] [LINK]
```

| Argument | Description |
|---|---|
| `[LINK]` | Pairing link; omit to read from stdin or a hidden prompt (recommended) |
| `--name <NAME>` | Profile name used with `--remote NAME`; default: the host name's first label |
| `--force` | Replace an existing profile with the same name |

### `openside remote`

This daemon's remote access (Tailscale, Cloudflare Tunnel or a relay) and the saved profiles for reaching other daemons

```text
Usage: openside remote [OPTIONS] <COMMAND>
```

#### `openside remote setup`

Enable a transport (or `off`) in config.toml and check it

```text
Usage: openside remote setup [OPTIONS] <MODE>
```

| Argument | Description |
|---|---|
| `<MODE>` | One of `tailscale`, `cloudflare`, `relay`, `off`. |
| `--config <PATH>` | Configuration file to edit; default: the discovered config.toml |
| `--apply` | tailscale: also run `tailscale serve` (otherwise the command is printed) |
| `--named` | cloudflare: use a named tunnel instead of a quick tunnel |
| `--hostname <HOST>` | cloudflare --named: the public hostname routed to the tunnel |
| `--token-file <PATH>` | cloudflare --named: file with the tunnel token; relay: file with the registration token |
| `--relay-url <URL>` | relay: the relay's https:// origin |

#### `openside remote status`

Transports the running daemon has up, with their public URLs

```text
Usage: openside remote status [OPTIONS]
```

#### `openside remote local-access`

Bootstrap the browser on this computer with local-person authority (local socket only)

```text
Usage: openside remote local-access [OPTIONS]
```

#### `openside remote profiles`

Saved profiles for reaching other daemons with `--remote NAME` (add one with `openside connect`)

```text
Usage: openside remote profiles [OPTIONS] <COMMAND>
```

##### `openside remote profiles list`

Saved remote daemons (tokens are never shown)

```text
Usage: openside remote profiles list [OPTIONS]
```

##### `openside remote profiles rm`

Forget a remote and delete its stored token. The daemon keeps the device until you run `openside device revoke <id>` there

```text
Usage: openside remote profiles rm [OPTIONS] <NAME>
```

| Argument | Description |
|---|---|
| `<NAME>` | Profile name |

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

