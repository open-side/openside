# Models & accounts

This specification defines the owner-approved multi-account cutover: labelled provider API keys, isolated external-CLI subscription accounts, an explicit provider account pool, and daemon-owned per-request selection. It intentionally supersedes the earlier single-key-per-provider and source-wide subscription-toggle contract. There are no deprecated route aliases after cutover. openside does not implement an OAuth sign-in flow: the person signs in with the external CLI using the command displayed for that account.

## Public API

The authoritative route/schema sources are `openside-proto::ROUTES` and the generated OpenAPI snapshot. `ModelAccountRef` is `{source,account_id}`: a durable opaque source-local reference, never an upstream account identity or credential.

| Method | Route | Scope | Body / response |
| --- | --- | --- | --- |
| GET | `/v1/settings/models` | Read | Grouped model catalog, candidates and provider pools |
| PUT | `/v1/settings/models` | Admin | `{default_model,fallback_models}` → new-session selection |
| GET | `/v1/settings/keys` | Admin | `{keys:ProviderKeyInfo[]}` |
| POST | `/v1/settings/keys/{provider}` | Admin + person, or local socket | `{label,key}` → 201 metadata |
| PUT | `/v1/settings/keys/{provider}/{key_id}` | Admin + person, or local socket | `{key,label?}` → replacement metadata |
| PATCH | `/v1/settings/keys/{provider}/{key_id}` | Admin + person, or local socket | `{label?,enabled?,reserve_percent?}` → metadata |
| DELETE | `/v1/settings/keys/{provider}/{key_id}` | Admin + person, or local socket | 204 |
| GET | `/v1/settings/subscriptions` | Admin | Source groups with all subscription accounts |
| POST | `/v1/settings/subscriptions/{source}` | Admin + person, or local socket | `{label}` → 201 account and exact login command |
| GET | `/v1/settings/subscriptions/{source}/{account_id}` | Admin | Poll one external login's status |
| PATCH | `/v1/settings/subscriptions/{source}/{account_id}` | Admin + person, or local socket | `{label?,enabled?,reserve_percent?}` → account |
| DELETE | `/v1/settings/subscriptions/{source}/{account_id}` | Admin + person, or local socket | 204; wipe managed home only |
| GET | `/v1/settings/pools` | Read | `{pools:ProviderPool[]}` |
| PUT | `/v1/settings/pools/{provider}` | Admin + person, or local socket | `{accounts:ModelAccountRef[]}` → ordered provider pool |
| PUT | `/v1/sessions/{id}/account` | Steer | `{account:ModelAccountRef|null}` → next-request preference |

A read-scoped paired phone cannot inspect key metadata or alter managed credentials/pools. Admin-scoped agent clients cannot perform person-only credential/pool mutations. A local socket is the local operator. Revoked devices are rechecked inside serialized writes. Session account preference may only select a member of the effective, already authorized provider/Agent pool; it cannot enable an account or introduce another source.

`ProviderKeyInfo` contains only `{provider,key_id,label,last_four,created_at,enabled,reserve_percent}`. Managed API-key namespaces are `anthropic`, `openai`, and `google`; unsupported providers fail with `invalid_provider`. Keys are 8–8192 ASCII graphic bytes and malformed value-carrying bodies return fixed `invalid_value`, never input-quoting parser errors. Labels are 1–80 non-control Unicode characters. Reserve percent is0–99, default10. A replacement preserves the durable key id and replaces creation time. Key material never appears in public responses, config TOML, argv, process environment, events or logs.

`SubscriptionAccount` contains `{source,provider,account_id,label,managed,installed,logged_in,enabled,available,reason,login_command,created_at,reserve_percent}`. Source is `claude-code` or `codex-cli`. Admin-only login_command contains the account-specific environment/home setting and `claude auth login` or `codex login`, never a token. The GUI creates the account, copies that command for the person to run, then polls its status; it does not open or implement its own OAuth flow.

`ProviderPool` is `{provider,accounts:ModelAccountRef[]}`. It is person-built and ordered; an empty provider pool disables routing. Adding a key or CLI account does not silently add it to an explicit pool. Existing SDK namespaces remain distinct: in particular `openai` and `openai-codex` are not automatically interchangeable, and pool members must support the selected model/provider.

## Catalog, account metadata and model selection

Catalog candidates combine configured default/fallback, source-reported models and the curated SDK chat/tool-capable model list. Groups are sorted and deduplicated. Availability means local credential/pool/model readiness, not a live upstream entitlement check. The catalog exposes each provider's pool and candidate account `{source,account_id,label,enabled,ready,in_pool,available,reason,usage_windows,blocked_until_ms}`. `ready` is source readiness; `available` also applies membership, compatibility and active blocks. Read metadata contains neither key suffixes, private login homes/commands, upstream identities nor credentials.

`GET /v1/accounts` lists every source's accounts with attributed usage and safe status. A usage window is `{name,used_percent:number|null,reset_at_ms:number|null}`; unknown usage/reset is null, not a fabricated zero or timer. Metadata may be cached for at most five seconds, invalidated after managed writes. Authentication never relies on that metadata TTL: every outbound model attempt resolves current credential state.

`PUT /v1/settings/models` validates every selected id against the available catalog; max16 fallbacks, no duplicate default/fallback. It preserves unrelated config and writes private TOML atomically without daemon restart. Response `{default_model,fallback_models,restart_required:false,effective:"new_sessions_and_home_next_turn"}`. Ordinary/History sessions retain their stored model; a Team home uses its Agent override, otherwise the current default, and reopens its idle sidecar on the next real turn when effective selection changes. A running request completes with its captured model/fallback binding. Unknown/unavailable ids return `model_unavailable`; invalid list sizes/duplicates return `invalid_models`. Changed default/fallback selection advances the model generation; a repeated identical selection does not.

Agent `account_pool` is null to inherit, or a nonempty ordered restriction within the person-built provider pool; an empty Agent list is invalid. Session account pinning is a preferred sticky account, not permission to bypass disabled/blocked state or escape an Agent restriction. Rotation may select another allowed member; a one-account restriction cannot escape to a sibling. Affinity uses durable ids, never array positions.

## API keys and managed subscription storage

API keys are dedicated encrypted Vault entries, OS-keyring-wrapped by default; an existing explicitly configured passphrase Vault remains respected. Locked/unavailable Vault refuses managed credential release. Typed credential metadata separates keys/subscriptions/channels from ordinary browser items: no browser origins, generic item CRUD/fill, device-sync export/import, or overwrite of a legacy browser item whose label happens to resemble a credential id. Credentials are released only through audited private callbacks and registered with output redaction.

The following managed-subscription policy is an intentional override of the previous memory-only rule:

- The Vault is canonical for managed subscription OAuth state; only the daemon refreshes it. Refresh tokens never enter sidecars, web/API responses, logs or diagnostics.
- Each added account receives a private managed CLI home under daemon data. At launch the daemon materializes its canonical credential file with mode0600 inside mode0700 home.
- If the external CLI rotates the file, generation/mtime plus credential-content/revision checks adopt the newer login into the Vault, then rewrite canonical file state. A stale refresh cannot overwrite a newer login or resurrect a removed account.
- Account removal wipes only its managed credential file/home, without following untrusted symlinks. Existing default user homes are read-only, external-CLI-owned accounts; the daemon never writes/removes them.
- Default Claude detection remains bounded/read-only (`claude auth status` is not invoked because it creates CLI configuration files). Default Codex detection reuses the existing CLI format. No raw login identity/token is displayed.

Refresh is single-flight per durable account/revision. One ordinary OAuth401 can request owner-side refresh-current recovery; recent successful mint reuse avoids repeated refresh-token churn. Definitive invalid-grant/auth failure marks only the matching revision unusable; transient network/server failure does not permanently disable it. Broker-owned accounts request authoritative broker refresh; openside does not redeem/copy that broker's refresh tokens.

Managed homes are `<data_dir>/managed-subscriptions/<source>/<account_id>`. POSIX sign-in commands isolate Claude with `CLAUDE_CONFIG_DIR` and clear inherited secure-store/custom-OAuth overrides; Codex uses `CODEX_HOME` and `-c 'cli_auth_credentials_store="file"'`. Windows commands use PowerShell environment assignments. The daemon does not execute these sign-in commands or open a browser: the operator runs the displayed command, and polling adopts the resulting external login. [Claude authentication](https://code.claude.com/docs/en/authentication), [Claude CLI reference](https://code.claude.com/docs/en/cli-reference), and [Codex credential storage source](https://github.com/openai/codex/blob/main/codex-rs/login/src/auth/storage.rs) define these CLI-specific locations.

Windows creates managed directories and credential replacement files with a protected owner-only DACL (`D:P(A;OICI;FA;;;OW)`) in `SECURITY_ATTRIBUTES`, matching the native importer’s existing pattern. It does not create a broadly accessible file and tighten it later, or rely on a custom data directory’s inherited ACL. The replacement file is flushed before atomic replacement, and the owner-only ACL is checked before launch.

On macOS Claude uses the login Keychain, not the Linux credential file. openside addresses only the generic-password item for the exact managed `CLAUDE_CONFIG_DIR` and the current OS account; it never enumerates Keychain items. Its service is `Claude Code-credentials-` followed by the first eight lowercase SHA-256 hex digits of the NFC-normalized literal directory string. This is a version-sensitive CLI implementation detail, with decoded-source provenance in the [Claude Code namespace investigation](https://github.com/alexey-pelykh/sessiometer/blob/main/build/version-compat.md#issue-100--keychain-service-name-under-a-non-default-claude_config_dir), not a stable upstream API guarantee. The existing `keyring` apple-native backend reads, materializes and deletes that one item. **First access may display a macOS Keychain permission prompt** because the CLI and daemon have different application identities; refusal leaves credentials unavailable. No default-profile Keychain item is modified. Native storage uses exact-item content revisions because the keyring API does not expose modification timestamps; Linux and Windows retain file modification-time/content checks.

Canonical OAuth state and its revision are atomically persisted together before materialization. The stored external digest identifies the pre-materialization file, so restarting after a canonical write but before the CLI write restores the newer Vault state rather than importing the old file. Removal first durably disables the record, wipes its managed location, then removes the typed record; a failure during wiping leaves an unavailable account that can be removed again. Default-home metadata may be edited, but its credential remains external-owned and is never refreshed or deleted by the daemon.

Concurrency boundary: the daemon serializes its own per-account operations and checks durable revision plus observed file content/time before committing; it hashes the materialized credential again afterward. A detected external change fails the current resolution and is adopted on the next poll/request. There is no shared lock protocol with the external CLI: a same-user process replacing a credential between the final check and replacement is a trusted-writer residual, not an atomic cross-process CAS guarantee. Windows checks each path component with `CreateFileW(FILE_FLAG_OPEN_REPARSE_POINT)`, rejects reparse points and detects changed ancestor identities; path-based operations retain the same same-user namespace-race boundary as native browser imports. The macOS service suffix must match the CLI’s 32-bit truncated hash: crafted/colliding user-selected configuration paths may alias a Keychain service, and that upstream namespace limit is not strengthened by openside. Same-user malware is outside this boundary; the daemon must never follow an observed link outside a managed home.

Removing a default account records a durable hidden/disabled tombstone in openside; discovery does not recreate it, and no external home or Keychain item is wiped. Initial default-account enablement preserves the existing `credentials.subscriptions` setting (Claude defaults off, Codex defaults on), rather than silently enabling a previously excluded credential source.

## Account pool & rotation

The daemon is the single selection, usage and rotation policy owner. Python SDK clients obtain a fresh private credential snapshot for every outbound model-request attempt; SDK objects may stay warm, but captured credentials do not remain authorized until the session ends. An already-sent request may finish. A later request observes removal, replacement, disablement, pool edits and refresh without restarting the conversation.

G-2 amendment: migration starts with accounts of the currently selected source only. An unreachable/blocked broker cannot turn migration into Vault/CLI/env/command fallback. The person may explicitly include compatible accounts from different sources in one provider pool; only those explicitly listed sources may participate in failover. Source-wide outage, AuthUnavailable or exhaustion never spills into an unlisted source. Cross-provider changes require an explicitly configured fallback model. Failure is visible as a sanitized reason and authoritative retry time when known, never silent downgrade.

Selected-source pools are initialized after startup Vault unlock and before credential CRUD/Vault lock, so adding a new higher-priority Vault account cannot replace a previously active source. A provider without any usable source or selected broker barrier stays unmigrated until a source becomes usable; an explicitly saved empty pool remains disabled.


Selection policy:

1. Adopt current credential revisions; intersect the provider pool with Agent restrictions and remove disabled/deleted/unreadable/incompatible or actively blocked members.
2. Prefer an eligible explicit session preference, then a healthy warm sticky account, preserving prompt-cache value and durable identity.
3. For new/cold/invalid affinity, use verified applicable quota windows/reserves and headroom, then person-configured order for ties. Unknown quota is not unlimited; sources without quota APIs retain ordered candidates with null windows.
4. Attribute usage/outcomes to the leased durable account and credential revision. Merge out-of-order blocks by longest active deadline and preserve account/model scopes. Sanitized quota windows and observed token/request counts belong to the daemon; stale reports do not extend cache freshness or heal auth-invalid state.
5. Classify ordinary401 as auth; account/model permission403 as policy; usage/quota exhaustion as quota; account-scoped or bare429 as rate_limit; proven per-minute/concurrency caps, transport errors and5xx as transient. Raw error text/bodies/headers/identities are not persisted or returned.
6. Refresh eligible OAuth once on ordinary auth failure, then rotate to a distinct allowed sibling. API-key auth failure rotates directly. Rate/quota/policy blocks rotate only within the effective pool. Transient errors do not cause credential rotation. Never replay a model attempt after its streaming context has yielded output/stream.
7. Reconsider accounts after authoritative reset/retry times using current revision/usage. Unknown reset remains unknown; manual disable never auto-enables. Recovery permits a new/cold affinity without disrupting a healthy warm sibling. A newer credential/login or successful owner recovery clears auth-invalid state.
8. Maximum64 outbound attempts per logical model request, including the initial attempt; one refresh-current replay per account revision, no unchanged-credential cycles, no replay after partial streaming. Exhaustion fails closed.

Credential generation/startup fencing remains: account/key/source/pool/refresh mutations advance generation; startup rechecks before runtime open and re-resolves a changed snapshot while the read/write fence closes the check-to-spawn race. Mutation generation advances before releasing the write permit, including cancellation/partial failure. A write may wait for an in-flight bounded startup handshake. The earlier rule that an active SDK client keeps a captured key until sidecar end is intentionally superseded by per-request authentication.

Lease robustness: private leases have a10-minute TTL, max8 per session and256 globally, with atomic insertion checks. Periodic/admission reaping and runtime disconnect/shutdown cleanup release owned leases and retry state. Shared-health reports are limited to8 modifying observations per session per60 seconds. Broker-measured windows are authoritative; SDK-reported windows are advisory, clamped to7 days and ordered by lease observation so older same-revision reports cannot lower newer usage or extend a stale cooldown. A missing reset remains null in public metadata; a60-second daemon recheck backoff is not a claimed quota reset. Auth reports put the account in `needs_check`, not administrative disablement: the owner verifies OAuth through refresh, and a static credential is checked by its next admitted provider request after bounded backoff. Only a definitive owner verification failure may persist invalid state.

Agent model/account_pool/status changes participate in the same generation writer fence. A slow credential resolver must re-evaluate current Agent authority when a concurrent narrowing advances generation, rather than delivering a now-disallowed account.


## Private credential RPC

The private authenticated v1 session socket carries `credential_request` → `credential_response` or `credential_error`, and `credential_report` → `credential_reported`. These are not public HTTP routes or model tools; Host binds session authority. Core typed callbacks resolve and report, while the sidecar is only a request client, not another selector/refresher.

Resolve includes opaque request_id, provider, qualified model, zero-based attempt0–63 and nullable daemon-known subagent. Success carries a lease, generation, opaque account ref and selected auth `{kind,value,upstream_account_id,expires_at_ms}`; kind is api_key/claude_code/codex_cli/bearer. Only an API key/access token and necessary private request identity cross this boundary. No refresh token, home, broker bearer or complete source configuration is sent. Redacting Debug/repr and zeroized serialized buffers protect ordinary output/lifetime; no credential disk snapshot cache is added.

Report outcome is success/auth/rate_limit/quota/policy/transient/cancelled, with stream_started, nullable provider retry duration, account/model scope, sanitized usage counts/windows. Ack action is complete/refresh/rotate/stop. Report effects commit before ack, which must be awaited before reacquiring. Refresh/rotate ack carries no credential: the sidecar resolves again with incremented attempt under the same logical request id. Delayed/idempotent lease-bound reports cannot block a replacement credential or recreate an account. On cancellation/abort/shutdown only, send-only cancelled reports may discard their late ack; no resolve/retry follows, avoiding cleanup dispatcher deadlock.

Python session and diagnostic paths do not read credential homes/refresh tokens/broker credential snapshots. Daemon-provided sanitized metadata supports diagnostics; standalone Python diagnostics may report installation/configuration, not fabricate login readiness. Credential-free curated SDK model discovery remains in Python.

Sidecar and SDK-index subprocesses start with `env_clear` and a runtime-only allowlist: process/runtime paths, HOME and XDG directories, locale/temp directories and `SSL_CERT_FILE`/`SSL_CERT_DIR`. Proxy variables are omitted because current network policy does not authorize inherited proxies. Known and custom credential environment names override this allowlist and remain daemon-only, including a custom name that happens to be HOME. The diagnostic input carries model identifiers/provider namespaces only; startup forwards only a bounded validated `anthropic.claude_code_version`, never source configuration. Disposable test script/stderr paths are explicitly allowed non-secret instrumentation.

The entire managed-subscriptions directory is protected from model file tools, artifact collection and sandboxed bash. Credential files discovered inside managed homes also enroll retained metadata-only CLOEXEC descriptors, so hardlink aliases and an unlinked but still-open old token file remain protected.

## Child crash protection

Only session-sidecar children receive soft/hard RLIMIT_CORE=0 before exec, plus Linux PR_SET_DUMPABLE=0. Because [Linux exec resets dumpability](https://man7.org/linux/man-pages/man2/execve.2.html), the built-in Python child reapplies the Linux flag before connecting/accepting credentials and fails closed if protection cannot be installed. This does not change daemon dump policy, erase every memory copy, or protect against privileged inspection/swap. A custom sidecar must reapply Linux dumpability after exec before receiving credentials.

## CLI and verification

`openside models` lists catalog/pool eligibility; `models set <model> --fallback <model>` selects defaults for new sessions. `models pool list` and `models pool set <provider> [source:account_id ...]` manage the explicit ordered trust boundary; an empty list disables the provider. `models account <session> [source:account_id]` sets/clears the next-request preference. `openside accounts` lists all source accounts; `accounts add <source> --label <label>`, `show`, `enable`, `disable` and `rm <source> <account_id>` manage subscriptions and print exact external login guidance only on the admin surface. `openside key set <provider> --label <label>` reads hidden stdin only; `--key-id <id>` replaces. `key update <provider> <key_id>` edits metadata and `key rm <provider> <key_id>` removes one account. No key argument is accepted; parse errors cannot echo an accidentally supplied key.

Acceptance requires multiple same-provider keys and isolated CLI homes; person/scope refusal; no key/token in public/config/log output; explicit-pool fail-close and affinity; auth versus rate/quota classification; bounded distinct-account retry and no partial-stream replay; single-flight refresh/revision race protection; default-home read-only behavior and managed-file wipe; deleted/disabled accounts observed by later requests; generation/startup fencing; and spawned child core/dumpability assertions. Disposable daemon smoke and scripted/mocked upstream boundaries avoid touching live installations/accounts. No live upstream entitlement or OAuth login claim follows from metadata readiness alone.
