# openside-agent

The model runtime of openside: a Bun/TypeScript sidecar built on omp's
`@oh-my-pi/pi-agent-core` and `@oh-my-pi/pi-ai` (pinned to 18.16.0 in
`package.json` and `bun.lock`).

## Architecture

The Rust daemon owns sessions, security policy, the vault, browser control, tickets and
tool execution. The sidecar runs the model loop and calls daemon tools through a
reverse-RPC proxy; it has no tools of its own. The wire contract is
[`sidecar-protocol.md`](../docs/specs/sidecar-protocol.md).

| Path | Role |
| --- | --- |
| `src/main.ts` | Entry point: sidecar mode (default), `doctor --json`, `accounts --json`, `models --json` |
| `src/protocol/` | Wire types, inbound schema validation (`validate.ts`), version info |
| `src/transport/` | 4 MiB JSONL over the daemon's Unix socket, nonce `hello`, reverse-RPC correlation |
| `src/runtime/session.ts` | Session state machine: open, runs, steer, abort, resolve, snapshots, titles |
| `src/runtime/hooks.ts` | `SessionAgent`: one agent's runs over pi-agent-core `agentLoop`/`agentLoopContinue` |
| `src/runtime/{tools,deferred,state,compaction,subagents,team}.ts` | Tool proxies, deferred calls, `openside-agent-state/2` snapshots, history compaction, Task/child agents |
| `src/providers/` | Model resolution, daemon credential leases, request auth, outcome classification, egress guard, catalog and diagnostics |
| `src/capabilities/` | Secret redaction, system reminders, prompt-injection defense, cache-bust warning (no tools) |
| `src/features/` | Pure modules: skills, citations, research mode, prompt epochs, memory, browse policy |
| `src/scripted.ts` | Scripted model for tests (`OPENSIDE_AGENT_SCRIPT`, protocol §9) |

## Install and run

Bun 1.4.4 or newer is required (`engines.bun`). From this directory:

```bash
bun install --frozen-lockfile                # development
bun install --frozen-lockfile --production   # release tree
```

The install includes the platform's `@oh-my-pi/pi-natives-<os>-<arch>` native addon
(about 245 MB on linux-x64), which pi-agent-core loads at import time.

The daemon normally starts the sidecar itself as `bun run <agent dir>/src/main.ts` and
injects `OPENSIDE_SIDECAR_SOCKET` and `OPENSIDE_SIDECAR_NONCE`. Manual start:

```bash
bun run src/main.ts
```

Diagnostics read their configuration JSON from `OPENSIDE_DOCTOR_CONFIG`:

```bash
OPENSIDE_DOCTOR_CONFIG='{"credentials":{},"models":{}}' bun run src/main.ts doctor --json
bun run src/main.ts models --json
```

`doctor --json` reports `runtime` (`bun <version>`), the `@oh-my-pi/*` and
`openside-agent` package versions, credential diagnostics and the model settings.

## Provider credentials

The daemon owns account selection, explicit pool trust, sticky affinity, usage and block
state, and managed token refresh. For every outbound request attempt the sidecar obtains
exactly one private access credential and reports only a sanitized outcome; it never reads
credential homes, broker snapshots or refresh tokens. Managed local OAuth state lives in the
encrypted vault and private CLI homes; the default user home stays read-only. See
[`models-accounts.md`](../docs/specs/models-accounts.md).

### Request credentials

`src/providers/stream.ts` is the custom pi-agent-core `StreamFn` that owns the lease
loop: `credential_request` → pi-ai `streamSimple` → `credential_report`.
`src/providers/credential-rpc.ts` multiplexes the typed private replies and
`src/providers/request-auth.ts` strips every credential header pi-ai built and injects
the lease only for the selected provider origin. pi-ai's own `ApiKeyResolver` is not used:
it only runs on authentication failures, while the daemon needs every outcome.

The exchange is specified in [`sidecar-protocol.md` §6.1](../docs/specs/sidecar-protocol.md).
Attempts are numbered from 0 to 63. Outcomes are `success`, `auth`, `rate_limit`, `quota`,
`policy`, `transient`, `cancelled`; the daemon answers `complete`, `refresh`, `rotate` or
`stop`. A failure acknowledgement must complete before a credential is requested again. A
request is never replayed once any event reached the agent, and a cancellation report is
sent without waiting, so shutdown never waits on a stopped dispatcher. Provider-internal
transient retries stay enabled within `maxRetryDelayMs`; only their final outcome is
reported.

The default policy fails closed and never downgrades to an unlisted source. G-2 allows
switching between sources only inside a provider pool a person configured explicitly.
Explicit model fallback is separate, and `AuthUnavailable` never triggers it.

### Provider authentication

pi-ai receives the leased token as a plain string, so it reads no credential files and
runs no OAuth refresh of its own. Claude Code request shaping applies only to `claude_code`
leases, never to Anthropic API keys: pi-ai applies the Claude Code fingerprint in OAuth
mode, and `src/providers/anthropic.ts` selects that mode from the daemon's auth kind and
owns the version override (`OPENSIDE_CLAUDE_CODE_VERSION` or
`credentials.anthropic.claude_code_version`). Every model request goes through the egress
guard in `src/providers/egress.ts`: only loopback, the called provider's origin and
`network.allow` entries are reachable, and each redirect hop is checked before it connects.
Every resolved address is checked too: a name reached only through a wildcard entry must
resolve to a public address, while an exact entry (or admitted endpoint) may resolve to an
internal one only for the scheme and port it names, so `https://lan.example` never vouches
for `http://lan.example`.

Google Cloud Code accounts (`google-gemini-cli`, `google-antigravity`) need their project
on every request. The daemon sends it as the lease's private `auth.request_metadata.project_id`
(1–256 chars of `[A-Za-z0-9._:-]`); the sidecar hands pi-ai the structured key
`{"token":"<placeholder>","projectId":"…"}` so the request body carries the project, and the
lease bearer replaces the placeholder per request. A Cloud Code lease without a project is
reported and refused with `AuthUnavailable` before any request is sent. The project is
cleared with the token when the attempt ends. Antigravity requests use pi-ai's pinned client
version (`PI_AI_ANTIGRAVITY_VERSION` is defaulted unless the environment sets it) so pi-ai
never fetches its version manifest from a non-provider origin.

Consumed JSONL line buffers are zeroed (`src/transport/jsonl.ts`), and a lease token's only
sidecar reference is dropped when its attempt ends (`src/providers/secret.ts`).
JavaScript strings, JSON decoding and the HTTP stack keep immutable copies that cannot be
wiped, so process memory is not guaranteed to be erased. Tokens, raw provider response
bodies and exception text never reach reports, state snapshots, public diagnostics or logs.

### Credential-free diagnostics

`models --json` returns only the curated model index
`{providers:[{provider,models:[{id,label}]}]}` from `src/providers/catalog.ts`. Rust merges
configured and account-reported models and adds availability, pools and usage. `accounts`
and `doctor` may use the daemon's sanitized `credentials._diagnostics.accounts` metadata;
standalone, they detect only whether a CLI is installed and leave login readiness unknown.
They never read CLI auth files, broker snapshots, environment key values or credential
command output.

## Structured results with `present`

`present` is exposed only through the daemon-supplied catalog and the existing reverse-RPC
proxy; the sidecar adds no card schema, ToolCall variant or wire version. The runtime
instructions (`PRESENT_INSTRUCTIONS` in `src/runtime/hooks.ts`) apply to single, front,
chief, teammate and delegated agents whenever the allowed catalog contains `present`;
existing team role, persona and roster instructions stay unchanged.

Use bounded, non-interactive cards for product or place lists, comparison tables,
order/payment summaries, form summaries and connection intents, following the catalog
schema and the limits in [`cards.md`](../docs/specs/cards.md). The tool returns only
`{card_id}`; the daemon shows the card. Do not repeat the whole card or a Markdown table;
give safe context in a short bubble. For example, present a comparison as a `table` with
text and money cells, then say "Here is the option I found."; present an order as an
`order_summary` with `kv_rows` and integer money minor units, then say "Review the total; no
order has been placed." The runnable comparison example in the instructions is checked
against the generated daemon catalog in `tests/present.test.ts`.

Login, OTP and page-form input go through `request_login`, `request_otp` and `request_form`,
never improvised interactive cards. Credential and payment field values go directly to the
daemon's verified page, never into `present` or text; secure outcomes carry only status
metadata. Daemon rejections reach the model as ordinary retries, and correcting a draft
never creates ticket authority.

```bash
bun test tests/present.test.ts
/tmp/openside-heavy-run cargo test -p openside-e2e --test cards cards_present_scripted_roundtrip -j 2 -- --test-threads=1
```

The TypeScript tests are self-contained: they load the generated
`docs/specs/profiles/*.open.json` catalog snapshots and use the scripted model and the fake
JSONL daemon (`tests/helpers/fake-daemon.ts`). They check catalog and schema exposure,
card-id-only results, retries after malformed or over-limit rejections, status-only secure
results, and wire and state version invariants. The Rust `cards_present_scripted_roundtrip`
test drives the real daemon and this sidecar with the scripted provider harness and checks
real registration rejection and corrected card display without a browser or provider
request.

## Tests and checks

Run from this directory:

```bash
bun test
bun x tsc --noEmit -p .
bun x biome check .
bun test tests/providers tests/protocol.test.ts   # provider and wire subset
```

`bun test --reporter=junit --reporter-outfile=<file>` writes JUnit for `tools/gate.sh`.
Test titles keep the earlier pytest function names (for example `test_present_examples_follow_the_daemon_catalog_schema`), so `docs/specs/acceptance.csv` rows map one to one.

Deterministic provider tests use a fake credential daemon
(`tests/providers/fake-credential-daemon.ts`) and local `Bun.serve` provider servers, with
no provider credentials. They cover 429 Retry-After rotation, 401 invalidation, affinity
across turns, the 64-attempt cap, typed exhaustion, one-way cancellation, no replay of a
partial stream, canary-token masking, egress refusal and large (>64 KiB) JSONL frames to a
slow reader. No test makes a live provider or broker call.

## Task escalation and history compaction

Worker instructions use known context, authorized connections/APIs, exposed web
search, authorized browser access, then a human question. An absent connector can
be skipped; a broken connector, expired authentication, or denial cannot be
bypassed through a browser or another execution tool. Browser structure precedes
pixels. Security challenges stay human-only. This policy adds no tools and is not
installed on the front agent.

History compaction (`src/runtime/compaction.ts`) uses the current session model, not a
separate routing policy. The default history budget is 64,000 estimated tokens. Text and
structural metadata use UTF-8 bytes divided by three (rounded up), counting the current
instructions once rather than their replay copies in earlier requests. Each image
or binary media part reserves 1,600 tokens instead of counting its encoded payload.
Set `compaction_max_tokens` in the
session/model agent configuration to override it. `compaction_max_messages` is an
optional legacy trigger; short conversations are not compacted merely for
reaching 100 messages. The normal tail is 20 messages (`compaction_keep_messages`);
when that tail exceeds the size budget, the last two messages remain verbatim,
extended backward as necessary to preserve tool-call/result pairs. The first user
goal also remains verbatim. A currently indispensable oversized tail is retained,
not silently truncated; the daemon's Task spill path owns large-result storage.

After a settled turn, idle compaction waits 250 ms and skips known queued work,
pending approvals, and active children. Incoming work cancels only the speculative
summary, never a host tool. A history fingerprint, history identity, prompt epoch,
and generation compare-and-set discard stale summaries. Committed compaction
invalidates browser snapshot bases and persists a new continuation snapshot;
opaque spill artifacts retain their independent daemon retention policy. The
prompt epoch stays fixed. Failed idle summaries leave the original history intact.

## Prompt epochs and cache diagnostics

Each parent and Task agent assembles a prompt epoch (`src/features/prompt-epoch.ts`)
before its first run. The epoch stores canonical name-sorted daemon tool definitions and
separate base system, persona, skill-index, policy, and memory-profile sections in the
opaque daemon snapshot. Resume reuses these exact bytes, even after deployment changes;
removed tools still face daemon-side authorization at execution time. New sessions
create new epochs. `PromptEpoch.assemble(..., { transition: true })` is the
reset boundary; ordinary turns, deployment, and history compaction do not reset it.
Snapshots without an epoch establish one on their next open.

Live roster and selected memory context belong at the latest model-message
boundary, never in the frozen instructions. Changed stable sections produce only
a data-only pending-change notice until the next epoch. Ephemeral context is
request-local rather than repeatedly copied into durable conversation history.

Every parent/Task model request logs `prompt_sections`: section SHA-256 hashes,
Unicode character lengths, an opaque epoch ID, and change causes. First-message
hashes detect history-prefix replacement; provider/model identities are hashed.
Credential acquisition logs `prompt_request_identity`, including hashed account
identity and a distinct account-boundary cause for rotation. These stderr records
contain no prompt text, raw account IDs, or credentials. Hashes are diagnostics,
not encryption or a guarantee that low-entropy content cannot be guessed.

Every request carries the session id as pi-ai's `sessionId`, and pi-ai's default prompt
caching applies: Anthropic requests mark the stable system prompt and tool definitions
with `cache_control`, and OpenAI requests carry a `prompt_cache_key`.
`tests/providers/prompt-cache.test.ts` inspects the serialized provider requests and the
usage parsing against local servers. It does not establish live hit rates: minimum prompt
sizes, account/model boundaries, provider support, and retention still determine real cache
reuse. No paid prefetch occurs.
