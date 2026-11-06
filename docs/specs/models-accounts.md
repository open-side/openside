# Models & accounts

This specification defines the owner-approved multi-account cutover: labelled provider API keys, isolated external-CLI subscription accounts, openside-held OAuth accounts (`pi-oauth`), an explicit provider account pool, and daemon-owned per-request selection. It intentionally supersedes the earlier single-key-per-provider and source-wide subscription-toggle contract. There are no deprecated route aliases after cutover. External-CLI subscription accounts are signed in with the external CLI using the command displayed for that account; `pi-oauth` accounts are signed in by openside itself through a daemon-owned helper (see [OAuth accounts](#oauth-accounts-pi-oauth)).

## Public API

The authoritative route/schema sources are `openside-proto::ROUTES` and the generated OpenAPI snapshot. `ModelAccountRef` is `{source,account_id}`: a durable opaque source-local reference, never an upstream account identity or credential.

| Method | Route | Scope | Body / response |
| --- | --- | --- | --- |
| GET | `/v1/settings/models` | Read | Grouped model catalog, candidates and provider pools |
| PUT | `/v1/settings/models` | Admin | `{default_model,fast_model,fallback_models}` → reasoning/fast selection |
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
| GET | `/v1/settings/oauth/providers` | Admin + person, or local socket | `{providers:OAuthProviderInfo[]}` in fixed order |
| GET | `/v1/settings/oauth/accounts` | Admin + person, or local socket | `{accounts:OAuthAccount[]}` sorted by provider, then creation time |
| POST | `/v1/settings/oauth/accounts/{provider}` | Admin + person, or local socket | `{label,join_pool?,method?}` → 202 `OAuthTransaction` |
| GET | `/v1/settings/oauth/accounts/{provider}/{account_id}` | Admin + person, or local socket | `OAuthAccount` |
| PATCH | `/v1/settings/oauth/accounts/{provider}/{account_id}` | Admin + person, or local socket | `{label?,enabled?,reserve_percent?}` → `OAuthAccount` |
| DELETE | `/v1/settings/oauth/accounts/{provider}/{account_id}` | Admin + person, or local socket | 204; local removal only |
| POST | `/v1/settings/oauth/accounts/{provider}/{account_id}/relogin` | Admin + person, or local socket | `{method?}` → 202 `OAuthTransaction` |
| GET | `/v1/settings/oauth/transactions/{transaction_id}` | Admin + person, or local socket | Poll one sign-in |
| POST | `/v1/settings/oauth/transactions/{transaction_id}/input` | Admin + person, or local socket | `{code}` → 202 `OAuthTransaction` |
| POST | `/v1/settings/oauth/transactions/{transaction_id}/cancel` | Admin + person, or local socket | 204 |

A read-scoped paired phone cannot inspect key metadata or alter managed credentials/pools. Admin-scoped agent clients cannot perform person-only credential/pool mutations. A local socket is the local operator. Revoked devices are rechecked inside serialized writes. Session account preference may only select a member of the effective, already authorized provider/Agent pool; it cannot enable an account or introduce another source.

`ProviderKeyInfo` contains only `{provider,key_id,label,last_four,created_at,enabled,reserve_percent}`. Managed API-key namespaces are `anthropic`, `openai`, and `google`; unsupported providers fail with `invalid_provider`. Keys are 8–8192 ASCII graphic bytes and malformed value-carrying bodies return fixed `invalid_value`, never input-quoting parser errors. Labels are 1–80 non-control Unicode characters. Reserve percent is0–99, default10. A replacement preserves the durable key id and replaces creation time. Key material never appears in public responses, config TOML, argv, process environment, events or logs.

`SubscriptionAccount` contains `{source,provider,account_id,label,managed,installed,logged_in,enabled,available,reason,login_command,created_at,reserve_percent}`. Source is `claude-code` or `codex-cli`. Admin-only login_command contains the account-specific environment/home setting and `claude auth login` or `codex login`, never a token. The GUI creates the account, copies that command for the person to run, then polls its status; it does not open or implement its own OAuth flow.

`ProviderPool` is `{provider,accounts:ModelAccountRef[]}`. It is person-built and ordered; an empty provider pool disables routing. Adding a key or CLI account does not silently add it to an explicit pool. Existing SDK namespaces remain distinct: in particular `openai` and `openai-codex` are not automatically interchangeable, and pool members must support the selected model/provider. A member its source no longer reports (for example an `omp-broker` credential id the broker re-created under a new id) is skipped, and routing moves on to the next member. When every member of a non-empty pool is unreported, the pool routes through the accounts of the provider's first ready source in its source order (the same choice that seeds a missing pool) instead of failing every request; the substitute is not persisted, so the stored pool keeps the stale pin until the person replaces it, and a member that reappears takes over again. The catalog then marks those accounts `in_pool` and each model's `reason` reads `Pool accounts <source:id, …> are no longer reported by their source; using current <source> accounts`; with no ready substitute it reads `… choose current accounts for the <provider> pool`. Broker members are unknowable, not stale, while the configured broker cannot be listed: that pool stays closed (`Configured broker unavailable; no unlisted-source fallback`) rather than moving to a source the person never listed. Session account preference may select a substitute account.

## Catalog, account metadata and model selection

Catalog candidates combine explicitly configured default/front/fallback ids and the current curated SDK chat/tool-capable model list. Account-reported ids restrict availability but cannot reintroduce superseded generations. Groups are sorted and deduplicated. Availability means local credential/pool/model readiness, not a live upstream entitlement check. The catalog exposes each provider's pool and candidate account `{source,account_id,label,enabled,ready,in_pool,available,reason,usage_windows,blocked_until_ms}`. `ready` is source readiness; `available` also applies membership, compatibility and active blocks. Read metadata contains neither key suffixes, private login homes/commands, upstream identities nor credentials.

`GET /v1/accounts` lists every source's accounts with attributed usage and safe status. A usage window is `{name,used_percent:number|null,reset_at_ms:number|null}`; unknown usage/reset is null, not a fabricated zero or timer. Metadata may be cached for at most five seconds, invalidated after managed writes. Authentication never relies on that metadata TTL: every outbound model attempt resolves current credential state.

The session API's `model` field records the provider-qualified model selected when the session was created. It is not the effective current model after a Team home adopts role/settings changes or a request uses a fallback. The CLI conversation list labels this column **Started with**; the web conversation UI keeps titles and status only, with no model label. Usage records describe actual execution without rewriting the person's conversation title.

`PUT /v1/settings/models` validates every selected id against the available catalog; max16 fallbacks, no duplicate default/fallback. It preserves unrelated config and writes private TOML atomically without daemon restart. Response `{default_model,fast_model,fallback_models,restart_required:false,effective:"new_sessions_and_home_next_turn"}`. Ordinary/History sessions retain their stored model; a Team home uses its Agent override, otherwise the current default, and reopens its idle sidecar on the next real turn when effective selection changes. A running request completes with its captured model/fallback binding. Unknown/unavailable ids return `model_unavailable`; invalid list sizes/duplicates return `invalid_models`. Changed reasoning/fast/fallback selection advances the model generation; a repeated identical selection does not.

### Reasoning and fast roles

Settings configures both roles once, not per conversation or message. `default_model` is the **Reasoning model** (`models.default`), used by ordinary sessions, Tasks without a chief-selected override, and teammates without their own override. `fast_model` is the **Fast model** (`models.front`) for the chief's front-agent loop only. Null uses the reasoning model; selecting the same id as `default_model` stores null. The PUT response and catalog both include `fast_model`. Changing either role advances the home generation, so an idle chief adopts it on its next turn without a daemon restart. Task admission defaults to the session's reasoning model; the sidecar builds Task children from the admitted model, never implicitly from `front_model`.

Each model has nullable `recommended_for: "fast" | "reasoning"`, `tier: "fast" | "reasoning" | "frontier"`, a short `strengths` line, and `in_catalog`. One credential-free family table in `FAMILIES` in `agent/src/providers/catalog.ts` owns current generations and routing metadata. Superseded entries (`gpt-5.4-mini`, `claude-opus-4-8`, `claude-sonnet-4-6`, `gemini-2.5-pro`) are hidden even if reported by accounts. An explicitly configured id outside the table stays selectable subject to credential readiness, marked `in_catalog:false`. Unknown configured families use the neutral reasoning tier and explicitly state that capabilities are not curated. Recommendations are hints, not restrictions: either Settings role accepts any available model.

When both selections are unset, startup and credential/account/pool mutations attempt one automatic pair selection. It chooses the first available recommended reasoning model and the first available recommended fast model in the same provider/pool, falling back to another provider. It persists the pair atomically only after rechecking stored choices under the settings write lock. Explicit selections, including a reasoning model with null fast selection, are never replaced. Catalog GET remains read-only. If no recommended reasoning model is available, selection remains unset.

`openside models` displays both roles and remaining usage for each connected account, preserving unknown values and reset times. `openside models set <reasoning-id> --fast <fast-id|none> [--fallback <id>]` saves the pair; `none` clears the fast override.

Agent `account_pool` is null to inherit, or a nonempty ordered restriction within the person-built provider pool; an empty Agent list is invalid. Session account pinning is a preferred sticky account, not permission to bypass disabled/blocked state or escape an Agent restriction. Rotation may select another allowed member; a one-account restriction cannot escape to a sibling. Affinity uses durable ids, never array positions.

### Chief Task model routing

`create_task({goal,tools?,model?})` accepts a model from the person's available connected catalog and the chief's effective provider/Agent pool. An unavailable id returns typed denial `model_unavailable`; a model with no eligible account in the Agent restriction returns `model_not_in_pool`; unavailable catalog metadata returns `model_catalog_unavailable`. Omitting `model` retains the session reasoning model. The admitted model is recorded in the Task and used by its child loop. Children retain the sidecar's captured configured fallback chain; credential authorization accepts the admitted child model or that captured fallback chain, never a newly configured or front-only model.

The front-only `get_models({})` returns `{models:[{id,display_name,tier,strengths,remaining_percent,resets_at,usage_windows,accounts}]}` for eligible connected models only. Each account contains a masked ordinal `label`, `remaining_percent`, `resets_at`, and `usage_windows`. Windows contain `{name,remaining_percent,resets_at}`; reset times are Unix milliseconds and unknown values are null. Model summaries use the eligible account with the greatest known remaining percentage, with each account's limiting known window defining its summary. Unknown windows remain visible and are not treated as unlimited. No account identifiers, source references, raw labels, upstream identities, or credentials enter this tool response.

The front consults this inventory and silently routes trivial lookups/simple browsing to fast models and multi-step/hard work to reasoning/frontier models. It avoids models below 10% remaining when alternatives exist and never chooses unavailable models. `send_message` conveys only a concise answer, necessary question, or useful result—not model choices, routing, tool activity, or Task-start announcements.

The front loop requests reasoning effort `minimal` with `maxTokens` 1024 (`agent/src/runtime/session.ts`); the pinned pi-ai translates the effort into each provider's own reasoning field, and a model override with reasoning disabled sends none. This does not change Task/child model settings.

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

## OAuth accounts (`pi-oauth`)

`pi-oauth` accounts are signed in by openside on the daemon host. Rust owns the provider allowlist, authorization, storage and refresh policy; a one-shot helper process does only the provider protocol. Source `pi-oauth` is listed in `GET /v1/accounts` but is in no default source order, so a new account never joins a pool by itself: `join_pool:true` appends it to the end of its provider pool (creating the pool) through the same serialized settings write as `PUT /v1/settings/pools/{provider}`; otherwise the person adds `pi-oauth:<account_id>` later.

| Provider (pool namespace) | Name | Methods (first = default) | Pasted code | Lease | Required sign-in detail |
| --- | --- | --- | --- | --- | --- |
| `anthropic` | Claude (Pro/Max) | browser | browser | `claude_code` | none |
| `openai-codex` | ChatGPT (Codex) | browser, device | browser | `codex_cli` + upstream account id | account id |
| `google-gemini-cli` | Google Gemini CLI | browser | browser | `bearer` + private `request_metadata.project_id` | project id |
| `google-antigravity` | Google Antigravity | browser | browser | `bearer` + private `request_metadata.project_id` | project id |
| `github-copilot` | GitHub Copilot | device | none | `bearer` | none |

Other providers return `oauth_provider_unknown` (400); a method not listed returns `oauth_flow_unsupported` (400). An account missing its required detail is listed with `available:false` and reason `Account is missing required sign-in details`; other reasons are `OAuth account ready`, `Account disabled` and `Sign in again`. `OAuthAccount` is `{source,provider,account_id,label,enabled,reserve_percent,available,reason,auth_invalid,created_at_ms}`. Its `account_id` is openside's own local handle: a random opaque id the daemon generates when the transaction starts, never derived from provider data, and the only id routes and pool references use. Replies carry no provider-side identifier: no email, plan, organization, project id, upstream account id, token or token metadata.

**Transactions.** Create and relogin return 202 with an in-memory `OAuthTransaction` `{transaction_id,kind,provider,account_id,method,status,reason,authorization,manual_input,expires_at_ms,account}`. Status runs `starting` → `awaiting_browser` (browser) or `awaiting_code` (device, `authorization.user_code`) → `exchanging` → `ready` (with `account`), or ends `failed` (`oauth_callback_busy`, `oauth_invalid_grant`, `oauth_failed`, `oauth_flow_unsupported`, `oauth_helper_unavailable`, `oauth_account_removed`), `cancelled` (`oauth_cancelled`) or `expired` (`oauth_expired`, also used when the helper reports that the provider's own wait timed out). `authorization` is cleared once the transaction finishes. `manual_input` is true only while the helper asked for a pasted code and the status is `awaiting_browser`; `POST …/input` otherwise returns `oauth_manual_input_not_allowed` (409), and on a finished transaction `oauth_transaction_finished` (409). A paste moves the transaction to `exchanging`; if the helper asks again before it reports the exchange, the paste was rejected and the transaction returns to `awaiting_browser` with `manual_input:true`. The code is 1–4096 characters, write-only and never echoed. Transactions expire 10 minutes after creation (the helper is killed then), are kept 10 minutes after finishing, and are never persisted. At most 4 run at once and one per account (`oauth_busy`, 409); the limit of 4 is shared by every caller on the daemon, a known limit of the first release. A transaction belongs to the device that started it: another paired device gets `oauth_transaction_unknown` (404); the local socket sees all. Cancelling kills the helper; cancelling a finished transaction is a no-op 204.

**Create** validates label, provider, method and an open Vault (`vault_unavailable`, 409) before starting; nothing is stored until the provider returns credentials, then the account is written with `enabled:true`, `reserve_percent:10`. The store and the move to `ready` happen together: a cancel or delete that reaches the transaction first means nothing is stored, and one that arrives after the store no longer changes its outcome. With `join_pool` the account is then appended to its provider pool unless that pool already holds the maximum of 64 accounts. **Relogin** requires an existing account (`unknown_account`, 404) and replaces its credentials under revision compare-and-swap, keeping label, enablement, reserve and pool membership and clearing `auth_invalid`; if the account is deleted first the transaction fails with `oauth_account_removed` and nothing is recreated. **Delete** fails that account's live relogins with `oauth_account_removed`, removes the Vault record, removes the reference from every pool and drops its health, sticky and preferred state; a second delete returns `unknown_account`. If the pool cleanup fails after the record is gone, the delete still succeeds and the daemon logs the failure without detail; the leftover reference resolves to no account and can be removed with `PUT /v1/settings/pools/{provider}`. Deletion is local only: openside never claims to revoke the grant at the provider.

**Helper boundary.** The daemon spawns `<agent argv> auth-login` or `<agent argv> auth-refresh` on the daemon host with a cleared environment plus the runtime environment, piped stdin/stdout, no stderr, its own process group and kill-on-drop. They exchange JSON lines (≤ 65,536 bytes each, each ending in a newline); an oversize line, an unterminated final line or an unknown message fails the transaction with `oauth_failed`. The helper keeps no state, writes nothing to disk and only reaches the selected provider's auth origins plus its own loopback callback. Callback listeners bind the daemon host's loopback, so a remote person device opens the authorization URL and, where the provider supports it, pastes the code or redirect URL back through `…/input`. Agent clients and channel callers (paired agent devices and `x-openside-client: mcp|channel`) are refused with `person_device_required` (403).

**Callback listener proof (pi-ai 18.16.0, pinned in `agent/package.json`).** The helper calls pi-ai's own login and adds no listener of its own. The browser providers advertise their registered redirect unchanged; pi-ai binds only loopback addresses for it, never `0.0.0.0` or `::`. Paths below are under `agent/node_modules/@oh-my-pi/`.

- **Callback host per provider** (`pi-catalog/src/compat/rules/auth/*.kdl`, the `callback` line of each `login "oauth-code"` block):
  - `anthropic` uses `localhost:54545/callback` (`anthropic.kdl:18`).
  - `openai-codex` uses the exact `redirect-uri="http://localhost:1455/auth/callback"` with `port-fallback=#false` (`openai-codex.kdl:16`).
  - `google-gemini-cli` uses `hostname="127.0.0.1"`, port 8085 (`google-gemini-cli.kdl:11`).
  - `google-antigravity` uses `hostname="127.0.0.1"`, port 51121 (`google-antigravity.kdl:14`).
  - `openai-codex-device` and `github-copilot` are `login "custom"` device-code flows (`openai-codex-device.kdl:3`, `github-copilot.kdl:4`). They poll the provider and open no listener: no `Bun.serve` call exists in `pi-ai/src/registry` outside `oauth/callback-server.ts`.
- **Bind addresses:**
  - `resolveCallbackOptions` passes the rule's hostname through (`pi-ai/src/registry/engine/oauth-code.ts:39-51`). A redirect override can only re-point the listener at a loopback host (`oauth-code.ts:25-27, 68-79`); none of these rules declares an override variable.
  - `#createServer` binds a non-`localhost` hostname exactly as given (`pi-ai/src/registry/oauth/callback-server.ts:480-481`). For `localhost` it binds the literals `127.0.0.1` and, when the host has IPv6 loopback, `::1` (`callback-server.ts:22-25, 490-520`).
  - Every listener is a `Bun.serve` with an explicit `hostname` and `reusePort: false` (`callback-server.ts:525-532`).
- **State check:**
  - The default state is 16 random bytes, hex-encoded (`callback-server.ts:220-226`; the rule's `state` default is used by `oauth-code.ts:112-121`).
  - A callback whose `state` differs is answered 500 and never resolves the flow (`callback-server.ts:573-574`). An `error` redirect is honoured only when it carries the expected state (`callback-server.ts:584-595`).
  - Pasted input with a different state is ignored (`callback-server.ts:641-646`).
- **PKCE:**
  - The verifier is 96 random bytes, base64url-encoded, and the challenge is its SHA-256 (`pi-ai/src/registry/oauth/pkce.ts:5-17`).
  - The verifier is kept in a private field (`oauth-code.ts:93, 131-135`). Only the `S256` challenge goes in the authorization URL (`oauth-code.ts:152-155`); the verifier is sent only in the token exchange (`oauth-code.ts:191, 206`).
  - `google-gemini-cli` declares no PKCE; its state check still applies.
- **Single use:**
  - The callback is one `Promise.withResolvers` (`callback-server.ts:618-620`). Its resolvers are cleared on abort and once the race settles (`callback-server.ts:622-630, 654-658`).
  - `login()` stops the server and clears the pending `/launch` URL in `finally` (`callback-server.ts:322-337`). A replayed callback therefore has no listener to reach.
- **Egress:**
  - The OAuth code paths reach the network only through the free `fetch` identifier, resolved at call time, or `ctrl.fetch`, which the helper leaves unset. This covers `oauth-code.ts:109`, `engine/device-code.ts:42`, `engine/refresh.ts:64`, `oauth/openai-codex.ts:158, 216, 262`, `oauth/github-copilot.ts:337` and `oauth/google-oauth-shared.ts:55`. The helper's egress guard therefore covers them.
  - The only `node:net`/`node:tls` namespace imports in pi-ai are in `utils/proxy.ts`, which only the cursor provider uses.

`agent/tests/auth-helper.test.ts` covers this list. Each browser provider runs its real pi-ai login, with only the token exchange stubbed. The tests check five things:

- The listener accepts on `127.0.0.1` and is unreachable on every non-loopback interface address.
- A wrong-state callback gets a 500 and nothing is exchanged.
- The PKCE verifier matches the challenge.
- The listener is closed once the flow settles.
- A login or refresh that fetches an unlisted origin ends with `failed`, and the request never leaves the process (a child process runs with the real guard installed).

**Refresh.** Resolution is single-flight per account. The daemon refreshes when access expires within 60 seconds, or once after an auth report against the leased revision, through the `auth-refresh` helper (60-second deadline). A refreshed payload is stored with compare-and-swap against the revision it read. If the revision moved while the helper ran but the stored refresh token is still the one it sent, only metadata changed (a rename or toggle), and the refreshed tokens are stored on top of the new metadata; a different refresh token is a newer sign-in, which is used and never overwritten. `invalid_grant` marks `auth_invalid` by the same rule, never on a newer sign-in; other failures are transient and change nothing, and a token that is still valid (inside the 60-second window) is leased anyway unless the refresh was forced by an auth report. Readiness of accounts with required sign-in details is cached per revision, so listing accounts does not read their tokens each time. Leases carry `{kind,value:access,upstream_account_id,expires_at_ms,request_metadata}` per the table; refresh tokens never leave the daemon.

## Plugin sources

A plugin source is a model relay that only some networks can reach (an office LAN or a tailnet). It is described by a local manifest in `plugins/*.toml` beside the config file (`~/.config/openside/plugins/` by default). Manifests are personal files: they live outside every repository, carry the person's own identity value, and openside never writes, lists or returns them. There is no Settings card, API route or CLI command for plugin sources.

```toml
id = "example-relay"              # [a-z0-9][a-z0-9-]{1,31}; unique; the pool account id
url = "http://127.0.0.1:7844"     # http(s) origin only: no credentials, path, query or fragment
identity = "alice"                # the person's value the relay knows them by; sent as the credential

[providers.anthropic]             # anthropic | openai-codex | google
path = ""                         # appended to url; optional
auth = "claude_code"              # anthropic: claude_code | api_key | bearer
[providers.openai-codex]
path = "/backend"
auth = "codex_cli"                # openai-codex: codex_cli (no upstream account id)
[providers.google]
path = "/v1beta"
auth = "api_key"                  # google: api_key (x-goog-api-key) | bearer

[[session_headers]]               # at most 4
name = "x-example-task"           # lower-case x- header, never an auth header
template = "openside:{session}"   # {session}, {subagent}; ids reduced to [A-Za-z0-9._:-]
max_length = 64                   # 1-256, default 64

[probe]
path = "/health"                  # GET; 2xx = available
identity_header = "x-api-key"     # optional: send the identity on the probe
timeout_ms = 1500                 # 100-10000
interval_seconds = 60             # 10-3600 while available; failures retry after 10 s

[errors]
hint_header = "x-example-hint"    # required when a rule matches a hint
[[errors.rules]]                  # at most 16; first match wins
status = 429
hint = "unsupported-client"       # hint header value, case-insensitive
# body_contains = "..."           # error message substring, case-insensitive
outcome = "unsupported_client"    # unsupported_client | sign_in | capacity | rate_limit
message = "Optional explanation shown instead of the status text"
```

Unknown keys or out-of-range values reject the whole manifest; a duplicate `id` keeps the first file in name order; at most 32 files of up to 64 KiB are read.

**Trust.** A manifest is fully trusted: it decides where the identity and all model traffic of its accounts go, and its origin and probe address are not restricted (a relay is normally a private-network address). File trust is therefore the boundary. The daemon and `openside doctor` accept manifests only when the `plugins` directory is owned by the current user and not group- or other-writable, and each manifest is a regular file opened without following symlinks (`O_NOFOLLOW`; ownership and mode are checked on the opened descriptor) that is owned by the current user and grants group and other users no access at all (mode `600` or `400`), since it holds the identity. Anything else is rejected and counted in doctor's rejected-manifest count. The probe goes only to `url` + `probe.path`, sends the identity only there (when `identity_header` is set), and never follows redirects, so a redirect answer counts as unavailable.

**Availability.** The daemon re-reads the directory at most every 10 seconds and probes each manifest. It also tracks the local address the host would use to reach the relay origin (a connected UDP socket; no packet is sent), and a change of that address (joining or leaving a network) re-probes immediately instead of waiting for the interval. While the probe succeeds, every provider the manifest serves gets one account `plugin:<id>`; when it fails the account disappears.

**Routing.** A reachable plugin account is appended to the end of its provider's effective pool for credential resolution and report rotation, without being persisted. An explicitly empty pool stays disabled; a provider without a pool routes through plugin accounts alone. Plugin accounts never appear in `GET /v1/settings/pools`, catalog rows or `GET /v1/accounts`; a catalog model that only a plugin account can serve reports like any other available model (`available:true`, reason `Ready`) with no `source`/`account_id`. `PUT /v1/settings/pools/{provider}` refuses `plugin:` references (400). The identity is not registered as a redaction secret, because it is usually a login name that also appears in paths.

**Lease.** The lease carries the identity as `value`, the provider route as private `auth.base_url`, the expanded session headers as `auth.headers` and the rules as `auth.error_rules` (see [Private credential RPC](#private-credential-rpc)). The sidecar routes the attempt to `base_url`, admits exactly that origin through the egress guard, skips the Codex upstream-account-id requirement, and adds the `claude-code-20250219` beta to Claude Code-shaped requests that pi-ai sends without it (tool-less requests). Relay refusals are classified by the rules first: `unsupported_client` → `policy` (no retry on the account), `sign_in` → `auth`, `capacity` and `rate_limit` → `rate_limit` with the `Retry-After` delay. The rule's `message`, or a fixed default, replaces the status-only error text; response text is never surfaced.

**Diagnosis.** `openside doctor` reports `models.plugin_sources`: how many manifests loaded, how many origins accept a TCP connection now, their ids, and how many manifests were rejected. URLs and identities are not printed.

## Account pool & rotation

The daemon is the single selection, usage and rotation policy owner. The sidecar's custom `streamFn` (`agent/src/providers/stream.ts`) obtains a fresh private credential snapshot for every outbound model-request attempt; model objects may stay warm, but captured credentials do not remain authorized until the session ends. An already-sent request may finish. A later request observes removal, replacement, disablement, pool edits and refresh without restarting the conversation.

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

Resolve includes opaque request_id, provider, qualified model, zero-based attempt0–63 and nullable daemon-known subagent. Success carries a lease, generation, opaque account ref and selected auth `{kind,value,upstream_account_id,expires_at_ms}`; kind is api_key/claude_code/codex_cli/bearer. Plugin-source leases add private `base_url` (≤512-char http(s) URL without credentials, query or fragment), `headers` (≤8 lower-case `x-` headers, printable ASCII values ≤256, never an auth header) and `error_rules` (`{hint_header?, rules:[{status 400–599, hint?, body_contains?, outcome, message?}]}`, ≤16 rules); an invalid field rejects the whole lease. Only an API key/access token and necessary private request identity cross this boundary. No refresh token, home, broker bearer or complete source configuration is sent. Redacting Debug/repr and zeroized serialized buffers protect ordinary output/lifetime; no credential disk snapshot cache is added.

Report outcome is success/auth/rate_limit/quota/policy/transient/cancelled, with stream_started, nullable provider retry duration, account/model scope, sanitized usage counts/windows. Ack action is complete/refresh/rotate/stop. Report effects commit before ack, which must be awaited before reacquiring. Refresh/rotate ack carries no credential: the sidecar resolves again with incremented attempt under the same logical request id. Delayed/idempotent lease-bound reports cannot block a replacement credential or recreate an account. On cancellation/abort/shutdown only, send-only cancelled reports may discard their late ack; no resolve/retry follows, avoiding cleanup dispatcher deadlock.

Sidecar session and diagnostic paths do not read credential homes/refresh tokens/broker credential snapshots. Daemon-provided sanitized metadata supports diagnostics; standalone sidecar diagnostics may report installation/configuration, not fabricate login readiness. Credential-free curated model discovery remains in the sidecar (`models --json`).

Sidecar and SDK-index subprocesses start with `env_clear` and a runtime-only allowlist: process/runtime paths, HOME and XDG directories, locale/temp directories and `SSL_CERT_FILE`/`SSL_CERT_DIR`. Proxy variables are omitted because current network policy does not authorize inherited proxies. Known and custom credential environment names override this allowlist and remain daemon-only, including a custom name that happens to be HOME. The diagnostic input carries model identifiers/provider namespaces only; startup forwards only a bounded validated `anthropic.claude_code_version`, never source configuration. Disposable test script/stderr paths are explicitly allowed non-secret instrumentation.

The entire managed-subscriptions directory is protected from model file tools, artifact collection and sandboxed bash. Credential files discovered inside managed homes also enroll retained metadata-only CLOEXEC descriptors, so hardlink aliases and an unlinked but still-open old token file remain protected.

## Child crash protection

Only session-sidecar children receive soft/hard RLIMIT_CORE=0 before exec, plus Linux PR_SET_DUMPABLE=0. Because [Linux exec resets dumpability](https://man7.org/linux/man-pages/man2/execve.2.html), the built-in Bun child reapplies the Linux flag through `prctl` (`agent/src/main.ts`) before connecting/accepting credentials and fails closed if protection cannot be installed. This does not change daemon dump policy, erase every memory copy, or protect against privileged inspection/swap. A custom sidecar must reapply Linux dumpability after exec before receiving credentials.

## CLI and verification

`openside models` lists catalog/pool eligibility; `models set <model> --fallback <model>` selects defaults for new sessions. `models pool list` and `models pool set <provider> [source:account_id ...]` manage the explicit ordered trust boundary; an empty list disables the provider. `models account <session> [source:account_id]` sets/clears the next-request preference. `openside models accounts` lists all source accounts; `models accounts add <source> --label <label>`, `show`, `enable`, `disable` and `rm <source> <account_id>` manage subscriptions and print exact external login guidance only on the admin surface. (`openside account` is a different command: it picks the team agent a CLI session runs as.) `openside key set <provider> --label <label>` reads hidden stdin only; `--key-id <id>` replaces. `key update <provider> <key_id>` edits metadata and `key rm <provider> <key_id>` removes one account. No key argument is accepted; parse errors cannot echo an accidentally supplied key.

OAuth sign-ins use the same command with source `oauth` over `/v1/settings/oauth/*`: `models accounts providers` lists provider, name, methods and the methods that accept a pasted code; `add oauth --provider <provider> --label <label> [--method browser|device] [--pool]` creates a transaction (`--pool` sets `join_pool`; without it the account joins no pool and the CLI prints the `pi-oauth:<account_id>` reference for `models pool set`); `login oauth <provider>/<account_id> [--method ...]` signs in again; `show`, `enable`, `disable`, `rm oauth <provider>/<account_id>` and `update oauth <provider>/<account_id> [--label ...] [--reserve-percent N]` map to GET/PATCH/DELETE. `add` and `login` poll the transaction every second until it ends, write the authorization URL, instructions and user code to stderr once per change, and print only the account (JSON: the final transaction, whose `authorization` is cleared) on stdout. When the daemon sets `manual_input` and stdin is a terminal, the CLI asks for the code or redirect URL at a hidden prompt (Enter alone keeps waiting for the browser); otherwise it tells the person to finish in the browser. A code is never accepted in argv, and a parse error for `add oauth`/`login oauth` does not echo the arguments. Ctrl-C calls the transaction's cancel route and exits 130. Failed, cancelled and expired transactions exit 1 with a short message and the reason code.

Acceptance requires multiple same-provider keys and isolated CLI homes; person/scope refusal; no key/token in public/config/log output; explicit-pool fail-close and affinity; auth versus rate/quota classification; bounded distinct-account retry and no partial-stream replay; single-flight refresh/revision race protection; default-home read-only behavior and managed-file wipe; deleted/disabled accounts observed by later requests; generation/startup fencing; and spawned child core/dumpability assertions. Disposable daemon smoke and scripted/mocked upstream boundaries avoid touching live installations/accounts. No live upstream entitlement or OAuth login claim follows from metadata readiness alone.
