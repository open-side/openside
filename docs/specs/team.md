# Team agents

The daemon persists the team registry in `agents.db`. Each agent has one durable home conversation. `chief` is the immutable chief identity; its default display name is openside and can be changed. The chief uses the openside mark and the role “Your assistant”, not a separate persona. Existing conversations remain untagged and are available through `GET /v1/sessions?agent=none`. `agent=<id>` selects conversations tagged with that identity.

## Registry and startup

Schema migrations use transactional SQLite `user_version` upgrades. Version 1 creates the registry, handoffs, proposal cooldowns, resumable transfer progress and a metadata-only audit. Version 2 adds the unique channel-account binding index, including retired agents. Registry writes and their audit records commit together. Audit records contain the agent id, operation and timestamp, not persona text, messages or credentials. Future schema versions are refused.

Version 3 adds a nullable proposal label captured from the teammate's role when the person turns a suggestion off. Older rows keep a null label. Clearing a suppression and its metadata-only audit entry commit together; a cleared marker prevents historical tickets from reinstating the old decision.

Version 4 preserves `last_declined_at` separately from the latest topic action, so clearing suppression or approving an explicit fallback cannot erase or restart the seven-day global backstop. Version 5 marks the one-time historical role-key migration.

A separate transactional `agent_migrations` marker renames only a chief still using the previous default name, preserving person-customized names. If another teammate already owns `openside`, startup continues with the previous name, logs a warning, and exposes `agent.chief_name_conflict` in doctor. Renaming that teammate and restarting retries the migration. This marker is independent of the deferred version-5 proposal-key migration.

Startup creates the chief once. The registry stores its chosen home-session id before creating the conversation, so an interrupted bootstrap repairs the same home rather than creating another. The chief can exist before a default model is selected: its initial session model is empty until runtime settings supply one. Startup does not call a model or rewrite earlier conversations. Home conversations cannot be deleted; retire the teammate instead.

## API and authority

| Route | Baseline scope | Behavior |
| --- | --- | --- |
| `GET /v1/agents` | Read | Roster plus home-thread state |
| `POST /v1/agents` | Admin + person device | Create a teammate and its home |
| `GET /v1/agents/{id}` | Read | Full profile |
| `PATCH /v1/agents/{id}` | Steer | Edit profile; permission widening, skills, model, account pool and channel changes additionally require Admin + person device |
| `POST /v1/agents/{id}/retire` | Admin + person device | Retire through the ownership saga; `revert_hire:true` restores the hire's recorded ownership |
| `POST /v1/agents/{id}/restore` | Admin + person device | Restore the archived teammate through the lifecycle saga |
| `POST /v1/agents/{id}/transfer` | Admin + person device | Transfer selected ownership through the resumable saga; retries use a stable request fingerprint |
| `GET /v1/agents/{id}/handoffs` | Read | Handoffs sent or received by this teammate |
| `POST /v1/agents/{id}/handoffs` | Steer | The chief delegates the supplied packet to this teammate |
| `GET /v1/team/proposals/suppressed` | Read | Array of `{key,label,status,decided_at}`; nullable legacy labels fall back to the key |
| `DELETE /v1/team/proposals/{key}` | Admin + person device | Clear a suppressed topic; returns 204, including when already cleared |

Local socket callers satisfy the person-device condition. Profile edits cannot change kind, id or home-session identity. Nullable PATCH overrides distinguish omitted (unchanged) from explicit null (inherit). Create/update validation rejects invalid profiles rather than truncating them.

`POST /v1/sessions` accepts `agent_id` as an immutable tag. Unknown ids return `agent_not_found`; retired agents return `agent_retired`. Tagged channel conversations use the same agent persona and permission ceiling but do not replace its home session. Tagged conversations use the shared browser space. Requested permissions cannot exceed the agent, chief or current daemon default. Null model/account-pool overrides inherit settings; an explicit empty account pool is invalid, and agent account preferences can only restrict the person-configured pool.

`message.queued` and the public `Message` projection carry daemon-owned `author_kind` (`person`, `agent_client`, or `internal`) and `from_agent`. Web/person-device, ordinary local CLI, and paired channel inputs are `person`; API agent-client tokens and MCP inputs are `agent_client`. Runtime/browser notices, routines, Task reports and team deliveries are `internal`; only actual team deliveries name a sender in `from_agent`. Public submit bodies reject both fields. Message ids and text prefixes never grant person authority, and idempotent retries preserve the original author. Historical logs without author metadata replay with `internal` and no sender.

MCP requests use the `x-openside-client: mcp` transport marker to reduce message authority, including over the local socket. It never upgrades a token: an agent-client token without the marker remains `agent_client`. Existing caller and ticket-approval authority are unchanged. Admission to an agent-bound conversation serializes with retirement through the durable queue append, rejects retired owners with `agent_retired`, and releases the registry gate before runtime delivery.

List rows contain `session_status`, `activity`, `needs_you`, `last_seq`, `last_activity` and `last_message_at`. `last_activity` is a bounded latest-message preview; `last_message_at` is the last non-metadata event timestamp for ordering. `last_seq` is the home thread's SSE sequence. Each device computes unread locally from its seen cursor; there are no server read receipts.

## Persona and roster limits

Names have 1–24 Unicode scalar characters and initials 1–3. Roles have 1–60; voices at most 100. Responsibilities and boundaries each have at most six nonempty lines, each at most 100 characters. The rendered persona is capped at 1,500 characters, without truncation. Teammates need at least one responsibility and a specific role rather than a generic assistant/helper role. `[team] max_active_teammates` defaults to `5`, excludes the chief, and applies to creation and restoration under the registry lock; zero disables new hires. The name-and-role prompt is capped at 2,000 characters.

Avatar colors use the eight shipped accent colors: `#2f6fed`, `#a34dff`, `#00a240`, `#f0b429`, `#f97316`, `#ef476f`, `#00a6a6`, `#888888`.

## Channel bindings

An agent may bind one exact `{kind,account_id}`. Profile ownership and the channel account's explicit `agent_id` must agree. A second agent cannot claim that binding, even if its current owner is retired. Retirement never routes new inbound messages to the chief as a fallback. Only unassigned new channel threads default to the chief; existing thread tags never change on reassignment. Deleting the account clears its profile binding, not historical session identity.

Settings writes precede registry lifecycle locks. Profile updates release the registry gate before channel supervisor reconciliation; this prevents inverse-lock deadlocks. Connection sign-in actions use the chief's home conversation rather than a separate Sign-ins conversation.

## Lifecycle integration

Hiring, retirement and ownership transfer use resumable progress records rather than claiming atomicity across the registry, scheduler, memory and event databases. Skills transfer as allowlist grants, never as movement of globally installed skills. Autonomous hiring and approved proposals share the same lifecycle implementation. All delegation flows through the chief; teammates ask the chief for help. Canonical tools are `team_delegate` and `team_ask_chief`, not alternate handoff aliases.

`[team] max_running` is a positive integer, default `3`. The daemon queues additional teammate handoffs durably instead of starting more simultaneous teammate runs. This setting is separate from ephemeral Task concurrency; zero and negative values are rejected.

Lifecycle journals record each individual registry, routine, memory, archive and event step before applying it. Replaying a step accepts its own result but refuses to overwrite a later unrelated owner. The original routine action and memory owner are retained for Undo hire, including chief-owned and previously unowned work. Retirement transfers ownership to the selected successor (the chief by default), optionally pauses transferred routines with `agent_retired`, and archives the retired home. Restoring the profile does not silently reclaim work already handed to the successor.

Retirement signals every active conversation tagged with the agent, including non-home channel threads, while holding the same registry gate that rejects new messages. Priority actor notifications bypass bounded command queues and never wait for a self-retiring actor to acknowledge itself. Each actor aborts its stream, closes its runtime, and cancels its helpers and pending approvals through the normal Stop path.

Open handoffs deduplicate by `dedupe_key`; a conflicting packet with the same open key is rejected. Review handoffs require `subject_handoff_id` and cannot target that subject's original author. Tool execution rechecks the current registry role, even if a stale runtime still lists a tool. Autonomous hiring is restricted to the chief's home and deduplicates by session and tool-call id. Responsibility and retirement tools use the sensitive-action approval path.

Handoffs, questions, answers and reports use stable daemon-owned message identities. An event-driven settle listener persists the outcome before delivering its report, and startup repairs unfinished delivery. Replayed outbox work cannot append a second queue message. There is no team polling loop. Persona edits reopen an idle warm runtime on its next turn; execution intersects live agent, chief and daemon permission ceilings even before that reopen. Skill allowlists filter both the prompt index and execution-time skill loading.

Proposals open a durable `Hire` ticket with its authoritative `HireDraft`. The existing ticket resolve endpoint accepts `{value:"hire",hire?:HireDraft}` to approve with optional edits, or `{value:"not_now"}` / `{value:"never"}` to defer or decline. Every decision requires Approve and the existing person-device/self-approval checks. A missing proposal cannot be approved using card text. One pending proposal is allowed at a time. Not now imposes a 30-day topic cooldown; Don't suggest this suppresses that topic until the person clears it under **Suggestions you turned off** in Team settings.

Admission uses a server-canonical role: NFKC normalization, lowercase, punctuation/whitespace collapsed to spaces, and a leading “a”, “an” or “the” removed. Caller-supplied `proposal_key` does not control admission. On the first proposal admission after upgrade, the version-5 data migration maps existing decisions using roles in historical Hire tickets; unresolved keys remain unchanged. Any Not now or Don't suggest this decision also blocks all unsolicited chief proposals for seven days, so changing the role cannot immediately evade a decline.

Hire tickets project common cards with editable nonsecret name and role fields. Resolved, declined, deferred, rejected and expired states become inactive receipts. Team events carry the same common card projection on their durable event; handoff completion replaces the original handoff card. Explicit person-requested hires carry typed Undo hire and Retire commands, never messages pretending to authorize an undo. Undo remains available while the person retains the hire card; it has no invented clock cutoff. Retire opens the successor/pause form, and both commands retain the lifecycle API's person-device and Admin requirements.

Every hired-teammate event receipt offers Retire, including an approved proposal. Only an explicit person-requested hire additionally offers Undo hire; proposal approval never acquires Undo authority through its receipt.

Public schemas are generated from `openside-proto`; regenerate OpenAPI and web API types together after all team slices have landed.

## CLI

Every team command uses the public agents/team API. Plain-text roster rows include id, name, role, active/retired status, visibility and owned responsibility lines. `--json` emits the canonical API response followed by the existing CLI summary; clearing a proposal returns 204, so only the summary is emitted.

| Command | API | Behavior |
| --- | --- | --- |
| `openside team` / `openside team list` | `GET /v1/agents` | Complete roster, including hidden and retired profiles |
| `openside team show <id\|name>` | `GET /v1/agents`, `GET /v1/agents/{id}` | Read-only inspection; ids take precedence over case-insensitive names |
| `openside team hire --name NAME --role ROLE [--from-session SESSION_ID]` | `POST /v1/agents`; optional session read and event poll | Role also becomes the initial responsibility; omitted overrides inherit daemon defaults |
| `openside team retire ID [--hand-to SUCCESSOR_ID \| --pause]` | `POST /v1/agents/{id}/retire` | Default successor is the chief; `--pause` also pauses transferred routines |
| `openside team undo ID` | `POST /v1/agents/{id}/retire` with `revert_hire:true` | Restore recorded pre-hire owners and retire the teammate; the CLI equivalent of Undo hire |
| `openside team restore ID` | `POST /v1/agents/{id}/restore` | Restore the same home without reclaiming transferred work |
| `openside team rename ID NAME` | `PATCH /v1/agents/{id}` | Change only the name |
| `openside team hide ID` / `openside team unhide ID` | `PATCH /v1/agents/{id}` | Change sidebar visibility only; inspection with `show` never unhides |
| `openside team proposals` | `GET /v1/team/proposals/suppressed` | Suggestions turned off, with the topic key, label, decision and date |
| `openside team proposals allow KEY` | `DELETE /v1/team/proposals/{key}` | Allow that topic to be suggested again |

`--from-session` reads at most the latest 512 durable events at the observed session cursor. It includes only queued-message text and final replies, retaining a recent excerpt bounded to 12,000 UTF-8 bytes. It neither reads private snapshots nor calls a model to summarize, and it does not transfer routines or memories implicitly. The daemon receives the excerpt as `history_summary`, not permanent persona instructions. A missing source fails before hiring.

`openside run GOAL` and its `openside chat GOAL` alias target the chief's existing home conversation unless `--session SESSION_ID` is supplied. Detached output and `--wait`, `--follow`, stdin goal `-` and timeout behavior are unchanged. Session-creation options (`--model`, `--mode`, `--workspace`, `--browser-space`) are rejected for run/chat rather than ignored; use `openside session new` with those options, then run with its id. The bare `openside "task"` form still creates a legacy conversation.

Hire proposals remain ticket interactions: `openside ticket resolve TICKET_ID hire`, `not_now` or `never`. Team commands do not bypass the daemon's person-device, capability or retirement checks.

## Security

The chief may call `team_hire` with `{draft}` without asking first when a recurring responsibility, long-running domain, or independent parallel stream warrants a teammate. It must reuse an existing responsibility owner and use `create_task` for one-off work. After success it sends exactly one short `send_message`, for example “I brought in Mina to handle weekly market notes.” The scripted acceptance test exercises this tool sequence; semantic decisions remain prompt guidance.

The daemon rejects autonomous hires whose normalized responsibility overlaps an active teammate (`responsibility_taken`) and enforces the configured active-teammate cap (`roster_full`). Permission mode remains bounded by both the chief and daemon default, regardless of model-supplied mode. Autonomous teammates inherit the chief's model/account overrides and any skill ceiling. Drafts selecting a channel, account pool or model, or requesting skills beyond that ceiling, require the existing Hire approval flow instead. `team_propose_hire` remains available for authority-expanding proposals; it is not required for ordinary bounded hiring. New connections/accounts and other authority expansion still require their own approvals.

Autonomous hire receipts offer Retire. Historical person-requested receipts retain their typed Undo controls. The person can retire teammates through the existing lifecycle API.

