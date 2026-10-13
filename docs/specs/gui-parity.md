# GUI parity: what a person can do without a terminal

Rule: nearly every feature is usable from the web UI (and the desktop launcher that opens it). The one exception is the **first sign-in of the Claude Code and Codex command-line tools**, which is a vendor flow: the Models screen shows the exact command with a copy button. Nothing else on screen tells a person to run a command as *the* way to do something. A command may stay as a muted "Or from a terminal: `cmd`" hint (`web/src/pages/CliHint.tsx`) next to a working GUI path.

Status: **done** = a GUI path exists and the command is secondary or gone; **exception** = intentionally terminal or vendor-side, explained in the row.

## Operator screens (Workspace)

| Feature | Screen | Daemon routes | Notes |
|---|---|---|---|
| Diagnostics (`openside doctor`) | Workspace → Diagnostics (`/diagnostics`) | `GET /v1/doctor` | Checks are grouped; every check that has a fix shows one button. `fix.action` maps to: `open_models` → Models, `open_vault` → Vault, `open_remote` → Remote access, `open_configuration` → Configuration, `open_agents` → Agents, `open_policy` → Policy, `restart_daemon` → confirm, then restart, `download_memory_model` → starts the download and opens Memory. |
| Memory search model (`openside memory model fetch`) | Memory → Search model card | `GET /v1/memory/model`, `POST /v1/memory/model/download`, `POST /v1/memory/model/cancel` | Download, byte and file progress, cancel, plain-language failures (`egress_refused` points to Policy → Network), then a Restart button when the model needs a restart to take effect. |
| Restart the daemon | `RestartNotice` wherever a change needs it (Configuration, Remote access, Memory, Policy → Network, Diagnostics) | `POST /v1/daemon/restart`, `GET /v1/daemon` | Needs local authority. After the request the page polls `GET /v1/daemon` until `boot_id` changes, riding out the gap, and gives up after 90 seconds. |
| Edit configuration | Workspace → Configuration | `GET/PUT /v1/settings/config` | A device without local authority can change only the keys the daemon lists in `remote_editable`; every other key is marked "local only" with an explanation of why and how to change it (open openside on that computer). |
| Skills folder for an agent that is not a preset | Workspace → Agents, and the onboarding Agents step | `PUT/DELETE /v1/external-agents/{id}` | Add a name and a folder (absolute or `~/…`, no `..`, inside home, config or data). Presets cannot be removed. Install the browser skill into any listed agent. |
| Sync (`openside sync …`) | Workspace → Sync | `GET /v1/sync`, `PUT /v1/sync/config`, `POST /v1/sync/setup`, `/pair`, `/unlock`, `/run` | Folder or WebDAV backend, start a new account or join with a code, unlock, "Sync now" with a plain-words report, what to sync (policy, memory tags). Secrets are read from the form at submit time, cleared from the inputs, and never kept in component state. |
| Pinned tabs | Workspace → Sync | `GET/POST/DELETE /v1/pinned-tabs` | List, pin an address, unpin. |
| Live policy (`openside policy …`) | Workspace → Policy | `GET/PUT /v1/policy`, `POST /v1/policy/test`, `POST /v1/policy/validate`, `GET/PUT /v1/policy/network` | Default mode, allowed and denied sites, rules, network allow-list, and "Try an action". Save validates first and marks problems per row. Network changes show a Restart notice. |

## Original instruction-by-instruction table

| String (before) | File (after the change) | GUI replacement | Status |
|---|---|---|---|
| "No token yet? Run `openside pair` ..." | `web/src/components/TokenEntry.tsx` | "On that computer open openside → Settings → Remote access → Connect a phone, then scan the QR code"; command is a `CliHint` | done |
| "Run `openside pair --via relay` ..." | `web/src/components/TokenEntry.tsx` | Same wording; command is a `CliHint` | done |
| "Run openside pair --via relay again." | `web/src/pages/PairPage.tsx` | "Make a new QR code in openside on the computer (Settings → Remote access → Connect a phone)" | done |
| "Scan a new QR code from openside pair." (three places) | `web/src/pages/PairPage.tsx` | "…from openside on the computer." | done |
| "This pairing link has no code. Run openside pair ..." | `web/src/pages/PairPage.tsx` | Settings → Remote access → Connect a phone | done |
| `openside pair [--agent] --name` (Devices) | `web/src/pages/DevicesPage.tsx` | "Connect a phone with a QR code" is primary; command is a `CliHint` | done |
| `openside pair --agent`, `openside connect`, `openside --remote <name> mcp` (Devices, agent) | `web/src/pages/AgentLink.tsx`, `DevicesPage.tsx` | "Create a link" issues an `agent_client` pairing offer with local authority, with a name field, copy and countdown. The only command left is the agent-side `openside connect`, a `CliHint` | done |
| `--scope steer` / `--scope admin` explanation | `web/src/pages/DevicesPage.tsx` (removed) | Scopes are chosen with checkboxes in Connect a phone | done |
| "Run `openside doctor` on the computer" (Vault unavailable) | `web/src/pages/VaultPage.tsx` | Diagnostics screen and an "Open Configuration" link; `openside doctor` is a `CliHint` | done |
| `vault.unlock`, `vault.passphrase_cmd` wording | `web/src/pages/VaultPage.tsx` | "Choose the passphrase unlock method in Configuration" | done |
| "This device needs the admin scope ..." (generic dead end) | `web/src/pages/ScopeNotice.tsx` and callers | "Ask for access" → access request; the computer shows an Allow / Decline card; the asking device unlocks itself | done |
| Approving an access request | `web/src/access/AccessRequestDock.tsx`, `crates/opensided/src/access_requests.rs` | Allow / Decline card on the computer; Allow needs local authority and is audited | done |
| `pairing_expired` / `pairing_not_found`: "Create a new one with `openside pair`" | `web/src/api/client.ts` | "Make a new one in openside on your computer (Settings → Remote access)" | done |
| `vault_unsupported`: "Run `openside doctor` there" | `web/src/api/client.ts` | "Choose another unlock method under Configuration → vault" | done |
| `skills_dir_unknown`: "Set it under [skills.agents] in config.toml" | `web/src/api/client.ts`, `web/src/pages/AgentFolderDialog.tsx` | "Add its skills folder in Agents": the folder dialog | done |
| "An agent that is not listed can be set up with `openside skills install <agent>`" | `web/src/onboarding/AgentsStep.tsx` | Same Agents list as Workspace → Agents, with Add agent | done |
| `keyring_password_required`: "run openside import with --keyring-password-cmd" | `web/src/api/client.ts` | "Unlock the keyring on the computer and try again, or leave cookies out", plus the "Leave cookies out" button | done |
| "Import ... run `openside import`" | `web/src/onboarding/ImportStep.tsx` | Whole flow in the GUI: detect, preview counts, import, progress, summary | done |
| Vault setup link in the import step | `web/src/onboarding/VaultGate.tsx` | Inline setup or unlock form, passphrase or keyring | done |
| Pair step command fallback | `web/src/onboarding/PairStep.tsx` | QR flow; a browser that is not the computer gets `LocalOnlyNotice` only | done |
| Extension: "Run `openside token create --person --scope read ...`" | `extension/static/options.html` | "Pairing link" field → `exchangePairing`; the token form moves into a closed "Use a device token instead" disclosure with the command as a hint | done (direct links; relay links need the web app) |
| Extension panel: "Connect ... with a device token." | `extension/static/panel.html` | "…with a pairing link from Connect a phone." | done |
| `local_access_required`: "Reopen openside from the computer it runs on" | `web/src/api/client.ts` | The desktop launcher is the GUI path; no shell command shown | done |
| Memory `next_step`: "Run `openside memory model fetch <repo>`" | `web/src/pages/MemoryPage.tsx` | Search model card (see above); the server text is no longer shown | done |
| Configuration "only in config.toml" for a remote device | `web/src/pages/ConfigPage.tsx` | Local-only keys explained and marked; remote-editable keys saved from the page | done |
| Restart the daemon after a config change ("restart openside") | `web/src/pages/RestartNotice.tsx` and callers | Restart daemon button with confirmation and automatic reconnect | done |
| Device sync, live policy, pinned tabs (`UNSURFACED`) | `web/src/pages/SyncPage.tsx`, `PolicyPage.tsx` | See the screens table | done |

## Command by command

| Command | Where in the GUI |
|---|---|
| `accounts`, `models`, `key` | Models & accounts (API keys, subscription accounts, pools, default and fallback models) |
| `channels` | Settings → Channels (Telegram, Slack and Discord accounts: add, edit, reconnect, test, turn on or off, remove, teammate, callback address) |
| `session`, `ticket`, `run`, `steer`, `cancel`, `tasks` | Conversations, the ticket cards in a chat, the Tasks panel, stop and steer controls |
| `routine` | Workspace → Routines |
| `memory` | Workspace → Memory (notes, search, retention, search model) |
| `token`, `pair`, `connect`, `device` | Settings → Remote access → Connect a phone, Workspace → Devices (including agent links) |
| `vault` | Workspace → Vault (unlock, per-item policy, audit) |
| `sync` | Workspace → Sync |
| `policy` | Workspace → Policy |
| `skills` | Workspace → Skills, and Agents for installing the browser skill |
| `import` | First-run guide, Import step |
| `remote` | Settings → Remote access |
| `doctor` | Workspace → Diagnostics |
| `browser`, `repl`, `mcp` | Agent-side tools. The in-app agent calls the same tools; the person watches and takes over in the browser panel. No person-facing CLI step exists, so there is nothing to surface |
| `service` | **Exception, by nature.** Writes a systemd or launchd file for people who run the daemon headless. The desktop launcher starts the daemon on demand, so a person using the GUI never needs it; see `docs/INSTALL.md` |

## What stays terminal-only, and why

| Flow | Why | What the screen shows |
|---|---|---|
| First sign-in of Claude Code and Codex | A vendor login that happens in the vendor's own tool. openside cannot sign in for it | Models → account dialog shows the exact command with a copy button and waits for the sign-in to appear. API keys remain a fully in-app path |
| Agent-side `openside connect` | The agent is a command-line program; there is no GUI on that side | A copyable hint next to "Create a link" |
| `openside remote local-access --json` paste | A fallback for a browser opened without the desktop launcher | A closed disclosure inside `LocalOnlyNotice`; the primary text is "reopen openside from this computer" |
| Extension token form | Kept for direct token use | Closed disclosure; the "Pairing link" field is the primary path |
