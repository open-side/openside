# API reference

Generated from `docs/specs/openapi.json` by `tools/gen-api-docs.py`; do not edit by hand (`tools/gen-docs.sh` regenerates it). openside API v1.

## Authentication

- `capabilityToken` (http, bearer): Device token from POST /v1/devices/pair or `openside token create --scope <scope>`. Each operation's x-required-capability names the scope it needs.

## Endpoints

| Group | Operations |
|---|---|
| access-requests | 3 |
| accounts | 1 |
| agents | 9 |
| browser | 2 |
| connections | 10 |
| daemon | 2 |
| devices | 5 |
| doctor | 1 |
| extension | 1 |
| external-agents | 4 |
| favicons | 1 |
| health | 1 |
| hooks | 1 |
| import | 9 |
| memory | 10 |
| onboarding | 2 |
| otp | 1 |
| pairing | 2 |
| pinned-tabs | 3 |
| policy | 6 |
| push | 5 |
| remote | 5 |
| routines | 8 |
| sessions | 26 |
| settings | 22 |
| skills | 5 |
| sync | 6 |
| team | 2 |
| tickets | 6 |
| tools | 1 |
| vault | 9 |

## access-requests

### GET /v1/access-requests

Required capability: `admin`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [AccessRequestList](#accessrequestlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/access-requests/{id}/approve

Required capability: `admin`

Grants the requested scopes to the device, in addition to the ones it holds. Requires the same local-person authority as pairing (local socket, or verified loopback TCP with a socket-issued person_device admin token and the current X-Openside-Local-Secret); a QR-paired, agent or proxied caller gets 403 local_access_required. 404 not_found for an unknown, expired or already decided request.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |
| `X-Openside-Local-Secret` | header | string | no | Per-boot local-person proof obtained only over the local socket. Required for TCP pairing/configuration; never grants authority to a QR-paired, agent or proxied caller. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Device](#device) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/access-requests/{id}/deny

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## accounts

### GET /v1/accounts

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [AccountsResponse](#accountsresponse) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## agents

### GET /v1/agents

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [AgentList](#agentlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/agents

Required capability: `admin`

Request body (required): `application/json` [HireDraft](#hiredraft)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [Agent](#agent) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/agents/{id}

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Agent](#agent) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PATCH /v1/agents/{id}

Required capability: `steer`

Edit the profile for the next turn. Omitted nullable overrides are unchanged; explicit null inherits defaults. Permission widening and skills/model/account-pool/channel changes additionally require Admin and a person device (local socket exempt). Immutable identity and home fields cannot be patched.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

Request body (required): `application/json` [UpdateAgentRequest](#updateagentrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Agent](#agent) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/agents/{id}/handoffs

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [HandoffList](#handofflist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/agents/{id}/handoffs

Required capability: `steer`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

Request body (required): `application/json` [CreateHandoffRequest](#createhandoffrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Handoff](#handoff) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/agents/{id}/restore

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Agent](#agent) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/agents/{id}/retire

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

Request body (required): `application/json` [RetireAgentRequest](#retireagentrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Agent](#agent) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/agents/{id}/transfer

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

Request body (required): `application/json` [TransferRequest](#transferrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Agent](#agent) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## browser

### POST /v1/browser/prewarm

Required capability: `steer`

Launch the browser ahead of a new chat's first message. Idempotent; returns before the browser is up.

| Status | Description | Body |
|---|---|---|
| 202 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/browser/tabs

Required capability: `read`

Metadata-only ledger of explicitly agent-owned targets with a known non-incognito owner. Omits user/external targets and removes URL credentials, query and fragment. Empty before browser launch; never launches or reconnects the browser.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [BrowserTabList](#browsertablist) |
| 503 | The previously started browser controller is disconnected | `application/json` [ApiError](#apierror) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## connections

### GET /v1/connections

Required capability: `admin`

Admin-only sign-in metadata. Cookie/header values and identity HMACs are never returned. Missing rows return connection_not_found; grant/policy refusal returns connection_denied; unavailable browser returns connection_browser_unavailable. Same-site URL validation returns connection_cross_site.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `agent` | query | string | no | Exclude rows explicitly denied to this agent. |
| `status` | query | [ConnectionStatus](#connectionstatus) | no |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ConnectionList](#connectionlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/connections/audit

Required capability: `admin`

Admin-only sign-in metadata. Cookie/header values and identity HMACs are never returned. Missing rows return connection_not_found; grant/policy refusal returns connection_denied; unavailable browser returns connection_browser_unavailable. Same-site URL validation returns connection_cross_site.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `connection` | query | string | no |  |
| `agent` | query | string | no |  |
| `action` | query | [ConnectionAuditAction](#connectionauditaction) | no |  |
| `limit` | query | integer | no |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ConnectionAuditList](#connectionauditlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/connections/candidates/{id}/dismiss

Required capability: `admin`

Admin-only sign-in metadata. Cookie/header values and identity HMACs are never returned. Missing rows return connection_not_found; grant/policy refusal returns connection_denied; unavailable browser returns connection_browser_unavailable. Same-site URL validation returns connection_cross_site.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/connections/spaces

Required capability: `admin`

Admin-only sign-in metadata. Cookie/header values and identity HMACs are never returned. Missing rows return connection_not_found; grant/policy refusal returns connection_denied; unavailable browser returns connection_browser_unavailable. Same-site URL validation returns connection_cross_site.

Request body (required): `application/json` [CreateAccountSpaceRequest](#createaccountspacerequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [CreateAccountSpaceResult](#createaccountspaceresult) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/connections/spaces/{space_id}

Required capability: `admin`

Admin-only sign-in metadata. Cookie/header values and identity HMACs are never returned. Missing rows return connection_not_found; grant/policy refusal returns connection_denied; unavailable browser returns connection_browser_unavailable. Same-site URL validation returns connection_cross_site.

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/connections/{id}

Required capability: `admin`

Admin-only sign-in metadata. Cookie/header values and identity HMACs are never returned. Missing rows return connection_not_found; grant/policy refusal returns connection_denied; unavailable browser returns connection_browser_unavailable. Same-site URL validation returns connection_cross_site.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Connection](#connection) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PATCH /v1/connections/{id}

Required capability: `admin`

Admin-only sign-in metadata. Cookie/header values and identity HMACs are never returned. Missing rows return connection_not_found; grant/policy refusal returns connection_denied; unavailable browser returns connection_browser_unavailable. Same-site URL validation returns connection_cross_site.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

Request body (required): `application/json` [UpdateConnectionRequest](#updateconnectionrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Connection](#connection) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/connections/{id}/check

Required capability: `admin`

Admin-only sign-in metadata. Cookie/header values and identity HMACs are never returned. Missing rows return connection_not_found; grant/policy refusal returns connection_denied; unavailable browser returns connection_browser_unavailable. Same-site URL validation returns connection_cross_site.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Connection](#connection) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/connections/{id}/relogin

Required capability: `admin`

Start the existing login flow in the supplied shared session, or the chief agent's home session. Silent Vault completion returns signed_in. OTP, device approval and login continuations return needs_you with the matching existing ticket; other actionable states use a Finish signing in takeover ticket.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

Request body (required): `application/json` [ReloginConnectionRequest](#reloginconnectionrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ReloginResult](#reloginresult) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/connections/{id}/revoke

Required capability: `admin`

Admin-only sign-in metadata. Cookie/header values and identity HMACs are never returned. Missing rows return connection_not_found; grant/policy refusal returns connection_denied; unavailable browser returns connection_browser_unavailable. Same-site URL validation returns connection_cross_site.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

Request body (required): `application/json` [RevokeConnectionRequest](#revokeconnectionrequest)

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## daemon

### GET /v1/daemon

Required capability: `read`

Identity of the running process: version, a random `boot_id` that changes on every start, when it started and how a restart would be carried out. A client that asked for a restart waits until `boot_id` changes.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [DaemonStatus](#daemonstatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/daemon/restart

Required capability: `admin`

Restart the daemon by graceful shutdown and in-place exec with the same PID and arguments, including under systemd or launchd. Returns 202 with the current boot_id; wait for a new boot_id to reconnect. Requires local access (403 local_access_required). A private single-use 0600 handoff in the 0700 data directory preserves the launcher secret only for this controlled exec, bound to the old boot_id and restart request id by an exec-only environment marker, with a five-minute TTL. Ordinary external or service-manager restarts rotate the secret.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket. |

| Status | Description | Body |
|---|---|---|
| 202 | Success | `application/json` [RestartDaemonResponse](#restartdaemonresponse) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## devices

### GET /v1/devices

Required capability: `admin`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [DeviceList](#devicelist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/devices/me

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [MyDevice](#mydevice) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/devices/me/access-requests

Required capability: `read`

A person's device asks for scopes it does not hold, for example admin to manage the vault. The request waits ten minutes for someone on the computer running openside to allow or decline it; a new request from the same device replaces the old one. 400 invalid_value when the device already holds every requested scope; 403 person_approval_required for an agent client.

Request body (required): `application/json` [RequestAccessRequest](#requestaccessrequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [AccessRequest](#accessrequest) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/devices/pair

Required capability: `admin`

Only callable over the Unix socket, whose callers are implicitly admin (local user); TCP callers get 403 local_socket_required even with an admin token, so a leaked token cannot mint further tokens. Grant the narrowest scopes: read, steer, approve, human_step, browse, vault_use, otp_ingest or admin. admin grants every scope and steer also grants browse; no other scope implies another.

Request body (required): `application/json` [PairDeviceRequest](#pairdevicerequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [PairDeviceResponse](#pairdeviceresponse) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/devices/{id}

Required capability: `admin`

Revoke a device token. The next request with it gets 401, its open SSE streams end, and its push subscriptions are deleted.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## doctor

### GET /v1/doctor

Required capability: `admin`

Run the diagnostics of `openside doctor` inside the daemon and return them grouped by area, with a web fix for findings that have one. The vault probe reads only the file header and the keyring state. One report at a time; callers wait for the run in progress.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [DoctorReport](#doctorreport) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## extension

### GET /v1/extension/import/{ticket}

Required capability: `read`

Paired device-token fetch of its one-use native delivery ticket. HMAC-SHA256 authenticates ticket \|\| batch_id \|\| hex(SHA256(exact UTF-8 payload)) with the32-byte secret established only at locally authorized, pinned-extension-origin redemption. Payload never goes through a web page; owned cookie/payload buffers are zeroized where feasible, with JS/HTTP/WebSocket/library residuals. The extension verifies before native writes.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ImportDeliverySigned](#importdeliverysigned) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## external-agents

### GET /v1/external-agents

Required capability: `admin`

The agent CLIs openside can install its browser skill for: the `claude`, `codex` and `omp` presets, then any agent whose skills folder was added with `PUT /v1/external-agents/{id}` (stored as a `[skills.agents]` entry). `detected` means the executable is on the daemon's PATH or the agent's configuration directory exists; the skills directory follows the same rules as `openside skills install`.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ExternalAgentList](#externalagentlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PUT /v1/external-agents/{id}

Required capability: `admin`

Add or change the skills folder of an agent that is not a preset. Requires local access (the computer's own user, or its launcher with X-Openside-Local-Secret): 403 local_access_required otherwise. The id is lowercase letters, digits and hyphens and not a preset name (400 invalid_agent); the folder must be absolute or `~/…`, without `..`, and resolve inside the user's home, config or data folders (400 invalid_skills_dir). Persisted as `[skills.agents]` in config.toml and effective at once; audited.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

Request body (required): `application/json` [SetExternalAgentRequest](#setexternalagentrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ExternalAgent](#externalagent) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/external-agents/{id}

Required capability: `admin`

Forget a custom agent's skills folder. Requires local access. Presets cannot be removed (400 invalid_agent); 404 unknown_agent. Installed skill files are not touched.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ExternalAgentList](#externalagentlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/external-agents/{id}/browser-skill

Required capability: `admin`

Install the bundled `openside-browser` skill into the agent's skills directory (what `openside skills install <agent>` does). The content is fixed and the directory comes from the preset or configuration, never from the request. 404 unknown_agent, 409 exists (retry with `force`), 400 skills_dir_unknown (add the folder with `PUT /v1/external-agents/{id}`).

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

Request body (required): `application/json` [InstallAgentSkillRequest](#installagentskillrequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [InstalledAgentSkill](#installedagentskill) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## favicons

### GET /v1/favicons/{origin}

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## health

### GET /v1/health

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Health](#health) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## hooks

### POST /v1/hooks/{kind}/{account_id}

Required capability: `read`

Public signed platform callback for one managed identity, not bearer authentication. Slack events/interactivity use v0 HMAC-SHA256; Discord uses strict Ed25519 verification. Both signed timestamps must be within five minutes. Exact-body replay detection is per account with five-minute monotonic expiry. Bodies are capped at 256 KiB with a ten-second read deadline; each account admits 120 requests per rolling minute. Unknown or inactive accounts return sanitized 404 without Vault or platform lookup. OS keyring idle lock does not stop service identities; an explicit openside Vault lock does.

`x-public`: `true`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `kind` | path | `"slack"` \| `"discord"` | yes |  |
| `account_id` | path | string | yes |  |
| `X-Slack-Request-Timestamp` | header | string | no | Required by the selected platform's signing protocol. |
| `X-Slack-Signature` | header | string | no | Required by the selected platform's signing protocol. |
| `X-Signature-Timestamp` | header | string | no | Required by the selected platform's signing protocol. |
| `X-Signature-Ed25519` | header | string | no | Required by the selected platform's signing protocol. |

Request body (required): `application/json` object; `application/x-www-form-urlencoded` object

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` object |
| 401 | channel_bad_signature | `application/json` [ApiError](#apierror) |
| 404 | channel_account_not_found | `application/json` [ApiError](#apierror) |
| 408 | channel_body_timeout | `application/json` [ApiError](#apierror) |
| 409 | channel_replay | `application/json` [ApiError](#apierror) |
| 413 | channel_body_too_large | `application/json` [ApiError](#apierror) |
| 429 | channel_rate_limited | `application/json` [ApiError](#apierror) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## import

### POST /v1/import

Required capability: `admin`

Start importing selected profiles and kinds into the agent's default browser profile; omitted selections mean everything from every detected profile. Requires local-person authority. Passwords go only into an unlocked openside Vault; 409 vault_setup_required or vault_locked before writes. This explicit run is the credential-access phase: macOS may show a Keychain permission request; Linux never requests unlocking. Returns an asynchronous job; progress is available through job polling or job SSE. One write at a time (409 import_in_progress). Keyring/encryption failures skip only the affected kind and are included in the summary.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket; paired, agent and proxied callers cannot use it. |

Request body (required): `application/json` [ImportRequest](#importrequest)

| Status | Description | Body |
|---|---|---|
| 202 | Success | `application/json` [ImportJob](#importjob) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/import/delivery

Required capability: `admin`

Local-person-only, value-free pending revision descriptors. Returns batch IDs and exact cookie permission origins, never bookmarks/history records or cookie values. Mint a device-bound single-use ticket for the paired extension; only the extension fetches authenticated native payloads. Agent cookies and read-only side-store destinations are already applied.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket; paired, agent and proxied callers cannot use it. |
| `target` | query | [ImportDeliveryTarget](#importdeliverytarget) | no | Independent migration destination; defaults to the dedicated desktop user profile. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ImportDeliveryList](#importdeliverylist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/import/delivery/{id}/ack

Required capability: `admin`

Acknowledge one migration revision only after the selected destination applied it. Targets acknowledge independently; retries are idempotent. Requires the same local-person authority as importing browser data.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket; paired, agent and proxied callers cannot use it. |

Request body (required): `application/json` [ImportDeliveryAck](#importdeliveryack)

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/import/delivery/{id}/ticket

Required capability: `admin`

Local-person-only authorization of a pending revision for one import-enabled extension device. Ticket expires after120 seconds and is consumed once by that device; not a cookie or data payload. A retry needs a new ticket and uses native-store idempotency.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket; paired, agent and proxied callers cannot use it. |

Request body (required): `application/json` [ImportDeliveryTicketRequest](#importdeliveryticketrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ImportDeliveryTicket](#importdeliveryticket) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/import/jobs/{id}

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket; paired, agent and proxied callers cannot use it. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ImportJob](#importjob) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/import/jobs/{id}/events

Required capability: `admin`

Replay durable per-job progress then stream new progress with SSE event import_progress and monotonically increasing event ids. Last-Event-ID resumes a stream; final completed/failed progress closes it. No secret data is included; local-person authority is required.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |
| `Last-Event-ID` | header | string | no | Last seen sequence in this stream; only later events are returned. |
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket; paired, agent and proxied callers cannot use it. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `text/event-stream` string; each event is [ImportProgress](#importprogress) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/import/passwords

Required capability: `admin`

Import browser-exported CSV passwords directly from an in-memory request into the encrypted Vault. Accepts Chrome, Edge, Firefox and Safari URL/username/password headers; never stores the CSV file or returns values. Dedupes normalized origin and username through Vault save_login. Requires local-person authority and an unlocked Vault (409 vault_setup_required, vault_locked).

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket; paired, agent and proxied callers cannot use it. |

Request body (required): `application/json` [ImportPasswordsRequest](#importpasswordsrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ImportCategoryReport](#importcategoryreport) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/import/preview

Required capability: `admin`

Preview selected local profiles without target writes or browser startup; omitted selections mean all profiles and kinds. Cookie/password counts come only from source rows or binary record headers: no decryption, OS-keyring access, Unlock or credential verification. No unlocked Vault is required. Notes explain that the system may ask for access during the explicitly started import; actual counts can decrease after expired/unsupported/duplicate filtering. Other kinds are parsed and URL-sanitized normally. Requires local-person authority.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket; paired, agent and proxied callers cannot use it. |

Request body (required): `application/json` [ImportRequest](#importrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ImportBatchReport](#importbatchreport) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/import/profiles

Required capability: `admin`

Detect all browser profiles in standard local locations, including Chromium brands, Firefox forks and Safari on macOS. Includes per-kind availability, local display metadata and permission warnings; never source paths. Requires local-person authority.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket; paired, agent and proxied callers cannot use it. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ImportProfileList](#importprofilelist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## memory

### GET /v1/memory

Required capability: `read`

Hybrid search: FTS5 bm25 and local-embedding cosine ranks fused by reciprocal rank (k=60); lexical-only without a local model. An empty q lists recent memories.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `owner` | query | string | no | Filter by an exact agent id, or shared for notes without an owner; omitted includes all owners. |
| `q` | query | string | no | Search text; omit to list the most recently updated memories. |
| `kind` | query | [MemoryKind](#memorykind) | no | Only memories of this kind. |
| `limit` | query | integer | no | Maximum hits; defaults to 20, capped at 100. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [MemorySearchResponse](#memorysearchresponse) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/memory

Required capability: `admin`

Create a note. Content, tags or values matching secret patterns (API keys, tokens, private keys, JWTs, password= pairs, Luhn-valid card numbers, unlocked vault values) are rejected with 400 secret_detected and never stored.

Request body (required): `application/json` [CreateMemoryRequest](#creatememoryrequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [Memory](#memory) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/memory/health

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [MemoryHealth](#memoryhealth) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/memory/model

Required capability: `read`

The local embedding model and the download job: whether both model files exist, the stage and byte progress of the current or last download, its error code and whether a restart is needed before vector search starts. The daemon downloads only when asked; nothing runs implicitly.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [MemoryModelStatus](#memorymodelstatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/memory/model/cancel

Required capability: `admin`

Stop the running download; the installed model and the staged files are left untouched. A no-op when nothing is running.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [MemoryModelStatus](#memorymodelstatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/memory/model/download

Required capability: `admin`

Start downloading the embedding model into `memory.embedding.model_dir`: the file list and per-file checksums come from the model hub (SHA-256 for weights, git blob id for plain files), every request and redirect must pass `network.allow`, both files must match their checksum and load as a static model, and only then is the model swapped in atomically. One download at a time (409 download_in_progress). An empty body fetches the default model from the default source; naming a repo, revision or endpoint needs local access (403 local_access_required). 400 invalid_model_source; the outcome (including checksum_mismatch, checksum_unavailable, egress_refused, download_failed, unsupported_model) is reported by GET /v1/memory/model.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof; only needed to name a different repo, revision or endpoint. |

Request body (required): `application/json` [StartMemoryModelRequest](#startmemorymodelrequest)

| Status | Description | Body |
|---|---|---|
| 202 | Success | `application/json` [MemoryModelStatus](#memorymodelstatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/memory/{id}

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Memory](#memory) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PATCH /v1/memory/{id}

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

Request body (required): `application/json` [UpdateMemoryRequest](#updatememoryrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Memory](#memory) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/memory/{id}

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Memory](#memory) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/memory/{id}/history

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [MemoryHistory](#memoryhistory) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## onboarding

### GET /v1/onboarding

Required capability: `read`

State of the first-run guide, kept on the daemon so every device agrees: `pending` until someone finishes or skips it once.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [OnboardingStatus](#onboardingstatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PUT /v1/onboarding

Required capability: `admin`

Record how the guide ended (`completed` or `skipped`; `pending` is 400 invalid_value). Reopening the guide from Settings does not reset the flag.

Request body (required): `application/json` [UpdateOnboardingRequest](#updateonboardingrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [OnboardingStatus](#onboardingstatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## otp

### POST /v1/otp

Required capability: `otp_ingest`

Forwarded SMS. 403 sender_not_allowed unless the sender is in otp.senders. A code extracted by an otp.sites pattern fills pending secret_input tickets of that exact origin, or is kept in memory for otp.ttl_secs for a step that opens later. Codes are never returned, logged or persisted.

Request body (required): `application/json` [OtpIngestRequest](#otpingestrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [OtpIngestResponse](#otpingestresponse) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## pairing

### POST /v1/pairing

Required capability: `admin`

Issue a one-time pairing code valid for 180 monotonic seconds. Requires the local socket, or verified loopback TCP with a socket-issued person_device admin token and current X-Openside-Local-Secret. Proxied/relay traffic is refused with 403 local_access_required; loopback alone never proves locality. Requested scopes cannot exceed caller grants (403 scope_ceiling_exceeded). The local operator fixes person_device or agent_client in the offer; redemption cannot change kind. Codes and local secrets are never audited.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot local-person proof obtained only over the local socket. Required for TCP pairing/configuration; never grants authority to a QR-paired, agent or proxied caller. |

Request body (required): `application/json` [CreatePairingRequest](#createpairingrequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [PairingOffer](#pairingoffer) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/pairing/exchange

Required capability: `admin`

No bearer token: the one-time code is the credential. The token inherits the operator-selected kind from the offer; sending kind at redemption is rejected as an unknown field. 404 pairing_not_found for an unknown or already used code, 410 pairing_expired after 180 seconds. A valid request consumes the code on its first exchange attempt.

`x-public`: `true`

Request body (required): `application/json` [ExchangePairingRequest](#exchangepairingrequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [PairDeviceResponse](#pairdeviceresponse) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## pinned-tabs

### GET /v1/pinned-tabs

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [PinnedTabList](#pinnedtablist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/pinned-tabs

Required capability: `admin`

Request body (required): `application/json` [CreatePinnedTabRequest](#createpinnedtabrequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [PinnedTab](#pinnedtab) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/pinned-tabs/{id}

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## policy

### GET /v1/policy

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [PolicySettings](#policysettings) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PUT /v1/policy

Required capability: `admin`

Replace the live policy. Invalid globs or a sensitive allow rule return 400 invalid_policy and change nothing; POST /v1/policy/validate reports which entry is wrong. Requires local access (403 local_access_required): the policy decides what agents may do, so it is edited from the computer itself. Applies to the next tool call of every session and is persisted in <data_dir>/policy.json, which overrides [policy] in config.toml.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket. |

Request body (required): `application/json` [PolicySettings](#policysettings)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [PolicySettings](#policysettings) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/policy/network

Required capability: `read`

The saved outbound allow-list (`network.allow`) and whether the running daemon still enforces an older one.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [NetworkPolicy](#networkpolicy) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PUT /v1/policy/network

Required capability: `admin`

Replace `network.allow` in config.toml (validated like the file; 400 invalid_network_allow). Requires local access; it applies after the daemon restarts. Audited.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket. |

Request body (required): `application/json` [UpdateNetworkPolicyRequest](#updatenetworkpolicyrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [NetworkPolicy](#networkpolicy) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/policy/test

Required capability: `read`

Evaluate one capability/target against the live policy without running anything.

Request body (required): `application/json` [PolicyTestRequest](#policytestrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [PolicyDecision](#policydecision) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/policy/validate

Required capability: `read`

Check an edit of the live policy and/or the network allow-list without saving it. Returns every problem with the list and zero-based index of the entry, so a form can mark the exact row; PUT /v1/policy and PUT /v1/policy/network apply the same checks.

Request body (required): `application/json` [ValidatePolicyRequest](#validatepolicyrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [PolicyValidation](#policyvalidation) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## push

### POST /v1/push/presence

Required capability: `read`

focused=true suppresses pushes to that subscription for 60 seconds unless renewed; focused=false ends it. 404 unknown_subscription for another device's subscription.

Request body (required): `application/json` [PushPresenceRequest](#pushpresencerequest)

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/push/subscriptions

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [PushSubscriptionList](#pushsubscriptionlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/push/subscriptions

Required capability: `read`

Request body (required): `application/json` [CreatePushSubscriptionRequest](#createpushsubscriptionrequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [PushSubscription](#pushsubscription) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/push/subscriptions/{id}

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/push/vapid

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [VapidPublicKey](#vapidpublickey) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## remote

### PUT /v1/remote/cloudflare

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot local-person proof obtained only over the local socket. Required for TCP pairing/configuration; never grants authority to a QR-paired, agent or proxied caller. |

Request body (required): `application/json` [SetCloudflareRequest](#setcloudflarerequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [RemoteStatus](#remotestatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/remote/local-access

Required capability: `admin`

Local socket only: every call atomically replaces the one managed owner-bootstrap device/token, closes the predecessor's streams/polls, clears its push subscriptions, and audits rotation. A distinct persistent bootstrap marker survives boots; intentional CLI devices are never revoked by display name. Returns the per-boot locality secret and fresh person/admin token plus a loopback launch URL with credentials in its fragment. TCP/relay callers get 403 local_socket_required. Keep the proof only in sessionStorage; strip the fragment immediately and clear proof on sign-out, rejection or identity change.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [LocalAccess](#localaccess) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PUT /v1/remote/relay

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot local-person proof obtained only over the local socket. Required for TCP pairing/configuration; never grants authority to a QR-paired, agent or proxied caller. |

Request body (required): `application/json` [SetRelayRequest](#setrelayrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [RemoteStatus](#remotestatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/remote/status

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot local-person proof obtained only over the local socket. Required for TCP pairing/configuration; never grants authority to a QR-paired, agent or proxied caller. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [RemoteStatus](#remotestatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PUT /v1/remote/tailscale

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot local-person proof obtained only over the local socket. Required for TCP pairing/configuration; never grants authority to a QR-paired, agent or proxied caller. |

Request body (required): `application/json` [ConfigureTailscaleRequest](#configuretailscalerequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [RemoteStatus](#remotestatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## routines

### GET /v1/routines

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [RoutineList](#routinelist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/routines

Required capability: `admin`

Request body (required): `application/json` [CreateRoutineRequest](#createroutinerequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [Routine](#routine) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/routines/suggestions

Required capability: `read`

Draft routines inferred from repeated user messages; nothing is created.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [RoutineSuggestionList](#routinesuggestionlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/routines/{id}

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Routine](#routine) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PATCH /v1/routines/{id}

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

Request body (required): `application/json` [UpdateRoutineRequest](#updateroutinerequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Routine](#routine) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/routines/{id}

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Routine](#routine) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/routines/{id}/run

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [RunRecord](#runrecord) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/routines/{id}/runs

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |
| `limit` | query | integer | no | Maximum runs to return, newest first; defaults to 100, capped at 500. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [RunList](#runlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## sessions

### GET /v1/sessions

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `agent` | query | string | no | Filter immutable agent ownership: none selects earlier untagged conversations; an agent id selects all its threads. |
| `q` | query | string | no | Case-insensitive substring of the title or first message. |
| `archived` | query | boolean | no | List archived (true) or unarchived (false, default) sessions; pinned sessions come first. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SessionList](#sessionlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sessions

Required capability: `steer`

Create a session. browser_space "isolated" gives its agent tabs their own cookies and storage in the shared managed browser, disposed on archive or delete and lost when the browser restarts; it cannot be combined with incognito (400 invalid_browser_space).

Request body (required): `application/json` [CreateSessionRequest](#createsessionrequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [Session](#session) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/sessions/{id}

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Session](#session) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PATCH /v1/sessions/{id}

Required capability: `steer`

Update title/pinned/archived as a durable session.updated event. Archiving an isolated-space session disposes its browser space.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |

Request body (required): `application/json` [UpdateSessionRequest](#updatesessionrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Session](#session) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/sessions/{id}

Required capability: `admin`

Stop the session, record a durable session.deleted tombstone and hide it; later requests for the id return 404.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PUT /v1/sessions/{id}/account

Required capability: `steer`

Set/clear a preferred sticky account for the next request. Requires steer and membership in the existing effective global/Agent pool; preference never bypasses disablement, quota blocks or Agent restrictions. Invalid account400 account_not_in_pool.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |

Request body (required): `application/json` [UpdateSessionAccountRequest](#updatesessionaccountrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SessionAccountSelection](#sessionaccountselection) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/sessions/{id}/artifacts

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ArtifactList](#artifactlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/sessions/{id}/artifacts/{path}

Required capability: `read`

Raw artifact bytes for download: Content-Disposition attachment, X-Content-Type-Options nosniff and a sandbox CSP, so the response never renders in the daemon origin.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |
| `path` | path | string | yes | Artifact path relative to the session artifact directory, percent-encoded as one segment (/ as %2F). |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/octet-stream` string (binary) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/sessions/{id}/artifacts/{path}/preview

Required capability: `read`

Sandboxed rendering of an HTML, SVG, image, PDF or text artifact for an iframe. No bearer token: the exp/sig query from a listed preview_url is the credential (HMAC over session, path and expiry, valid 10 minutes, per daemon run). Responses carry Content-Security-Policy sandbox (scripts only for HTML, never allow-same-origin) with default-src 'none', so content runs in an opaque origin without network access. 400 invalid_preview_signature (missing, wrong or expired signature), 415 preview_unsupported.

`x-public`: `true`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |
| `path` | path | string | yes | Artifact path relative to the session artifact directory, percent-encoded as one segment (/ as %2F). |
| `exp` | query | string | yes |  |
| `sig` | query | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `*/*` string (binary) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/sessions/{id}/browser/preview

Required capability: `browse`

Read-only WebSocket preview of the session's current owned tab. Requires browse (steer implies browse), since JPEG pixels cannot be redacted. JPEG quality 40, at most 720x450 and 2 fps delivery; capture has a bounded startup allowance of at most 3 frames, then sustained 2 fps. Client messages are ignored and limited to 1 KiB. Secret-tainted tabs are withheld; takeover pauses the preview and takes priority. At most one frame admitted before a safety transition may finish sending within the 5-second writer deadline; later captured frames are excluded. Private sessions return 409 private_session; at most 8 viewers per session (429 too_many_viewers). Device revocation ends its sockets; committed stop/deletion ends existing viewers even if the session immediately resumes. Origin and browser bearer subprotocol rules match takeover.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |

| Status | Description | Body |
|---|---|---|
| 101 | Switching to read-only WebSocket |  |
| 409 | Private session | `application/json` [ApiError](#apierror) |
| 429 | Too many preview viewers | `application/json` [ApiError](#apierror) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/sessions/{id}/browser/thumbnails/{thumbnail_id}

Required capability: `browse`

Read a bounded JPEG thumbnail referenced by browser.state. Requires browse; secret-tainted tabs, changed documents, takeover, private/stopped/archived/deleted sessions, expired IDs and IDs from another session are withheld. IDs expire after ten minutes and after daemon restart; responses are never cached.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |
| `thumbnail_id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `image/jpeg` string (binary) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/sessions/{id}/events

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |
| `Last-Event-ID` | header | string | no | Last seen sequence in this stream; only later events are returned. |
| `subagents` | query | boolean | no | Set false to omit subagent.* events from the stream. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `text/event-stream` string; each event is [Event](#event) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/sessions/{id}/events/poll

Required capability: `read`

Bounded long polling for transports that buffer SSE. At most 512 raw durable events per page, ordered by seq; last_seq is the exclusive replay cursor and includes filtered subagent events. Subscribe-before-read prevents replay/live gaps. Pending requests wake immediately on device revocation. Delivers the deletion tombstone as the final event, then 404. A cursor beyond an active session's durable log returns 400 invalid_event_id.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |
| `after` | query | integer | no |  |
| `wait` | query | integer | no |  |
| `subagents` | query | boolean | no |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [EventPoll](#eventpoll) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sessions/{id}/messages

Required capability: `steer`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |

Request body (required): `application/json` [SubmitMessageRequest](#submitmessagerequest)

| Status | Description | Body |
|---|---|---|
| 202 | Success | `application/json` [Message](#message) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sessions/{id}/prewarm

Required capability: `steer`

Start the session's agent process, launch the browser and open one blank agent tab its next browser_open_tab adopts, so the first reply and browser action skip cold starts. Idempotent; returns before the work finishes.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |

| Status | Description | Body |
|---|---|---|
| 202 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PATCH /v1/sessions/{id}/queue/{mid}

Required capability: `steer`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |
| `mid` | path | [MessageId](#messageid) | yes |  |

Request body (required): `application/json` [EditMessageRequest](#editmessagerequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Message](#message) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/sessions/{id}/queue/{mid}

Required capability: `steer`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |
| `mid` | path | [MessageId](#messageid) | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Message](#message) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sessions/{id}/queue/{mid}/move

Required capability: `steer`

Reorder a pending message inside its own lane: steering messages are delivered before follow-ups and a follow-up becomes a steer only through promote. position is zero-based and clamped to the lane length; a message that was already delivered or dropped returns 409.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |
| `mid` | path | [MessageId](#messageid) | yes |  |

Request body (required): `application/json` [MoveMessageRequest](#movemessagerequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Message](#message) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sessions/{id}/queue/{mid}/promote

Required capability: `steer`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |
| `mid` | path | [MessageId](#messageid) | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Message](#message) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sessions/{id}/repl

Required capability: `browse`

`x-capability-also-granted-by`: `["steer"]`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |

Request body (required): `application/json` [ReplRequest](#replrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ReplResponse](#replresponse) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sessions/{id}/stop

Required capability: `steer`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Session](#session) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/sessions/{id}/subagents

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SubagentList](#subagentlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sessions/{id}/subagents/{sid}/cancel

Required capability: `steer`

Cancel a running or waiting child; its pending tickets are rejected and the parent's delegate call returns a cancelled result. Cancelling a finished child returns 409.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |
| `sid` | path | string | yes | Subagent identifier from subagent.started. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Subagent](#subagent) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sessions/{id}/subagents/{sid}/steer

Required capability: `steer`

Queue steering text for the child's next model request; a waiting child receives it when it resumes.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |
| `sid` | path | string | yes | Subagent identifier from subagent.started. |

Request body (required): `application/json` [SteerSubagentRequest](#steersubagentrequest)

| Status | Description | Body |
|---|---|---|
| 202 | Success | `application/json` [Subagent](#subagent) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sessions/{id}/tools/{name}

Required capability: `browse`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |

Request body (required): `application/json` [ToolCallRequest](#toolcallrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ToolCallResponse](#toolcallresponse) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/sessions/{id}/visuals

Required capability: `read`

Visual blocks (fenced ```html visual or ```svg visual) from assistant.final events, each with its source text and a sandboxed preview_url.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [VisualList](#visuallist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## settings

### GET /v1/settings

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Settings](#settings) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/settings/channels

Required capability: `read`

Manage independent labelled channel identities. Read lists are metadata only. Mutations require admin and a person device or the local socket. Credentials are encrypted in the Vault, never returned, synced or exposed to browser fill. Account changes affect only that account; failed credentials never select another identity.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ChannelAccountList](#channelaccountlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/settings/channels/{kind}/accounts

Required capability: `admin`

Manage independent labelled channel identities. Read lists are metadata only. Mutations require admin and a person device or the local socket. Credentials are encrypted in the Vault, never returned, synced or exposed to browser fill. Account changes affect only that account; failed credentials never select another identity.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `kind` | path | string | yes |  |

Request body (required): `application/json` [AddChannelAccountRequest](#addchannelaccountrequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [ChannelAccount](#channelaccount) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PATCH /v1/settings/channels/{kind}/accounts/{account_id}

Required capability: `admin`

Manage independent labelled channel identities. Read lists are metadata only. Mutations require admin and a person device or the local socket. Credentials are encrypted in the Vault, never returned, synced or exposed to browser fill. Account changes affect only that account; failed credentials never select another identity.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `kind` | path | string | yes |  |
| `account_id` | path | string | yes |  |

Request body (required): `application/json` [UpdateChannelAccountRequest](#updatechannelaccountrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ChannelAccount](#channelaccount) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/settings/channels/{kind}/accounts/{account_id}

Required capability: `admin`

Manage independent labelled channel identities. Read lists are metadata only. Mutations require admin and a person device or the local socket. Credentials are encrypted in the Vault, never returned, synced or exposed to browser fill. Account changes affect only that account; failed credentials never select another identity.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `kind` | path | string | yes |  |
| `account_id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/settings/channels/{kind}/accounts/{account_id}/test

Required capability: `admin`

Manage independent labelled channel identities. Read lists are metadata only. Mutations require admin and a person device or the local socket. Credentials are encrypted in the Vault, never returned, synced or exposed to browser fill. Account changes affect only that account; failed credentials never select another identity.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `kind` | path | string | yes |  |
| `account_id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ChannelAccountTest](#channelaccounttest) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/settings/config

Required capability: `admin`

The whole config.toml as JSON, with defaults filled in. PUT validates exactly like loading the file (400 invalid_config with the offending key) and writes it atomically with mode 0600; the credentials section is file-only. Changes apply when the daemon restarts, except the live policy (/v1/policy). Who may change what: a caller with local access (the Unix socket, or the computer's own launcher on loopback with X-Openside-Local-Secret) may change any key. A paired person device with admin scope anywhere else may change only the behaviour keys listed in `remote_editable`; changing any other key returns 403 local_access_required, writes nothing and is audited as `refused_remote`. Agent clients never write. Security-sensitive keys (sandbox, network allow-list, remote transports, credentials sources, executables, paths, trust lists) are never remote-editable.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof that makes a loopback TCP caller local; paired devices elsewhere omit it. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SettingsDocument](#settingsdocument) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PUT /v1/settings/config

Required capability: `admin`

The whole config.toml as JSON, with defaults filled in. PUT validates exactly like loading the file (400 invalid_config with the offending key) and writes it atomically with mode 0600; the credentials section is file-only. Changes apply when the daemon restarts, except the live policy (/v1/policy). Who may change what: a caller with local access (the Unix socket, or the computer's own launcher on loopback with X-Openside-Local-Secret) may change any key. A paired person device with admin scope anywhere else may change only the behaviour keys listed in `remote_editable`; changing any other key returns 403 local_access_required, writes nothing and is audited as `refused_remote`. Agent clients never write. Security-sensitive keys (sandbox, network allow-list, remote transports, credentials sources, executables, paths, trust lists) are never remote-editable.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof that makes a loopback TCP caller local; paired devices elsewhere omit it. |

Request body (required): `application/json` [UpdateSettingsRequest](#updatesettingsrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SettingsDocument](#settingsdocument) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/settings/keys

Required capability: `admin`

Administrator-only labelled account metadata: provider, key_id, label, last_four, created_at, enabled and reserve_percent. No API path returns key material; read-scoped devices receive403.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ProviderKeyList](#providerkeylist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/settings/keys/{provider}

Required capability: `admin`

Requires admin plus person_device (local socket exempt). POST creates one labelled account; PUT replaces its key, PATCH edits metadata, DELETE removes only that account. Values arrive only in secret input bodies; no key in response/config/log/argv/env. Subsequent request snapshots observe changes; missing account404, locked Vault409.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `provider` | path | `"anthropic"` \| `"openai"` \| `"google"` | yes |  |

Request body (required): `application/json` [CreateProviderKeyRequest](#createproviderkeyrequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [ProviderKeyInfo](#providerkeyinfo) |
| 403 | Missing admin scope or person-device authority | `application/json` [ApiError](#apierror) |
| 409 | Vault unavailable, or an ordinary legacy item conflicts with the provider id | `application/json` [ApiError](#apierror) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PUT /v1/settings/keys/{provider}/{key_id}

Required capability: `admin`

Requires admin plus person_device (local socket exempt). POST creates one labelled account; PUT replaces its key, PATCH edits metadata, DELETE removes only that account. Values arrive only in secret input bodies; no key in response/config/log/argv/env. Subsequent request snapshots observe changes; missing account404, locked Vault409.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `provider` | path | `"anthropic"` \| `"openai"` \| `"google"` | yes |  |
| `key_id` | path | string | yes |  |

Request body (required): `application/json` [SetProviderKeyRequest](#setproviderkeyrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ProviderKeyInfo](#providerkeyinfo) |
| 403 | Missing admin scope or person-device authority | `application/json` [ApiError](#apierror) |
| 409 | Vault unavailable, or an ordinary legacy item conflicts with the provider id | `application/json` [ApiError](#apierror) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PATCH /v1/settings/keys/{provider}/{key_id}

Required capability: `admin`

Requires admin plus person_device (local socket exempt). POST creates one labelled account; PUT replaces its key, PATCH edits metadata, DELETE removes only that account. Values arrive only in secret input bodies; no key in response/config/log/argv/env. Subsequent request snapshots observe changes; missing account404, locked Vault409.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `provider` | path | `"anthropic"` \| `"openai"` \| `"google"` | yes |  |
| `key_id` | path | string | yes |  |

Request body (required): `application/json` [UpdateProviderKeyRequest](#updateproviderkeyrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ProviderKeyInfo](#providerkeyinfo) |
| 403 | Missing admin scope or person-device authority | `application/json` [ApiError](#apierror) |
| 409 | Vault unavailable, or an ordinary legacy item conflicts with the provider id | `application/json` [ApiError](#apierror) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/settings/keys/{provider}/{key_id}

Required capability: `admin`

Requires admin plus person_device (local socket exempt). POST creates one labelled account; PUT replaces its key, PATCH edits metadata, DELETE removes only that account. Values arrive only in secret input bodies; no key in response/config/log/argv/env. Subsequent request snapshots observe changes; missing account404, locked Vault409.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `provider` | path | `"anthropic"` \| `"openai"` \| `"google"` | yes |  |
| `key_id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| 403 | Missing admin scope or person-device authority | `application/json` [ApiError](#apierror) |
| 409 | Vault unavailable, or an ordinary legacy item conflicts with the provider id | `application/json` [ApiError](#apierror) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/settings/models

Required capability: `read`

Local model readiness grouped by provider, from configured ids, source accounts and curated chat/tool-capable models. Credential values, email and upstream account identities are never included. Metadata is cached for at most five seconds; key/source/Vault lifecycle writes invalidate it. Readiness is not an upstream entitlement guarantee.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ModelCatalog](#modelcatalog) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PUT /v1/settings/models

Required capability: `admin`

Set the default and ordered fallback ids without raw TOML editing. Requires admin; every id must be available in the current catalog (400 model_unavailable), at most16 distinct fallbacks and no default duplicate (400 invalid_models). Atomically persists selection without daemon restart. Ordinary sessions retain stored models; Team homes use their Agent override or the current default and reopen on the next idle/real turn when effective selection changes. Running requests finish their captured model. Response effective:new_sessions_and_home_next_turn.

Request body (required): `application/json` [UpdateModelsRequest](#updatemodelsrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ModelSelection](#modelselection) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/settings/pools

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ProviderPoolList](#providerpoollist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PUT /v1/settings/pools/{provider}

Required capability: `admin`

Replace the explicit ordered provider trust boundary. Requires admin plus person_device or local socket. Known compatible account refs only; no duplicates, at most64; empty disables routing. Unlisted sources never become automatic fallbacks.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `provider` | path | string | yes |  |

Request body (required): `application/json` [UpdateProviderPoolRequest](#updateproviderpoolrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ProviderPool](#providerpool) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/settings/subscriptions

Required capability: `admin`

Admin-only external CLI account inventory/status; mutations also require person_device (local socket exempt). POST creates an isolated private home and exact external login command; no openside OAuth sign-in flow. PATCH edits one account; DELETE wipes only managed homes and hides defaults without writing/deleting user homes. Managed OAuth is canonical encrypted Vault state; daemon-only refresh. Refresh/access tokens never enter public replies.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SubscriptionList](#subscriptionlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/settings/subscriptions/{source}

Required capability: `admin`

Admin-only external CLI account inventory/status; mutations also require person_device (local socket exempt). POST creates an isolated private home and exact external login command; no openside OAuth sign-in flow. PATCH edits one account; DELETE wipes only managed homes and hides defaults without writing/deleting user homes. Managed OAuth is canonical encrypted Vault state; daemon-only refresh. Refresh/access tokens never enter public replies.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `source` | path | `"claude-code"` \| `"codex-cli"` | yes |  |

Request body (required): `application/json` [CreateSubscriptionAccountRequest](#createsubscriptionaccountrequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [SubscriptionAccount](#subscriptionaccount) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/settings/subscriptions/{source}/{account_id}

Required capability: `admin`

Admin-only external CLI account inventory/status; mutations also require person_device (local socket exempt). POST creates an isolated private home and exact external login command; no openside OAuth sign-in flow. PATCH edits one account; DELETE wipes only managed homes and hides defaults without writing/deleting user homes. Managed OAuth is canonical encrypted Vault state; daemon-only refresh. Refresh/access tokens never enter public replies.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `source` | path | `"claude-code"` \| `"codex-cli"` | yes |  |
| `account_id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SubscriptionAccount](#subscriptionaccount) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PATCH /v1/settings/subscriptions/{source}/{account_id}

Required capability: `admin`

Admin-only external CLI account inventory/status; mutations also require person_device (local socket exempt). POST creates an isolated private home and exact external login command; no openside OAuth sign-in flow. PATCH edits one account; DELETE wipes only managed homes and hides defaults without writing/deleting user homes. Managed OAuth is canonical encrypted Vault state; daemon-only refresh. Refresh/access tokens never enter public replies.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `source` | path | `"claude-code"` \| `"codex-cli"` | yes |  |
| `account_id` | path | string | yes |  |

Request body (required): `application/json` [UpdateSubscriptionAccountRequest](#updatesubscriptionaccountrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SubscriptionAccount](#subscriptionaccount) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/settings/subscriptions/{source}/{account_id}

Required capability: `admin`

Admin-only external CLI account inventory/status; mutations also require person_device (local socket exempt). POST creates an isolated private home and exact external login command; no openside OAuth sign-in flow. PATCH edits one account; DELETE wipes only managed homes and hides defaults without writing/deleting user homes. Managed OAuth is canonical encrypted Vault state; daemon-only refresh. Refresh/access tokens never enter public replies.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `source` | path | `"claude-code"` \| `"codex-cli"` | yes |  |
| `account_id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## skills

### GET /v1/skills

Required capability: `read`

Skills visible to a session. With q this is the data source of the `/` mention picker: name-prefix matches first, then name, then description matches. Invalid skill directories are listed under problems instead of failing the request.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `q` | query | string | no | Case-insensitive text matched against name and description. |
| `session` | query | [SessionId](#sessionid) | no | Session whose workspace supplies project skills; without it only the user library is listed. |
| `enabled` | query | boolean | no | Only enabled (true) or only disabled (false) skills. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SkillList](#skilllist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/skills

Required capability: `admin`

Create a user skill from a name, description, Markdown body and optional text resources; the content is validated like any SKILL.md. 409 `exists` if the name is taken.

Request body (required): `application/json` [CreateSkillRequest](#createskillrequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [SkillDetail](#skilldetail) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/skills/{name}

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `name` | path | string | yes | Skill name. |
| `session` | query | [SessionId](#sessionid) | no | Session whose workspace supplies project skills. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SkillDetail](#skilldetail) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PATCH /v1/skills/{name}

Required capability: `admin`

Edit the description or body, or switch the skill on or off. Project skills are read-only (403 `read_only`).

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `name` | path | string | yes | Skill name. |
| `session` | query | [SessionId](#sessionid) | no | Session whose workspace supplies project skills. |

Request body (required): `application/json` [UpdateSkillRequest](#updateskillrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SkillDetail](#skilldetail) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/skills/{name}

Required capability: `admin`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `name` | path | string | yes | Skill name. |
| `session` | query | [SessionId](#sessionid) | no | Session whose workspace supplies project skills. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SkillDetail](#skilldetail) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## sync

### GET /v1/sync

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SyncStatus](#syncstatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PUT /v1/sync/config

Required capability: `admin`

Choose where synced data is stored: a shared directory or a WebDAV URL (https, or http on loopback), exactly one or neither to turn sync off, plus whether the live policy syncs and which memory tags sync. Requires local access. The backend of a device that already joined an account cannot change (409 sync_already_set_up). Persisted in `[sync]` of config.toml and effective at once without a restart. 400 invalid_sync_config.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket. |

Request body (required): `application/json` [UpdateSyncConfigRequest](#updatesyncconfigrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SyncStatus](#syncstatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sync/pair

Required capability: `admin`

Join the account of a pairing code with the same passphrase, typed in the body (needs local access) or read from sync.passphrase_cmd. 400 invalid_pairing_code, sync_wrong_passphrase; 403 local_access_required when a secret is sent without local access; 409 as setup; 502 sync_backend_unavailable, sync_unauthorized.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP when the body carries a passphrase or WebDAV password. |

Request body (required): `application/json` [PairSyncRequest](#pairsyncrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SyncStatus](#syncstatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sync/run

Required capability: `admin`

Sync now: seal local changes to the live policy, pinned tabs, vault items and tagged memories, pull and merge remote records (docs/specs/sync.md conflict rules), push winners with conditional writes. Tampered records are counted in `rejected` and never applied.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SyncReport](#syncreport) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sync/setup

Required capability: `admin`

Create a sync account for this device: derive the key from the passphrase in the body (typed in the app, memory only, needs local access) or from sync.passphrase_cmd, create the account collection on the WebDAV share or directory and return the pairing code for the second device. The body may be empty. 409 sync_not_configured, sync_already_set_up, sync_passphrase_unavailable, sync_password_unavailable; 403 local_access_required when a secret is sent without local access; 502 sync_backend_unavailable, sync_unauthorized.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP when the body carries a passphrase or WebDAV password. |

Request body (required): `application/json` [SyncSecrets](#syncsecrets)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [SyncSetupResponse](#syncsetupresponse) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sync/unlock

Required capability: `admin`

Derive the sync key for the account this device joined from a passphrase typed in the app (or from `sync.passphrase_cmd` when none is given) and keep it in daemon memory only. A WebDAV password typed here is likewise memory-only. Requires local access when a secret is sent. 400 sync_wrong_passphrase; 409 sync_not_set_up, sync_passphrase_unavailable.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `X-Openside-Local-Secret` | header | string | no | Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket. |

Request body (required): `application/json` [SyncSecrets](#syncsecrets)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SyncStatus](#syncstatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## team

### GET /v1/team/proposals/suppressed

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SuppressedProposalList](#suppressedproposallist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/team/proposals/{key}

Required capability: `admin`

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## tickets

### GET /v1/tickets

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `status` | query | [TicketStatus](#ticketstatus) | no | Return only tickets with the selected lifecycle status. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [TicketList](#ticketlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/tickets/{id}

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [TicketId](#ticketid) | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Ticket](#ticket) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/tickets/{id}/done

Required capability: `human_step`

Complete a device_approval or takeover ticket.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [TicketId](#ticketid) | yes |  |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Ticket](#ticket) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/tickets/{id}/reject

Required capability: `approve`

The daemon checks the ticket kind: confirm/info/choice require approve; secret_input/device_approval/takeover/login require human_step. A secret_input answer that registers an authenticator ({"value":{"totp":…}} on /resolve) and a login answer that keeps the login ({"value":{"values":…,"save":true}}) also require approve.

`x-capability-by-ticket-kind`: `{"choice": "approve", "confirm": "approve", "device_approval": "human_step", "info": "approve", "login": "human_step", "secret_input": "human_step", "takeover": "human_step"}`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [TicketId](#ticketid) | yes |  |

Request body (required): `application/json` [RejectTicketRequest](#rejectticketrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Ticket](#ticket) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/tickets/{id}/resolve

Required capability: `approve`

The daemon checks the ticket kind: confirm/info/choice require approve; secret_input/device_approval/takeover/login require human_step. A secret_input answer that registers an authenticator ({"value":{"totp":…}} on /resolve) and a login answer that keeps the login ({"value":{"values":…,"save":true}}) also require approve. Confirm approval, choice answers, TOTP enrollment, and login save:true require a person_device; agent clients and the Unix socket receive 403 person_approval_required and the ticket stays pending. A person device that directly created the ticket, or was recorded as its session driver, receives 403 self_approval_forbidden. Pairing fixes the device kind. Deny/reject and cancel remain allowed with the appropriate scope.

`x-capability-by-ticket-kind`: `{"choice": "approve", "confirm": "approve", "device_approval": "human_step", "info": "approve", "login": "human_step", "secret_input": "human_step", "takeover": "human_step"}`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [TicketId](#ticketid) | yes |  |

Request body (required): `application/json` [ResolveTicketRequest](#resolveticketrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Ticket](#ticket) |
| 403 | Caller lacks the ticket scope or the required person-device authority, or tries to approve its own ticket | `application/json` [ApiError](#apierror) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/tickets/{id}/takeover

Required capability: `human_step`

WebSocket upgrade for a pending takeover ticket whose target has a tab_id. The daemon streams JPEG frames of that tab (TakeoverServerMessage, JSON text) and forwards TakeoverInput messages to that tab only, with coordinates in viewport CSS pixels. One controller per ticket (409 takeover_in_use); an Origin header must match the Host. Browsers, which cannot set Authorization on a WebSocket, send the token as `Sec-WebSocket-Protocol: openside.v1, openside.bearer.<token>`; the daemon answers with `openside.v1` only. The stream ends with an `ended` message on done/reject/expiry, idle timeout, token revocation or tab loss. While the ticket is pending, agent browser tools on the tab are refused with takeover_active.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [TicketId](#ticketid) | yes |  |

| Status | Description | Body |
|---|---|---|
| 101 | Switching to WebSocket |  |
| 409 | Another controller is connected | `application/json` [ApiError](#apierror) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## tools

### GET /v1/tools

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [ToolCatalog](#toolcatalog) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## vault

### GET /v1/vault

Required capability: `admin`

Vault state (`uninitialized`, `locked`, `unlocked`, `unavailable`), the unlock method in effect (`vault.unlock = auto`: an existing file keeps its method; a new vault uses this OS's keyring when reachable, else a passphrase) and why it is locked or unavailable. Never values.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [VaultStatus](#vaultstatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/vault/audit

Required capability: `admin`

Vault audit records (who, what, item, origin, session/tool-call id, time, result), oldest first; never values.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `since` | query | string (date-time) | no | Only records at or after this RFC 3339 time. |
| `item` | query | string | no | Only records for this item id. |
| `session` | query | [SessionId](#sessionid) | no | Only records for this session (task) id. |
| `limit` | query | integer | no | Newest N matches; defaults to 200, capped at 1000. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [VaultAuditList](#vaultauditlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### GET /v1/vault/items

Required capability: `admin`

Value-free item metadata (ids, allowed origins, policy, TOTP/passkey presence). Usernames, passwords, seeds and private keys are never returned.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [VaultItemList](#vaultitemlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/vault/items

Required capability: `admin`

Add a login (password, optional username and authenticator) or an authenticator alone (`totp`: otpauth:// URI or base32 key). Values only travel to the daemon, are masked in every later output before the save, are saved encrypted and audited (`item_add`, attributed to the caller: `device:<id>` or `local`); the response is value-free metadata. With a keyring backend the first item creates the vault. 400 invalid_item, invalid_origin, invalid_totp, empty_item; 409 item_exists, vault_unavailable; 500 item_added_audit_incomplete (saved).

Request body (required): `application/json` [CreateVaultItemRequest](#createvaultitemrequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [VaultItem](#vaultitem) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PATCH /v1/vault/items/{id}

Required capability: `admin`

Set or clear (`default`) the item's access-policy override; the change is saved encrypted and audited, attributed to the caller (`device:<id>` or `local`). 404 unknown_item, 409 vault_unavailable; 500 policy_applied_audit_incomplete (saved).

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

Request body (required): `application/json` [UpdateVaultItemRequest](#updatevaultitemrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [VaultItem](#vaultitem) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### DELETE /v1/vault/items/{id}

Required capability: `admin`

Delete the item; saved encrypted and audited (`item_delete`, attributed to the caller). 404 unknown_item, 409 vault_unavailable; 500 item_deleted_audit_incomplete (deleted).

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | string | yes |  |

| Status | Description | Body |
|---|---|---|
| 204 | Success |  |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/vault/lock

Required capability: `admin`

Drop the decrypted vault from daemon memory; vault tools stop working until it is unlocked again. Audited (`lock`) after locking, attributed to the caller; 500 vault_locked_audit_incomplete means it is locked but unrecorded. Output masking of values seen while unlocked stays in place until the daemon restarts.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [VaultStatus](#vaultstatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/vault/setup

Required capability: `admin`

Create the vault file. Keyring backends take no body passphrase and keep a random wrapping key in the OS keyring; a passphrase vault needs `passphrase` (8+ characters), which is never stored or returned. Every attempt that reaches the vault is audited (`setup`: `completed` or `failed` with the code), attributed to the caller (`device:<id>` or `local`). 400 passphrase_required, passphrase_not_used, weak_passphrase; 409 vault_already_set_up; 500 vault_set_up_audit_incomplete (set up and unlocked); 503 keyring_unavailable, vault_unsupported.

Request body (required): `application/json` [VaultUnlockRequest](#vaultunlockrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [VaultStatus](#vaultstatus) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/vault/unlock

Required capability: `admin`

Decrypt the vault: a keyring vault re-reads its key from the OS keyring; a passphrase vault needs `passphrase`. Already unlocked is a no-op. Audited (`unlock`: `unlocked`, `denied` with wrong_passphrase, or `failed` with the code), attributed to the caller; an unlock whose record cannot be written does not happen (500 audit_unavailable). Each caller gets 5 wrong passphrases, then waits 30 s, doubling per further wrong one up to 15 min; a success resets it. Paired devices together get 20 wrong passphrases per 15 minutes; the Unix socket is exempt from that shared limit. At most one attempt per caller, and two across paired devices, run at once. Throttled attempts get 429 too_many_attempts with `Retry-After` seconds, before any key derivation, and are not audited. 400 passphrase_required, passphrase_not_used, wrong_passphrase; 409 vault_not_set_up; 503 keyring_unavailable, vault_unsupported.

Request body (required): `application/json` [VaultUnlockRequest](#vaultunlockrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [VaultStatus](#vaultstatus) |
| 429 | Too many wrong passphrases; retry after the `Retry-After` header's seconds | `application/json` [ApiError](#apierror) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## Schemas

### AccessRequest

A device waiting for someone on the daemon's computer to allow more scopes.

| Field | Type | Required | Description |
|---|---|---|---|
| `created_at` | string (date-time) | yes | When the request was made. |
| `device_id` | string | yes | The requesting device. |
| `expires_at` | string (date-time) | yes | Display time after which the request is dropped; the deadline itself is monotonic. |
| `id` | string | yes | Decision identifier (`POST /v1/access-requests/{id}/approve`). |
| `name` | string | yes | Label given at pairing. |
| `scopes` | [CapabilityScope](#capabilityscope)[] | yes | Scopes asked for. |

### AccessRequestList

Requests waiting for a decision.

| Field | Type | Required | Description |
|---|---|---|---|
| `requests` | [AccessRequest](#accessrequest)[] | yes | Oldest first. |

### AccountSource

One credential plugin's availability.

| Field | Type | Required | Description |
|---|---|---|---|
| `accounts` | [AccountStatus](#accountstatus)[] | yes | Registered broker accounts or configured local provider accounts. |
| `available` | boolean | yes | At least one account is usable (command results remain unverified). |
| `kind` | string | yes | Plugin kind: omp-broker, vault, env, command, claude-code, codex-cli or builtin. |
| `name` | string | yes | Stable plugin name. |
| `reason` | string | yes | Credential-value-free diagnostic. |

Unknown fields are rejected.

### AccountStatus

Public account metadata; ids are opaque and labels never contain raw email addresses.

| Field | Type | Required | Description |
|---|---|---|---|
| `available` | boolean \| null | no | Per-account readiness when supplied; absent in older diagnostic sources. |
| `blocked_until_ms` | number (double) \| null (double) | no | Latest active block expiry, as Unix milliseconds, or null. |
| `id` | string | yes | Opaque account reference, not an upstream account id. |
| `label` | string | yes | Safe display label. |
| `models` | string[] | yes | Account-reported models, or configured models when no catalog is supplied. |
| `provider` | string | yes | Provider identifier. |
| `reason` | string \| null | no | Sanitized per-account explanation, without provider response bodies. |
| `usage_windows` | [AccountUsageWindow](#accountusagewindow)[] | yes | Broker-reported usage windows; empty when unavailable. |

Unknown fields are rejected.

### AccountUsageWindow

One attributed usage window, without broker identifiers or metadata.

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Safe ordinal window label. |
| `reset_at_ms` | number (double) \| null (double) | no | Unix milliseconds of the broker-reported reset, or null when unknown. |
| `used_percent` | number (double) \| null (double) | no | Usage percentage in the inclusive 0–100 range, or null when unknown. |

Unknown fields are rejected.

### AccountsResponse

Credential-source diagnostics, never credential material.

| Field | Type | Required | Description |
|---|---|---|---|
| `cached_for_ms` | integer (uint32) | yes | Maximum time this daemon may reuse the result. |
| `sources` | [AccountSource](#accountsource)[] | yes | Sources in configured priority order. |
| `truncated` | boolean | yes | Some entries exceeded the documented metadata limits. |

Unknown fields are rejected.

### Action

Canonical Action wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Stable identity within its card namespace; never an authorization token. |
| `label` | string | yes | Plain-text accessible label; never markup. |
| `style` | [ActionStyle](#actionstyle) | yes | Visual recommendation; at most one primary per action area. |
| `target` | [ActionTarget](#actiontarget) | yes | Typed dispatch intent; appearance grants no authority. |

Unknown fields are rejected.

### ActionStyle

Canonical ActionStyle wire contract; presentation never grants authority.

| Value | Description |
|---|---|
| `"primary"` | Typed Primary presentation variant; source authorization remains unchanged. |
| `"secondary"` | Typed Secondary presentation variant; source authorization remains unchanged. |
| `"destructive"` | Typed Destructive presentation variant; source authorization remains unchanged. |

### ActionTarget

Canonical ActionTarget wire contract; presentation never grants authority.

- `type=ticket_resolve`: Typed TicketResolve presentation variant; source authorization remains unchanged. Fields: `answer`: [TicketAnswer](#ticketanswer).
- `type=ticket_reject`: Typed TicketReject presentation variant; source authorization remains unchanged.
- `type=ticket_done`: Typed TicketDone presentation variant; source authorization remains unchanged.
- `type=follow_up`: Typed FollowUp presentation variant; source authorization remains unchanged. Fields: `text`: string.
- `type=link`: Typed Link presentation variant; source authorization remains unchanged. Fields: `url`: string.
- `type=open_thread`: Typed OpenThread presentation variant; source authorization remains unchanged. Fields: `agent_id`: string, `anchor`: string \| null (optional).
- `type=open_ticket`: Typed OpenTicket presentation variant; source authorization remains unchanged. Fields: `ticket_id`: [TicketId](#ticketid).
- `type=open_connection`: Typed OpenConnection presentation variant; source authorization remains unchanged. Fields: `connection_id`: string.
- `type=team_command`: Typed TeamCommand presentation variant; source authorization remains unchanged. Fields: `agent_id`: string, `command`: [TeamCardCommand](#teamcardcommand).
- `type=takeover`: Typed Takeover presentation variant; source authorization remains unchanged.

### AddChannelAccountRequest

Create one labelled identity with encrypted write-only credentials.

| Field | Type | Required | Description |
|---|---|---|---|
| `agent_id` | string \| null | no | Optional teammate assignment for new sessions. Default: `null`. |
| `credentials` | [ChannelCredentials](#channelcredentials) | yes | Encrypted immediately, never echoed in the resulting account response. |
| `enabled` | boolean | no | Defaults off until the owner explicitly enables the configured allow-lists. Default: `false`. |
| `id` | string | yes | Stable identifier chosen independently from the service's own account id. |
| `label` | string | yes | Human-readable account label. |
| `options` | [ChannelAccountOptions](#channelaccountoptions) | no | Non-secret settings editable entirely through the GUI. Default: `{"allowed_channels": [], "allowed_chats": [], "allowed_guilds": [], "allowed_users": [], "allowed_workspaces": [], "api_base": null, "listen": null, "poll_timeout_secs": null, "public_key": null}`. |

Unknown fields are rejected.

### Agent

Durable roster record; a conversation tag never changes its home identity.

| Field | Type | Required | Description |
|---|---|---|---|
| `account_pool` | [ModelAccountRef](#modelaccountref)[] \| null | no | Null inherits; a nonempty list restricts and orders the person's provider pool. |
| `channel` | [AgentChannelBinding](#agentchannelbinding) \| null | no | Null removes the explicit profile binding, without rewriting historical threads. |
| `color` | string | yes | One exact value from ACCENT_COLORS. |
| `created_at` | string (date-time) | yes | Registry creation time, not the most recent activity time. |
| `hidden` | boolean | yes | Sidebar visibility only; hiding does not pause work. |
| `home_session_id` | [SessionId](#sessionid) | yes | Chosen once before the home event is written, enabling crash-safe repair. |
| `id` | string | yes | Opaque stable identity; the chief always uses `chief`. |
| `initials` | string | yes | Display override of 1–3 characters, derived from the name when omitted at hire. |
| `kind` | [AgentKind](#agentkind) | yes | Cannot be patched or changed during retirement/restoration. |
| `mode` | [PolicyMode](#policymode) | yes | Ceiling intersected with the chief, session and current daemon policy. |
| `model` | string \| null | no | Null inherits the person's default model. |
| `name` | string | yes | Unique display name, 1–24 Unicode scalar characters. |
| `persona` | [AgentPersona](#agentpersona) | yes | Validated so its rendered prompt fits the 1,500-character hard cap. |
| `persona_version` | integer (uint64) | yes | Monotonic revision used to reopen warm runtimes after profile edits. |
| `pinned` | boolean | yes | Person-selected roster priority, independent of legacy session pinning. |
| `retired_at` | string (date-time) \| null (date-time) | no | Null while active; preserved lifecycle metadata for retired records. |
| `skills` | string[] \| null | no | Null inherits enabled skills; a list is an allowlist, not installed-skill ownership. |
| `status` | [AgentStatus](#agentstatus) | yes | Retirement is not deletion; identity and historical context survive. |

### AgentChannelBinding

One exact channel identity, without credentials or a duplicate settings object.

| Field | Type | Required | Description |
|---|---|---|---|
| `account_id` | string | yes | Opaque account id within this service, not its editable label. |
| `kind` | string | yes | One of telegram, slack, discord. |

Unknown fields are rejected.

### AgentKind

Immutable role that controls which team tools the daemon exposes.

| Value | Description |
|---|---|
| `"chief"` | The unique `chief` identity; delegates but cannot be retired. |
| `"teammate"` | A durable specialist whose work is assigned through the chief. |

### AgentList

Roster ordered with the chief first, then by recent home-thread activity.

| Field | Type | Required | Description |
|---|---|---|---|
| `agents` | [AgentSummary](#agentsummary)[] | yes | Includes retired and hidden profiles so clients can present History and management. |

### AgentPersona

Bounded durable instructions; task-specific instructions belong in handoff packets.

| Field | Type | Required | Description |
|---|---|---|---|
| `boundaries` | string[] | no | At most six nonempty lines of 100 characters; policy remains the hard boundary. Default: `[]`. |
| `responsibilities` | string[] | no | At most six nonempty lines of 100 characters; teammates require at least one. Default: `[]`. |
| `role` | string | yes | Specific non-generic role, 1–60 Unicode scalar characters. |
| `voice` | string | no | At most 100 characters; never truncated when building the prompt. Default: `""`. |

Unknown fields are rejected.

### AgentStatus

Registry lifecycle, distinct from the home conversation's running/waiting state.

| Value | Description |
|---|---|
| `"active"` | May receive new conversations and messages. |
| `"retired"` | History and channel ownership remain, but new work is refused. |

### AgentSummary

Roster projection with a device-local unread cursor rather than server receipts.

| Field | Type | Required | Description |
|---|---|---|---|
| `account_pool` | [ModelAccountRef](#modelaccountref)[] \| null | no | Null inherits; a nonempty list restricts and orders the person's provider pool. |
| `activity` | string | yes | Current work phrase, separate from the most recent message preview. |
| `channel` | [AgentChannelBinding](#agentchannelbinding) \| null | no | Null removes the explicit profile binding, without rewriting historical threads. |
| `color` | string | yes | One exact value from ACCENT_COLORS. |
| `created_at` | string (date-time) | yes | Registry creation time, not the most recent activity time. |
| `hidden` | boolean | yes | Sidebar visibility only; hiding does not pause work. |
| `home_session_id` | [SessionId](#sessionid) | yes | Chosen once before the home event is written, enabling crash-safe repair. |
| `id` | string | yes | Opaque stable identity; the chief always uses `chief`. |
| `initials` | string | yes | Display override of 1–3 characters, derived from the name when omitted at hire. |
| `kind` | [AgentKind](#agentkind) | yes | Cannot be patched or changed during retirement/restoration. |
| `last_activity` | string | yes | Bounded latest-message preview; this field is not a timestamp. |
| `last_message_at` | string (date-time) | yes | Latest non-metadata event timestamp for stable roster ordering. |
| `last_seq` | integer (uint64) | yes | Home SSE sequence; compare with the device's locally stored seen cursor. |
| `mode` | [PolicyMode](#policymode) | yes | Ceiling intersected with the chief, session and current daemon policy. |
| `model` | string \| null | no | Null inherits the person's default model. |
| `name` | string | yes | Unique display name, 1–24 Unicode scalar characters. |
| `needs_you` | boolean | yes | Whether the home conversation currently requires human attention. |
| `persona` | [AgentPersona](#agentpersona) | yes | Validated so its rendered prompt fits the 1,500-character hard cap. |
| `persona_version` | integer (uint64) | yes | Monotonic revision used to reopen warm runtimes after profile edits. |
| `pinned` | boolean | yes | Person-selected roster priority, independent of legacy session pinning. |
| `retired_at` | string (date-time) \| null (date-time) | no | Null while active; preserved lifecycle metadata for retired records. |
| `session_status` | [SessionStatus](#sessionstatus) | yes | Home conversation state, not the registry's active/retired lifecycle. |
| `skills` | string[] \| null | no | Null inherits enabled skills; a list is an allowlist, not installed-skill ownership. |
| `status` | [AgentStatus](#agentstatus) | yes | Retirement is not deletion; identity and historical context survive. |

### ApiError

JSON error response for non-successful HTTP requests.

| Field | Type | Required | Description |
|---|---|---|---|
| `code` | string | yes | Machine-readable failure code. |
| `message` | string | yes | Sanitized human-facing detail. |

### Artifact

One file in a session's artifact directory.

| Field | Type | Required | Description |
|---|---|---|---|
| `created_at` | string (date-time) | yes | First recording time. |
| `mime` | string | yes | MIME type served by the raw route. |
| `modified_at` | string (date-time) | yes | Latest recording time. |
| `path` | string | yes | Path relative to the artifact directory. |
| `preview_url` | string \| null | no | Time-limited, unauthenticated URL path for sandboxed rendering, when previewable. |
| `purpose` | [ArtifactPurpose](#artifactpurpose) | yes | Delivery classification; working observations are omitted from the list route. |
| `size` | integer (uint64) | yes | Byte size. |
| `source` | [ArtifactSource](#artifactsource) | yes | Origin of the file. |
| `tool` | string \| null | no | Tool name that last wrote it, for tool artifacts. |
| `tool_call_id` | string \| null | no | Tool call that last wrote it, for tool artifacts. |

### ArtifactList

`GET /v1/sessions/{id}/artifacts` response, oldest first.

| Field | Type | Required | Description |
|---|---|---|---|
| `artifacts` | [Artifact](#artifact)[] | yes | User-visible files in creation order, excluding working observations. |

### ArtifactPurpose

Whether a file is a deliverable or an internal observation.

| Value | Description |
|---|---|
| `"output"` | A file available in the user-visible session files listing. |
| `"working"` | An internal observation excluded from user deliverables. |

### ArtifactSource

Where an artifact came from.

| Value | Description |
|---|---|
| `"attachment"` | A user attachment. |
| `"tool"` | A file a tool created or modified (copied from the workspace) or a tool output. |
| `"transcript"` | The exported session event log. |
| `"visual"` | A visual block from an assistant message. |

### Attachment

Durable record of a stored attachment, carried by `message.queued`.

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | [ContentKind](#contentkind) | yes | Media class. |
| `mime` | string | yes | Sniffed MIME type. |
| `name` | string \| null | no | Sanitized display name. |
| `path` | string | yes | Artifact path holding the bytes. |
| `sha256` | string | yes | Lowercase hex SHA-256 of the bytes. |
| `size` | integer (uint64) | yes | Byte size. |

### AttachmentUpload

One uploaded attachment; the daemon sniffs the type from the bytes.

| Field | Type | Required | Description |
|---|---|---|---|
| `data_base64` | string | yes | Standard base64 file content. Allowed: PNG, JPEG, GIF, WebP, PDF, WAV, MP3, OGG, FLAC and UTF-8 text; anything else is `415 unsupported_attachment`. |
| `name` | string \| null | no | Display file name; directories are stripped. |

Unknown fields are rejected.

### Block

Canonical Block wire contract; presentation never grants authority.

- `type=kv_rows`: Typed KvRows presentation variant; source authorization remains unchanged. Fields: `id`: string, `rows`: [KvRow](#kvrow)[], `total`: [KvRow](#kvrow) \| null (optional).
- `type=list_items`: Typed ListItems presentation variant; source authorization remains unchanged. Fields: `id`: string, `items`: [ListItem](#listitem)[].
- `type=table`: Typed Table presentation variant; source authorization remains unchanged. Fields: `columns`: [TableColumn](#tablecolumn)[], `id`: string, `layout`: [TableLayout](#tablelayout), `row_sites`: [CardSite](#cardsite) \| null[] \| null (optional), `rows`: [CardValue](#cardvalue)[][].
- `type=choice_options`: Human-facing choices can contain saved-account identities; not agent observations. Fields: `id`: string, `label`: string \| null (optional), `layout`: [ChoiceLayout](#choicelayout), `options`: [ChoiceOption](#choiceoption)[], `required`: boolean.
- `type=input_fields`: Typed InputFields presentation variant; source authorization remains unchanged. Fields: `fields`: [InputField](#inputfield)[], `id`: string.
- `type=notice`: Typed Notice presentation variant; source authorization remains unchanged. Fields: `id`: string, `placement`: [NoticePlacement](#noticeplacement), `text`: string, `tone`: [NoticeTone](#noticetone).
- `type=media`: Typed Media presentation variant; source authorization remains unchanged. Fields: `alt`: string, `caption`: string \| null (optional), `id`: string, `media`: [MediaRef](#mediaref).
- `type=people`: Typed People presentation variant; source authorization remains unchanged. Fields: `from`: [CardPerson](#cardperson) \| null (optional), `id`: string, `to`: [CardPerson](#cardperson).

### BrowserActionState

The tool call that produced a browser state.

| Field | Type | Required | Description |
|---|---|---|---|
| `phrase` | string | yes | Human phrase such as "Clicking" or "Opening example.com". |
| `target` | string \| null | no | Accessible label of the target element, read from the page. |
| `tool` | string | yes | Tool name. |
| `tool_call_id` | string | yes | Runtime call identifier. |

### BrowserPhase

Agent activity in a tab.

| Value | Description |
|---|---|
| `"loading"` | Navigating or waiting for the page to load. |
| `"reading"` | Reading the page (snapshot, screenshot). |
| `"acting"` | Clicking, typing or otherwise interacting. |
| `"waiting_human"` | Waiting for a person (login, code, choice, approval). |
| `"done"` | The Task finished with this tab. |
| `"error"` | The last browser action failed. |

### BrowserSpace

Where a session's agent tabs browse, fixed at creation.

| Value | Description |
|---|---|
| `"shared"` | The managed browser profile, with the logins and cookies it already holds. |
| `"isolated"` | An isolated browser context in the same managed browser: own cookies and storage, kept in memory and wiped when the session is archived or deleted. |

### BrowserState

Typed browser situation for one tab, rendered as a live card per Task.

| Field | Type | Required | Description |
|---|---|---|---|
| `action` | [BrowserActionState](#browseractionstate) \| null | no | The tool call behind this state. |
| `facts` | [PageFacts](#pagefacts) | yes | Facts read from the page. |
| `favicon` | string \| null | no | PNG data URL, at most 32×32 pixels and 8 KiB decoded. |
| `host` | string | yes | Host of `url`, empty for non-network pages. |
| `page_kind` | [PageKind](#pagekind) | yes | Classification of the page. |
| `phase` | [BrowserPhase](#browserphase) | yes | What the agent is doing in the tab. |
| `tab_id` | string | yes | Browser target identifier. |
| `task_id` | string \| null | no | Task (background subagent) acting in the tab; absent for the front agent. |
| `thumbnail` | string \| null | no | Opaque JPEG thumbnail ID, fetched through the authenticated session route. |
| `title` | string | yes | Page title. |
| `url` | string | yes | Current URL with sensitive components redacted. |

### BrowserTab

Metadata-only browser ledger entry; incognito tabs are never included.

| Field | Type | Required | Description |
|---|---|---|---|
| `opened_by` | [BrowserTabOwnership](#browsertabownership) | yes | Explicit provenance, independent of URL matching. |
| `owner_session_id` | string \| null | no | Session responsible for creation, if known. |
| `tab_id` | string | yes | CDP target identity, not a Chrome extension tab number. |
| `title` | string | yes | Current document title. |
| `url` | string | yes | Current URL without credentials, query parameters, or fragment. |

### BrowserTabList

Explicitly agent-owned tabs from the shared browser, empty before launch; a stale controller returns 503 browser_disconnected.

| Field | Type | Required | Description |
|---|---|---|---|
| `tabs` | [BrowserTab](#browsertab)[] | yes | Stable target-id ordering; user, external, unknown-owner and private-session tabs are omitted. |

### BrowserTabOwnership

Browser tab provenance; only `agent` permits Agent Tabs grouping.

| Value | Description |
|---|---|
| `"agent"` | Created by a session. |
| `"user"` | Adopted by the user. |
| `"external"` | Discovered without an explicit creation operation. |

### CapabilityScope

Capability required to access an HTTP operation.

| Value | Description |
|---|---|
| `"read"` | Inspect sessions, events, and tickets. |
| `"steer"` | Create sessions and modify or deliver messages. |
| `"approve"` | Resolve or reject confirm, info and choice tickets; positive confirm/choice decisions additionally require a person device. |
| `"human_step"` | Resolve, complete, or reject secret-input, login, device-approval, and takeover tickets. |
| `"admin"` | Pair and revoke devices. |
| `"browse"` | Drive session browser routes (`browser_repl`) without steering the agent; for sandboxed tools, MCP servers and extensions. |
| `"vault_use"` | Use saved vault credentials through vault routes; never reveals values. |
| `"otp_ingest"` | Deliver one-time codes to the OTP ingest route. |

### Card

Canonical Card wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `actions` | [Action](#action)[] | yes | Typed intents validated against the authoritative source. |
| `blocks` | [Block](#block)[] | yes | Ordered typed content; no HTML or executable templates. |
| `collapsed_summary` | string | yes | Value-free receipt or pending summary, bounded to 240 characters. |
| `id` | string | yes | Stable identity within its card namespace; never an authorization token. |
| `interaction` | [CardInteraction](#cardinteraction) | yes | Interactive cards require a daemon-issued ticket. |
| `kind` | [CardKind](#cardkind) | yes | Semantic presentation family, not permission to act. |
| `site` | [CardSite](#cardsite) | yes | Source attribution with an exact origin boundary. |
| `status` | [CardStatus](#cardstatus) | yes | Presentation lifecycle only; fresh ticket state governs actions. |
| `subtitle` | string \| null | no | Optional plain-text context, bounded to 280 Unicode scalar values. |
| `ticket_id` | [TicketId](#ticketid) \| null | no | Daemon-issued authority reference; forbidden in model presentations. |
| `title` | string | yes | Plain-text heading, bounded to 160 Unicode scalar values. |

Unknown fields are rejected.

### CardField

Canonical CardField wire contract; presentation never grants authority.

| Value | Description |
|---|---|
| `"name"` | Typed Name presentation variant; source authorization remains unchanged. |
| `"number"` | Typed Number presentation variant; source authorization remains unchanged. |
| `"expiry"` | Typed Expiry presentation variant; source authorization remains unchanged. |
| `"cvc"` | Typed Cvc presentation variant; source authorization remains unchanged. |
| `"postal_code"` | Typed PostalCode presentation variant; source authorization remains unchanged. |

### CardInteraction

Canonical CardInteraction wire contract; presentation never grants authority.

| Value | Description |
|---|---|
| `"none"` | Typed None presentation variant; source authorization remains unchanged. |
| `"ask"` | Typed Ask presentation variant; source authorization remains unchanged. |

### CardKind

Canonical CardKind wire contract; presentation never grants authority.

| Value | Description |
|---|---|
| `"product_list"` | Typed ProductList presentation variant; source authorization remains unchanged. |
| `"table"` | Typed Table presentation variant; source authorization remains unchanged. |
| `"place_list"` | Typed PlaceList presentation variant; source authorization remains unchanged. |
| `"order_summary"` | Typed OrderSummary presentation variant; source authorization remains unchanged. |
| `"payment"` | Typed Payment presentation variant; source authorization remains unchanged. |
| `"secure_login"` | Typed SecureLogin presentation variant; source authorization remains unchanged. |
| `"login_choice"` | Typed LoginChoice presentation variant; source authorization remains unchanged. |
| `"otp"` | Typed Otp presentation variant; source authorization remains unchanged. |
| `"payment_card"` | Typed PaymentCard presentation variant; source authorization remains unchanged. |
| `"confirmation"` | Typed Confirmation presentation variant; source authorization remains unchanged. |
| `"form"` | Typed Form presentation variant; source authorization remains unchanged. |
| `"connect_account"` | Typed ConnectAccount presentation variant; source authorization remains unchanged. |
| `"activity"` | Typed Activity presentation variant; source authorization remains unchanged. |
| `"handoff"` | Typed Handoff presentation variant; source authorization remains unchanged. |
| `"hire"` | Typed Hire presentation variant; source authorization remains unchanged. |
| `"secret_input"` | Typed SecretInput presentation variant; source authorization remains unchanged. |
| `"takeover"` | Typed Takeover presentation variant; source authorization remains unchanged. |

### CardLink

Canonical CardLink wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `label` | string | yes | Plain-text accessible label; never markup. |
| `url` | string | yes | Validated HTTP(S) navigation URL without credentials or secret material. |

Unknown fields are rejected.

### CardPerson

Canonical CardPerson wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `agent_id` | string \| null | no | Daemon-derived roster identity; not model-selected authority. |
| `color` | string \| null | no | Allowlisted team avatar token, never arbitrary CSS. |
| `initials` | string | yes | Bounded text avatar fallback. |
| `name` | string | yes | Plain-text display name, not a verified identity claim. |
| `role` | string \| null | no | Plain-text roster responsibility. |

Unknown fields are rejected.

### CardSite

Canonical CardSite wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `favicon_ref` | [FaviconRef](#faviconref) \| null | no | Daemon-normalized icon reference; never a remote image source. |
| `name` | string | yes | Plain-text display name, not a verified identity claim. |
| `origin` | string \| null | no | Exact canonical URL origin, including nondefault port. |

Unknown fields are rejected.

### CardStatus

Canonical CardStatus wire contract; presentation never grants authority.

| Value | Description |
|---|---|
| `"pending"` | Typed Pending presentation variant; source authorization remains unchanged. |
| `"done"` | Typed Done presentation variant; source authorization remains unchanged. |
| `"failed"` | Typed Failed presentation variant; source authorization remains unchanged. |
| `"expired"` | Typed Expired presentation variant; source authorization remains unchanged. |

### CardValue

Canonical CardValue wire contract; presentation never grants authority.

- `type=text`: Typed Text presentation variant; source authorization remains unchanged. Fields: `text`: string.
- `type=money`: Typed Money presentation variant; source authorization remains unchanged. Fields: `money`: [Money](#money).
- `type=link`: Typed Link presentation variant; source authorization remains unchanged. Fields: `link`: [CardLink](#cardlink).
- `type=rating`: Typed Rating presentation variant; source authorization remains unchanged. Fields: `rating`: [Rating](#rating).
- `type=image`: Typed Image presentation variant; source authorization remains unchanged. Fields: `alt`: string, `image`: [MediaRef](#mediaref).

### CatalogModel

One candidate and the credential source that would serve a new sidecar.

| Field | Type | Required | Description |
|---|---|---|---|
| `account_id` | string \| null | no | Opaque source-local account reference, or null when unavailable. |
| `accounts` | [ModelAccount](#modelaccount)[] | yes | Credential-free candidates, including blocked/unavailable choices. |
| `available` | boolean | yes | Local credential readiness under source priority and broker fail-close. |
| `id` | string | yes | Explicit provider:model selector; image-only models are not chat defaults. |
| `label` | string | yes | Model-name portion, never an account label or private identity. |
| `reason` | string | yes | Credential-value-free readiness or refusal explanation. |
| `source` | string \| null | no | First usable source, or null when unavailable. |

Unknown fields are rejected.

### ChannelAccount

Metadata-only account response; credential fields are intentionally absent.

| Field | Type | Required | Description |
|---|---|---|---|
| `agent_id` | string \| null | no | Owner of newly created sessions; changing this does not reroute existing ones. |
| `credential_source` | string | yes | `vault` for managed accounts, `legacy` for file-only environment/command sources. |
| `effective_agent_id` | string \| null | no | Resolved supervised owner, including inherited profile bindings; null when inactive. Default: `null`. |
| `enabled` | boolean | yes | Desired enablement; status separately reports effective availability. |
| `id` | string | yes | Stable per-kind id, independent of the account label. |
| `kind` | string | yes | Supported service discriminator. |
| `label` | string | yes | User-chosen label, never a credential suffix. |
| `options` | [ChannelAccountOptions](#channelaccountoptions) | yes | Non-secret platform options. |
| `receiver_address` | string \| null | no | Actual loopback webhook listener, null for polling or inactive accounts. |
| `restart_required` | boolean | yes | Live application unavailable; never implies another identity is in use. |
| `secret_present` | boolean | yes | Whether the unlocked Vault contains the typed entry, or legacy references exist. |
| `status` | [ChannelAccountStatus](#channelaccountstatus) | yes | Effective state of this account only. |
| `webhook_url` | string \| null | no | Exact signed callback URL to paste into the platform; null for Telegram or legacy. Default: `null`. |

### ChannelAccountList

Read-scoped settings inventory, with no upstream credentials.

| Field | Type | Required | Description |
|---|---|---|---|
| `accounts` | [ChannelAccount](#channelaccount)[] | yes | All configured identities, including disabled and normalized legacy accounts. |

### ChannelAccountOptions

Credential-free platform settings; allow-lists are intersected.

| Field | Type | Required | Description |
|---|---|---|---|
| `allowed_channels` | string[] | no | Slack or Discord channel allow-list. Default: `[]`. |
| `allowed_chats` | string[] | no | Telegram chat allow-list; at least one relevant allow-list is required. Default: `[]`. |
| `allowed_guilds` | string[] | no | Discord guild allow-list. Default: `[]`. |
| `allowed_users` | string[] | no | User allow-list intersected with channel and workspace restrictions. Default: `[]`. |
| `allowed_workspaces` | string[] | no | Slack team allow-list. Default: `[]`. |
| `api_base` | string \| null | no | Custom destination is writable only through the local socket. Default: `null`. |
| `listen` | string \| null | no | Remote writes permit only unprivileged loopback webhook listeners. Default: `null`. |
| `poll_timeout_secs` | integer (uint32) \| null (uint32) | no | Positive Telegram long-poll timeout; absent uses the configured default. Default: `null`. |
| `public_key` | string \| null | no | Public Ed25519 verification key for Discord inbound interactions. Default: `null`. |

Unknown fields are rejected.

### ChannelAccountStatus

Per-identity state; no status authorizes fallback to another account.

| Value | Description |
|---|---|
| `"connected"` | Adapter is running, or an explicit read-only connectivity test passed. |
| `"disabled"` | Owner disabled this identity; configuration and encrypted credentials remain. |
| `"expired"` | Credentials were refused; reconnect by replacing this account's credentials. |
| `"unavailable"` | Network, listener or credential-source failure; no other account is selected. |
| `"vault_locked"` | Vault cannot release this account's credentials. |
| `"restart_required"` | The legacy runner or absent daemon TCP listener prevents live application. |

### ChannelAccountTest

Result of a read-only platform identity check, with sanitized failure codes.

| Field | Type | Required | Description |
|---|---|---|---|
| `account_id` | string | yes | Account tested; other accounts are unaffected. |
| `code` | string | yes | Allow-listed machine-readable code, never upstream error text. |
| `kind` | string | yes | Service discriminator. |
| `ok` | boolean | yes | Whether the upstream identity check succeeded. |
| `restart_required` | boolean | yes | Whether enabled operation still needs a daemon listener or legacy runner restart. |
| `status` | [ChannelAccountStatus](#channelaccountstatus) | yes | Tested credential state, independent of desired enablement. |

### ChannelCredentials

Write-only input; Debug is masked and allocations are zeroized on drop.

| Field | Type | Required | Description |
|---|---|---|---|
| `signing_secret` | string \| null | no | Required for Slack inbound verification, forbidden for other services. Default: `null`. |
| `token` | string | yes | Bot credential supplied through a secure form or hidden stdin. |

Unknown fields are rejected.

### ChoiceAnswerEncoding

Canonical ChoiceAnswerEncoding wire contract; presentation never grants authority.

| Value | Description |
|---|---|
| `"page_option"` | Typed PageOption presentation variant; source authorization remains unchanged. |
| `"info_answer"` | Typed InfoAnswer presentation variant; source authorization remains unchanged. |

### ChoiceLayout

Canonical ChoiceLayout wire contract; presentation never grants authority.

| Value | Description |
|---|---|
| `"stack"` | Typed Stack presentation variant; source authorization remains unchanged. |
| `"providers"` | Typed Providers presentation variant; source authorization remains unchanged. |

### ChoiceOption

Canonical ChoiceOption wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `action` | [Action](#action) \| null | no | Optional immediate authoritative action; absence means selection for submit. |
| `description` | string \| null | no | Plain-text secondary context bounded to 280 characters. |
| `id` | string | yes | Stable identity within its card namespace; never an authorization token. |
| `label` | string | yes | Plain-text accessible label; never markup. |
| `provider` | [LoginProvider](#loginprovider) \| null | no | Recognized provider selects a bundled glyph, not a remote image. |

Unknown fields are rejected.

### CloudflareStatus

Supervised `cloudflared` state.

| Field | Type | Required | Description |
|---|---|---|---|
| `last_error` | string \| null | no | Latest failure, without secrets. |
| `restarts` | integer (uint32) | yes | Times the process exited and was started again. |
| `state` | string | yes | `starting`, `running`, `restarting` or `failed` (cannot start at all). |
| `url` | string \| null | no | Public HTTPS URL once known (quick tunnels change it on every restart). |

### ConfigureTailscaleRequest

Enable or disable openside's Tailscale Serve handler.

| Field | Type | Required | Description |
|---|---|---|---|
| `enabled` | boolean | yes | Whether the transport should run. |

Unknown fields are rejected.

### Connection

Authentication material and the identity HMAC are deliberately absent from this view.

| Field | Type | Required | Description |
|---|---|---|---|
| `account_space` | string \| null | no | Null selects the shared agent profile. Default: `null`. |
| `accounts_on_site` | integer (uint32) | yes | Number of recorded accounts in the same site/profile. |
| `created_at` | string (date-time) | yes | UTC time the row was first created. |
| `expires_at` | string (date-time) \| null (date-time) | no | Latest live fingerprint expiry, or null for session cookies. |
| `favicon` | string \| null | no | Bounded PNG data URL; no external URL is fetched by the client. |
| `grants` | [ConnectionGrant](#connectiongrant)[] | yes | Explicit overrides; unspecified teammates ask. |
| `home_url` | string \| null | no | Same-site HTTP(S) URL without query, fragment or credentials. |
| `id` | string | yes | Opaque c_-prefixed identifier. |
| `identity` | string \| null | no | Write-time-masked label; never a raw email or account key. |
| `identity_source` | [ConnectionIdentitySource](#connectionidentitysource) | yes | Evidence behind the label, not a trust level. |
| `jar_group` | string \| null | no | Shared cookie-set identifier for site account switchers. |
| `profile` | [ConnectionProfile](#connectionprofile) | yes | Cookie jar ownership, independent of the calling device. |
| `site` | string | yes | Registrable domain under the public and private suffix lists. |
| `source` | [ConnectionSource](#connectionsource) | yes | Strongest recorded sign-in source. |
| `status` | [ConnectionStatus](#connectionstatus) | yes | Current confidence and liveness. |
| `used_at` | string (date-time) \| null (date-time) | no | Most recent successful agent use, if any. |
| `used_by` | string \| null | no | Stable agent ID associated with used_at. |
| `vault_item_id` | string \| null | no | Optional saved login link; not the login's contents. |
| `verified_at` | string (date-time) \| null (date-time) | no | Most recent authoritative sign-in or explicit probe. |

### ConnectionAccess

Per-agent access; absent teammates default to ask.

| Value | Description |
|---|---|
| `"allowed"` | May use the sign-in subject to ordinary action policy. |
| `"ask"` | Must obtain a Confirm ticket before use. |
| `"denied"` | Cannot use or list the sign-in through agent tools. |

### ConnectionAuditAction

Closed audit vocabulary; arbitrary page text is never an action.

| Value | Description |
|---|---|
| `"detected"` | A sign-in source produced metadata. |
| `"verified"` | A probe verified authenticated state. |
| `"expired"` | All fingerprint keys disappeared or expired. |
| `"needs_login"` | An authentication challenge was observed. |
| `"relogin"` | An administrator requested authentication again. |
| `"open"` | An agent opened a signed-in page. |
| `"fetch"` | An agent issued an in-page request. |
| `"grant"` | An administrator changed grants. |
| `"forget"` | Metadata and grants were removed without deleting cookies. |
| `"sign_out"` | Site authentication was cleared. |

### ConnectionAuditList

Bounded, newest-first audit response.

| Field | Type | Required | Description |
|---|---|---|---|
| `records` | [ConnectionAuditRecord](#connectionauditrecord)[] | yes | Filtered metadata records. |

### ConnectionAuditQuery

Admin audit filters; all supplied predicates are combined.

| Field | Type | Required | Description |
|---|---|---|---|
| `action` | [ConnectionAuditAction](#connectionauditaction) \| null | no | One audit action. |
| `agent` | string \| null | no | Exact agent ID. |
| `connection` | string \| null | no | Exact connection ID. |
| `limit` | integer (uint) \| null (uint) | no | Defaults to 100 and is clamped to 1–1000. |

Unknown fields are rejected.

### ConnectionAuditRecord

Append-only metadata audit; no credentials, identities or request bodies.

| Field | Type | Required | Description |
|---|---|---|---|
| `action` | [ConnectionAuditAction](#connectionauditaction) | yes | Closed operation category. |
| `agent_id` | string | yes | Stable acting agent ID. |
| `at` | string (date-time) | yes | UTC event time. |
| `connection_id` | string | yes | Connection identifier, retained after removal. |
| `detail` | string \| null | no | Only method and path for fetch, never queries, headers or bodies. |
| `session_id` | [SessionId](#sessionid) \| null | no | Originating session when the operation has one. |
| `site` | string | yes | Registrable site, never a request URL. |

### ConnectionCheckArgs

Explicit probe arguments; the probe uses the calling session's Browse policy.

| Field | Type | Required | Description |
|---|---|---|---|
| `connection_id` | string | yes | Opaque registry ID. |

Unknown fields are rejected.

### ConnectionConfirmArgs

Page-derived identity input; excluded from Debug because it is not yet masked.

| Field | Type | Required | Description |
|---|---|---|---|
| `connection_id` | string | yes | Opaque registry ID; labels never select the authorization target. |
| `identity` | string | yes | Transient label to mask and HMAC at the write boundary. |

Unknown fields are rejected.

### ConnectionError

Stable connection error codes; diagnostic data is not included.

| Value | Description |
|---|---|
| `"connection_not_found"` | No live registry row exists. |
| `"connection_denied"` | Grant or policy refuses the operation. |
| `"connection_needs_login"` | Authentication must be completed before use. |
| `"connection_cross_site"` | URL or request origin is outside the connection site. |
| `"connection_browser_unavailable"` | The managed browser cannot service the operation. |
| `"connection_write_needs_approval"` | A mutating in-page request needs a bound approval. |

### ConnectionFetchArgs

Same-origin request arguments; bodies and queries must never enter audit detail.

| Field | Type | Required | Description |
|---|---|---|---|
| `connection_id` | string | yes | Opaque registry ID used for grant checks. |
| `json_body` | any | no | Request-only JSON data, excluded from audit. |
| `method` | string | yes | GET/HEAD browse; other methods require bound write approval. |
| `path` | string | yes | Same-origin path bound into the approval descriptor. |
| `query` | map of string \| null | no | Request-only query data, excluded from audit. |
| `reuse_headers_from` | string \| null | no | Daemon-observed request ID; header values never appear in arguments. |
| `tab_id` | string | yes | Session-owned tab already at the connection origin. |

Unknown fields are rejected.

### ConnectionGrant

A grant is bound to a stable agent ID, not an identity label.

| Field | Type | Required | Description |
|---|---|---|---|
| `access` | [ConnectionAccess](#connectionaccess) | yes | Additional gate; never bypasses Browse or write policy. |
| `agent_id` | string | yes | Stable team agent ID, or chief before team integration. |

Unknown fields are rejected.

### ConnectionIdentitySource

Identity labels are descriptive and never grant authority.

| Value | Description |
|---|---|
| `"probe"` | Generic account controls or landmarks. |
| `"agent"` | Agent-confirmed page text. |
| `"login_ticket"` | Login ticket identity. |
| `"vault"` | Linked Vault identity. |
| `"none"` | No account label is known. |

### ConnectionList

Admin listing separates weak candidates from ordinary cards.

| Field | Type | Required | Description |
|---|---|---|---|
| `browser_available` | boolean | yes | Whether a managed browser is configured and available. |
| `candidates` | [Connection](#connection)[] | yes | Weak detections for the collapsed disclosure. |
| `connections` | [Connection](#connection)[] | yes | Non-candidate rows in stable site/ID order. |

### ConnectionListQuery

Admin list filters; candidate separation is retained after filtering.

| Field | Type | Required | Description |
|---|---|---|---|
| `agent` | string \| null | no | Exclude connections explicitly denied to this agent. |
| `status` | [ConnectionStatus](#connectionstatus) \| null | no | Restrict to one liveness/confidence state. |

Unknown fields are rejected.

### ConnectionObserveArgs

Request-metadata observation never includes header values or bodies.

| Field | Type | Required | Description |
|---|---|---|---|
| `tab_id` | string | yes | Tab owned by the calling session. |

Unknown fields are rejected.

### ConnectionOpenArgs

Page-opening arguments, subject to independent grant and Browse gates.

| Field | Type | Required | Description |
|---|---|---|---|
| `connection_id` | string | yes | Opaque registry ID. |
| `url` | string \| null | no | Optional same-site destination instead of home_url. |

Unknown fields are rejected.

### ConnectionProfile

Browser profile whose sign-in metadata this row describes.

| Value | Description |
|---|---|
| `"agent"` | Managed shared agent profile. |
| `"user"` | Person's browser; sharing is a separate local-only operation. |

### ConnectionSource

Evidence that originally established or refreshed the sign-in.

| Value | Description |
|---|---|
| `"login"` | Completed login ticket or request. |
| `"vault"` | Saved password or passkey sign-in. |
| `"takeover"` | Human takeover crossed the login boundary. |
| `"otp"` | OTP submission followed by an authenticated page. |
| `"import"` | Explicit browser import. |
| `"sweep"` | Weak cookie/history sweep. |
| `"share"` | Explicit local sharing from the person's browser. |

### ConnectionStatus

Confidence and liveness; only an observed sign-in or probe establishes active status.

| Value | Description |
|---|---|
| `"candidate"` | Cookie/history heuristic, separated from confirmed connections. |
| `"unverified"` | Imported cookies that have not been checked. |
| `"active"` | Observed authenticated state. |
| `"expired"` | Every fingerprint key is absent or expired. |
| `"needs_login"` | The page or document response requests authentication. |

### ConnectionUseMode

Agent use category carried by connection.used events.

| Value | Description |
|---|---|
| `"open"` | Open or reuse a page. |
| `"fetch"` | Same-origin in-page HTTP request. |

### ConnectionsListArgs

Metadata-only listing; denied rows are excluded for the calling agent.

| Field | Type | Required | Description |
|---|---|---|---|
| `site` | string \| null | no | Optional registrable-domain filter. |

Unknown fields are rejected.

### ContentKind

Broad media class of one content part; the runtime maps it to model input types.

| Value | Description |
|---|---|
| `"text"` | UTF-8 text document. |
| `"image"` | Raster image (PNG, JPEG, GIF, WebP). |
| `"audio"` | Audio clip (WAV, MP3, OGG, FLAC). |
| `"file"` | Any other document, e.g. PDF. |

### CreateAccountSpaceRequest

A person opts in to a separate persistent sign-in profile for this site.

| Field | Type | Required | Description |
|---|---|---|---|
| `site` | string | yes | Registrable site domain, without a URL path. |

Unknown fields are rejected.

### CreateAccountSpaceResult

The new profile and its existing login continuation.

| Field | Type | Required | Description |
|---|---|---|---|
| `connection_id` | string \| null | no | Connection metadata created for this profile. |
| `result` | [ReloginResult](#reloginresult) | yes | Silent completion or the existing human-step ticket. |
| `space_id` | string | yes | Persistent profile identifier. |

### CreateHandoffRequest

Person-initiated assignment still routes through the chief as sender.

| Field | Type | Required | Description |
|---|---|---|---|
| `dedupe_key` | string | yes | Existing open ownership of this key is a conflict, not another dispatch. |
| `kind` | [HandoffKind](#handoffkind) | yes | Selects task, ownership or independent-review rules. |
| `packet` | [HandoffPacket](#handoffpacket) | yes | Validated self-contained assignment; goal and acceptance must be nonblank. |
| `subject_handoff_id` | string \| null | no | Review subject; never authorizes the subject's author to review itself. |

Unknown fields are rejected.

### CreateMemoryRequest

`POST /v1/memory` body; creates a note.

| Field | Type | Required | Description |
|---|---|---|---|
| `content` | string | yes | Text to remember; rejected with `secret_detected` when it looks like a secret. |
| `retention` | [Retention](#retention) \| null | no | Defaults to `memory.default_retention`. |
| `tags` | string[] | no | Optional labels. Default: `[]`. |

Unknown fields are rejected.

### CreatePairingRequest

Create a one-time pairing offer from the local socket or a verified local person.

| Field | Type | Required | Description |
|---|---|---|---|
| `import_delivery` | boolean | no | Establish an import authenticator for the extension through this local-person offer. Default: `false`. |
| `kind` | [DeviceKind](#devicekind) | no | Operator-selected identity role, fixed in the offer and not chosen by its redeemer. Default: `"person_device"`. |
| `name` | string | yes | Label the paired device will carry. |
| `scopes` | [CapabilityScope](#capabilityscope)[] | yes | Scopes the device token will hold; at least one. |
| `via` | [PairingTransport](#pairingtransport) \| null | no | Transport the link should use; default: the first entry of `remote.mode`, or the plain `remote.public_url` when no transport is configured. Default: `null`. |

Unknown fields are rejected.

### CreatePinnedTabRequest

`POST /v1/pinned-tabs`. Pinning an already pinned URL updates its title.

| Field | Type | Required | Description |
|---|---|---|---|
| `title` | string \| null | no | Optional display title (at most 512 characters). Default: `null`. |
| `url` | string | yes | http(s) URL. |

Unknown fields are rejected.

### CreateProviderKeyRequest

Secret creation input; never Debug/Serialize or error-quoted.

| Field | Type | Required | Description |
|---|---|---|---|
| `key` | string | yes | Provider key from the private input body, never argv or response. |
| `label` | string | yes | Non-control person-assigned label,1–80 characters. |

Unknown fields are rejected.

### CreatePushSubscriptionRequest

Register a browser push subscription for the calling device.

| Field | Type | Required | Description |
|---|---|---|---|
| `endpoint` | string | yes | Push service endpoint; HTTPS, or HTTP on a loopback address. |
| `keys` | [PushKeys](#pushkeys) | yes | Encryption keys. |

### CreateRoutineRequest

Routine creation request.

| Field | Type | Required | Description |
|---|---|---|---|
| `action` | [RoutineAction](#routineaction) | yes | Action performed on each run. |
| `catch_up` | boolean \| null | no | Run once for the latest occurrence missed while the daemon was down; defaults to false. |
| `enabled` | boolean \| null | no | Start enabled; defaults to true. |
| `name` | string | yes | Human-readable name; must not be blank. |
| `schedule` | [RoutineSchedule](#routineschedule) | yes | Firing schedule. |

Unknown fields are rejected.

### CreateSessionRequest

Session creation request.

| Field | Type | Required | Description |
|---|---|---|---|
| `agent_id` | string \| null | no | Existing active agent for a new non-home thread; cannot rebind an existing session. Default: `null`. |
| `browser_space` | [BrowserSpace](#browserspace) | no | Browse in the shared managed profile (default) or an isolated space; immutable afterwards. Incognito sessions already have their own browser and refuse `isolated`. |
| `incognito` | boolean | no | Create a private session; immutable afterwards. Browsing history is not indexed. |
| `mode` | [PolicyMode](#policymode) \| null | no | Permission override, or settings default when omitted. |
| `model` | string \| null | no | Model override, or settings default when omitted. |
| `title` | string \| null | no | Conversation title, or a daemon-generated title when omitted. |
| `workspace` | string \| null | no | Absolute workspace path, canonicalized by the daemon before session creation. |

Unknown fields are rejected.

### CreateSkillRequest

`POST /v1/skills` body; creates a user skill.

| Field | Type | Required | Description |
|---|---|---|---|
| `body` | string | no | Markdown instructions. Default: `""`. |
| `description` | string | yes | Required, at most 1024 characters. |
| `license` | string \| null | no | Optional license text. |
| `name` | string | yes | Skill name: 1-64 lowercase letters, digits and single hyphens. |
| `resources` | [SkillFile](#skillfile)[] | no | Optional text resources (at most 64 files, 512 KiB in total). Executable scripts cannot be uploaded; use `openside skills import --allow-scripts`. Default: `[]`. |

Unknown fields are rejected.

### CreateSubscriptionAccountRequest

Create an isolated private CLI home; the person runs its external login command.

| Field | Type | Required | Description |
|---|---|---|---|
| `label` | string | yes | Non-control person-assigned label,1–80 characters. |

Unknown fields are rejected.

### CreateVaultItemRequest

`POST /v1/vault/items`: a login (password, optional username and authenticator) or an authenticator alone (`totp` without `password`). Values only travel to the daemon; the response is value-free [`VaultItem`] metadata.

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | New item id, `[a-z0-9:_-]{1,64}`. |
| `origins` | string[] | yes | Allowed origins (`https://host`, `https://*.host`, `http://localhost:port`), at least one. |
| `password` | string \| null | no | Password for a login. Default: `null`. |
| `policy` | [VaultPolicy](#vaultpolicy) \| null | no | Per-item policy; null inherits the vault default. Default: `null`. |
| `totp` | string \| null | no | Authenticator: an `otpauth://totp/...` URI or a bare base32 setup key. Default: `null`. |
| `username` | string \| null | no | Username for a login. Default: `null`. |

Unknown fields are rejected.

### DaemonStatus

`GET /v1/daemon`: identity of the running daemon process.

| Field | Type | Required | Description |
|---|---|---|---|
| `boot_id` | string | yes | Random id of this process; a different value after a restart. |
| `restart_method` | [RestartMethod](#restartmethod) | yes | How `POST /v1/daemon/restart` would restart it. |
| `started_at` | string (date-time) | yes | When this process started. |
| `version` | string | yes | Daemon version. |

### Device

A paired device (never its token).

| Field | Type | Required | Description |
|---|---|---|---|
| `created_at` | string (date-time) \| null (date-time) | no | Pairing time, when known. |
| `device_id` | string | yes | Revocation identifier (`DELETE /v1/devices/{id}`). |
| `kind` | [DeviceKind](#devicekind) | no | Pairing-time trust role; legacy devices without recorded source default to person_device. Default: `"person_device"`. |
| `name` | string | yes | Label given at pairing; empty for devices paired before labels were stored. |
| `scopes` | [CapabilityScope](#capabilityscope)[] | yes | Scopes the token holds. |

### DeviceKind

Trust role fixed at pairing, not selected anew by each authenticated request.

| Value | Description |
|---|---|
| `"person_device"` | A web, phone, extension or channel client that conveys a person's response. |
| `"agent_client"` | A CLI, MCP or API agent that must wait for a person device's approval. |

### DeviceList

Paired devices.

| Field | Type | Required | Description |
|---|---|---|---|
| `devices` | [Device](#device)[] | yes | Devices in pairing order. |

### DoctorCheck

One diagnostic finding.

| Field | Type | Required | Description |
|---|---|---|---|
| `fix` | [DoctorFix](#doctorfix) \| null | no | A fix the web app offers, when one exists. |
| `id` | string | yes | Stable check id such as `memory.embeddings`. |
| `next_step` | string \| null | no | What to do about it in words; may mention a terminal command. |
| `status` | [DoctorStatus](#doctorstatus) | yes | Outcome. |
| `summary` | string | yes | What was found, without secret values. |

### DoctorFix

A fix the web app can offer for a finding.

| Field | Type | Required | Description |
|---|---|---|---|
| `action` | [DoctorFixAction](#doctorfixaction) | yes | What the button does. |
| `hint` | string | yes | One sentence on what to expect. |
| `label` | string | yes | Button label. |

### DoctorFixAction

Where in the web app a doctor finding can be fixed.

| Value | Description |
|---|---|
| `"download_memory_model"` | Download the memory embedding model (Memory page). |
| `"restart_daemon"` | Restart the daemon. |
| `"open_models"` | Open the model and API key settings. |
| `"open_vault"` | Open the vault. |
| `"open_remote"` | Open remote access. |
| `"open_configuration"` | Open the configuration editor. |
| `"open_agents"` | Open the agents list (browser skill for other agents). |
| `"open_policy"` | Open the network and policy screen. |

### DoctorGroup

Findings of one area.

| Field | Type | Required | Description |
|---|---|---|---|
| `checks` | [DoctorCheck](#doctorcheck)[] | yes | Findings in this area; nothing is hidden, passing families are not folded. |
| `name` | string | yes | Area name, in display order across the report. |

### DoctorReport

`GET /v1/doctor`: the same checks as `openside doctor`, grouped by area, run by the daemon.

| Field | Type | Required | Description |
|---|---|---|---|
| `generated_at` | string (date-time) | yes | When the checks ran. |
| `groups` | [DoctorGroup](#doctorgroup)[] | yes | Non-empty areas in display order. |
| `ok` | boolean | yes | No check failed (warnings allowed). |

### DoctorStatus

Outcome of one doctor check.

| Value | Description |
|---|---|
| `"ok"` | Nothing to do. |
| `"warn"` | Works, but something is missing or degraded. |
| `"fail"` | Broken: the feature does not work. |

### DropReason

Why a message was dropped (never silent).

| Value | Description |
|---|---|
| `"removed"` | The user removed it before delivery. |
| `"session_reset"` | The session was reset or deleted. |

### EditMessageRequest

Pending message text replacement.

| Field | Type | Required | Description |
|---|---|---|---|
| `text` | string | yes | Replacement text; only pending messages may be edited. |

Unknown fields are rejected.

### Event

Immutable durable record, with a gapless per-session sequence starting at one.

Common fields:

| Field | Type | Required | Description |
|---|---|---|---|
| `run_id` | string \| null | no | Runtime turn/run correlation, if applicable. |
| `seq` | integer (uint64) | yes | Assigned transactionally by the store, never by clients. |
| `session_id` | [SessionId](#sessionid) | yes | Session whose event log owns the record. |
| `ts` | string (date-time) | yes | Recording timestamp in RFC 3339 UTC. |

Variants:

- `kind=message.queued`: Message accepted durably into the queue. Fields: `attachments`: [Attachment](#attachment)[] (optional), `author_kind`: [MessageAuthorKind](#messageauthorkind) (optional), `from_agent`: string \| null (optional), `message_id`: [MessageId](#messageid), `mode`: [QueueMode](#queuemode), `text`: string.
- `kind=message.edited`: Pending message text replaced. Fields: `message_id`: [MessageId](#messageid), `text`: string.
- `kind=message.promoted`: Pending follow-up promoted to the steering lane. Fields: `message_id`: [MessageId](#messageid).
- `kind=message.moved`: Pending message moved to another position inside its lane. Fields: `message_id`: [MessageId](#messageid), `position`: integer (uint32).
- `kind=message.injected`: Queue head reserved for at-most-once runtime dispatch. Fields: `message_id`: [MessageId](#messageid).
- `kind=message.completed`: Terminal result for a previously injected message. Fields: `message_id`: [MessageId](#messageid), `outcome`: [MessageOutcome](#messageoutcome), `reason`: string \| null (optional).
- `kind=message.dropped`: Pending message explicitly dropped with a terminal reason. Fields: `message_id`: [MessageId](#messageid), `reason`: [DropReason](#dropreason).
- `kind=turn.started`: Agent turn began.
- `kind=turn.ended`: Agent turn ended.
- `kind=assistant.delta`: Incremental assistant output. Fields: `text`: string.
- `kind=assistant.final`: Complete assistant response. Fields: `text`: string.
- `kind=thinking.delta`: Incremental model thinking output. Fields: `text`: string.
- `kind=usage`: Token usage for a model request. Fields: `input_tokens`: integer (uint64), `model`: string, `output_tokens`: integer (uint64).
- `kind=tool.started`: Tool call began. Fields: `args`: any, `name`: string, `tool_call_id`: string.
- `kind=tool.updated`: Tool produced intermediate progress. Fields: `tool_call_id`: string, `update`: any.
- `kind=tool.ended`: Tool completed or failed. Fields: `card`: [Card](#card) \| null (optional), `is_error`: boolean, `result`: any, `tool_call_id`: string.
- `kind=artifact.saved`: A tool created or modified a file now kept in the session's artifact directory. Fields: `mime`: string, `path`: string, `size`: integer (uint64), `tool`: string, `tool_call_id`: string.
- `kind=ticket.opened`: A durable pending human-intervention ticket was created. Fields: `ticket`: [Ticket](#ticket).
- `kind=ticket.resolved`: Pending ticket resolved. Fields: `card`: [Card](#card) \| null (optional), `resolution`: any, `ticket_id`: [TicketId](#ticketid).
- `kind=ticket.rejected`: Pending ticket rejected. Fields: `card`: [Card](#card) \| null (optional), `reason`: string, `ticket_id`: [TicketId](#ticketid).
- `kind=ticket.expired`: Pending ticket expired. Fields: `card`: [Card](#card) \| null (optional), `ticket_id`: [TicketId](#ticketid).
- `kind=card.presented`: Validated read-only presentation, idempotently keyed by its original tool call. Fields: `card`: [Card](#card), `tool_call_id`: string.
- `kind=subagent.started`: The parent's `delegate` call was admitted and a child agent started. Fields: `background`: boolean (optional), `model`: string, `parent_tool_call_id`: string, `subagent_id`: string, `task`: string, `tools`: string[].
- `kind=subagent.ended`: A child agent reached a terminal state; its result went to the parent. Fields: `output`: string \| null (optional), `reason`: string \| null (optional), `status`: [SubagentStatus](#subagentstatus), `subagent_id`: string.
- `kind=subagent.lifecycle`: Subagent lifecycle transition. Fields: `status`: string, `subagent_id`: string.
- `kind=subagent.progress`: Subagent progress update. Fields: `progress`: any, `subagent_id`: string.
- `kind=tab.opened`: Agent-owned browser tab created. Fields: `tab_id`: string, `url`: string.
- `kind=tab.closed`: Browser tab closed. Fields: `tab_id`: string.
- `kind=tab.asleep`: Browser tab discarded or unloaded. Fields: `tab_id`: string.
- `kind=session.driver_recorded`: An agent-client identity drove the session and cannot approve its later confirmations. Fields: `device_id`: string.
- `kind=session.created`: First event in a session log. Fields: `agent_id`: string \| null (optional), `browser_space`: [BrowserSpace](#browserspace) (optional), `incognito`: boolean (optional), `mode`: [PolicyMode](#policymode) (optional), `model`: string, `title`: string, `workspace`: string \| null (optional).
- `kind=session.settled`: Runtime has no active turn.
- `kind=session.stopped`: User explicitly stopped the session; pending messages are retained.
- `kind=session.deleted`: Session removed by an administrator; the log remains as a tombstone and nothing can be appended after it.
- `kind=session.updated`: User-visible list metadata changed; omitted fields are unchanged. Fields: `archived`: boolean \| null (optional), `mode`: [PolicyMode](#policymode) \| null (optional), `pinned`: boolean \| null (optional), `title`: string \| null (optional).
- `kind=session.title_proposed`: A model-written conversation title; explicit titles and later user renames win. Fields: `title`: string.
- `kind=runtime.started`: Sidecar ready handshake completed with an exact tool-gate match. Fields: `startup_ms`: integer (uint64), `tools`: string[].
- `kind=runtime.error`: Sanitized runtime failure; credentials and secrets must not appear here. Fields: `code`: string, `message`: string, `retryable`: boolean (optional), `user_message`: string (optional).
- any What an agent is doing in a browser tab, derived by the daemon from the tool call and the page itself (labels come from the page, never from the model).
- `kind=connection.saved`: A completed sign-in saved value-free connection metadata. Fields: `connection_id`: string, `identity`: string \| null (optional), `site`: string.
- `kind=connection.used`: A granted agent use; no request values enter the event. Fields: `connection_id`: string, `method`: string \| null (optional), `mode`: [ConnectionUseMode](#connectionusemode), `site`: string.
- `kind=team.hired`: A completed hire, correlated to its human authorization. Fields: `agent_id`: string, `card`: [Card](#card) \| null (optional), `name`: string, `request_message_id`: [MessageId](#messageid) \| null (optional), `ticket_id`: string \| null (optional).
- `kind=team.handoff`: A durable work packet shared between the sender and recipient homes. Fields: `card`: [Card](#card) \| null (optional), `dedupe_key`: string, `from_agent`: string, `goal`: string, `handoff_id`: string, `handoff_kind`: [HandoffKind](#handoffkind), `to_agent`: string.
- `kind=team.handoff_ended`: A settled handoff whose report is delivered through the durable outbox. Fields: `card`: [Card](#card) \| null (optional), `handoff_id`: string, `status`: [HandoffStatus](#handoffstatus), `summary`: string \| null (optional).
- `kind=team.question`: A teammate requests a decision from the chief, not another teammate. Fields: `card`: [Card](#card) \| null (optional), `from_agent`: string, `options`: string[], `question_id`: string, `text`: string.
- `kind=team.answered`: The chief's answer was durably queued for the asking teammate. Fields: `card`: [Card](#card) \| null (optional), `question_id`: string.
- `kind=team.transferred`: Cross-store ownership updates have completed. Fields: `card`: [Card](#card) \| null (optional), `from_agent`: string, `responsibilities`: string[], `routine_ids`: string[], `to_agent`: string.
- `kind=team.retired`: A teammate is retired after its ownership saga. Fields: `agent_id`: string, `card`: [Card](#card) \| null (optional), `successor`: string \| null (optional).
- `kind=team.restored`: A retired profile is active again without reclaiming transferred work. Fields: `agent_id`: string, `card`: [Card](#card) \| null (optional).

### EventKind

Public event payloads with stable `kind` identifiers, dotted except for `usage`.

- `kind=message.queued`: Message accepted durably into the queue. Fields: `attachments`: [Attachment](#attachment)[] (optional), `author_kind`: [MessageAuthorKind](#messageauthorkind) (optional), `from_agent`: string \| null (optional), `message_id`: [MessageId](#messageid), `mode`: [QueueMode](#queuemode), `text`: string.
- `kind=message.edited`: Pending message text replaced. Fields: `message_id`: [MessageId](#messageid), `text`: string.
- `kind=message.promoted`: Pending follow-up promoted to the steering lane. Fields: `message_id`: [MessageId](#messageid).
- `kind=message.moved`: Pending message moved to another position inside its lane. Fields: `message_id`: [MessageId](#messageid), `position`: integer (uint32).
- `kind=message.injected`: Queue head reserved for at-most-once runtime dispatch. Fields: `message_id`: [MessageId](#messageid).
- `kind=message.completed`: Terminal result for a previously injected message. Fields: `message_id`: [MessageId](#messageid), `outcome`: [MessageOutcome](#messageoutcome), `reason`: string \| null (optional).
- `kind=message.dropped`: Pending message explicitly dropped with a terminal reason. Fields: `message_id`: [MessageId](#messageid), `reason`: [DropReason](#dropreason).
- `kind=turn.started`: Agent turn began.
- `kind=turn.ended`: Agent turn ended.
- `kind=assistant.delta`: Incremental assistant output. Fields: `text`: string.
- `kind=assistant.final`: Complete assistant response. Fields: `text`: string.
- `kind=thinking.delta`: Incremental model thinking output. Fields: `text`: string.
- `kind=usage`: Token usage for a model request. Fields: `input_tokens`: integer (uint64), `model`: string, `output_tokens`: integer (uint64).
- `kind=tool.started`: Tool call began. Fields: `args`: any, `name`: string, `tool_call_id`: string.
- `kind=tool.updated`: Tool produced intermediate progress. Fields: `tool_call_id`: string, `update`: any.
- `kind=tool.ended`: Tool completed or failed. Fields: `card`: [Card](#card) \| null (optional), `is_error`: boolean, `result`: any, `tool_call_id`: string.
- `kind=artifact.saved`: A tool created or modified a file now kept in the session's artifact directory. Fields: `mime`: string, `path`: string, `size`: integer (uint64), `tool`: string, `tool_call_id`: string.
- `kind=ticket.opened`: A durable pending human-intervention ticket was created. Fields: `ticket`: [Ticket](#ticket).
- `kind=ticket.resolved`: Pending ticket resolved. Fields: `card`: [Card](#card) \| null (optional), `resolution`: any, `ticket_id`: [TicketId](#ticketid).
- `kind=ticket.rejected`: Pending ticket rejected. Fields: `card`: [Card](#card) \| null (optional), `reason`: string, `ticket_id`: [TicketId](#ticketid).
- `kind=ticket.expired`: Pending ticket expired. Fields: `card`: [Card](#card) \| null (optional), `ticket_id`: [TicketId](#ticketid).
- `kind=card.presented`: Validated read-only presentation, idempotently keyed by its original tool call. Fields: `card`: [Card](#card), `tool_call_id`: string.
- `kind=subagent.started`: The parent's `delegate` call was admitted and a child agent started. Fields: `background`: boolean (optional), `model`: string, `parent_tool_call_id`: string, `subagent_id`: string, `task`: string, `tools`: string[].
- `kind=subagent.ended`: A child agent reached a terminal state; its result went to the parent. Fields: `output`: string \| null (optional), `reason`: string \| null (optional), `status`: [SubagentStatus](#subagentstatus), `subagent_id`: string.
- `kind=subagent.lifecycle`: Subagent lifecycle transition. Fields: `status`: string, `subagent_id`: string.
- `kind=subagent.progress`: Subagent progress update. Fields: `progress`: any, `subagent_id`: string.
- `kind=tab.opened`: Agent-owned browser tab created. Fields: `tab_id`: string, `url`: string.
- `kind=tab.closed`: Browser tab closed. Fields: `tab_id`: string.
- `kind=tab.asleep`: Browser tab discarded or unloaded. Fields: `tab_id`: string.
- `kind=session.driver_recorded`: An agent-client identity drove the session and cannot approve its later confirmations. Fields: `device_id`: string.
- `kind=session.created`: First event in a session log. Fields: `agent_id`: string \| null (optional), `browser_space`: [BrowserSpace](#browserspace) (optional), `incognito`: boolean (optional), `mode`: [PolicyMode](#policymode) (optional), `model`: string, `title`: string, `workspace`: string \| null (optional).
- `kind=session.settled`: Runtime has no active turn.
- `kind=session.stopped`: User explicitly stopped the session; pending messages are retained.
- `kind=session.deleted`: Session removed by an administrator; the log remains as a tombstone and nothing can be appended after it.
- `kind=session.updated`: User-visible list metadata changed; omitted fields are unchanged. Fields: `archived`: boolean \| null (optional), `mode`: [PolicyMode](#policymode) \| null (optional), `pinned`: boolean \| null (optional), `title`: string \| null (optional).
- `kind=session.title_proposed`: A model-written conversation title; explicit titles and later user renames win. Fields: `title`: string.
- `kind=runtime.started`: Sidecar ready handshake completed with an exact tool-gate match. Fields: `startup_ms`: integer (uint64), `tools`: string[].
- `kind=runtime.error`: Sanitized runtime failure; credentials and secrets must not appear here. Fields: `code`: string, `message`: string, `retryable`: boolean (optional), `user_message`: string (optional).
- any What an agent is doing in a browser tab, derived by the daemon from the tool call and the page itself (labels come from the page, never from the model).
- `kind=connection.saved`: A completed sign-in saved value-free connection metadata. Fields: `connection_id`: string, `identity`: string \| null (optional), `site`: string.
- `kind=connection.used`: A granted agent use; no request values enter the event. Fields: `connection_id`: string, `method`: string \| null (optional), `mode`: [ConnectionUseMode](#connectionusemode), `site`: string.
- `kind=team.hired`: A completed hire, correlated to its human authorization. Fields: `agent_id`: string, `card`: [Card](#card) \| null (optional), `name`: string, `request_message_id`: [MessageId](#messageid) \| null (optional), `ticket_id`: string \| null (optional).
- `kind=team.handoff`: A durable work packet shared between the sender and recipient homes. Fields: `card`: [Card](#card) \| null (optional), `dedupe_key`: string, `from_agent`: string, `goal`: string, `handoff_id`: string, `handoff_kind`: [HandoffKind](#handoffkind), `to_agent`: string.
- `kind=team.handoff_ended`: A settled handoff whose report is delivered through the durable outbox. Fields: `card`: [Card](#card) \| null (optional), `handoff_id`: string, `status`: [HandoffStatus](#handoffstatus), `summary`: string \| null (optional).
- `kind=team.question`: A teammate requests a decision from the chief, not another teammate. Fields: `card`: [Card](#card) \| null (optional), `from_agent`: string, `options`: string[], `question_id`: string, `text`: string.
- `kind=team.answered`: The chief's answer was durably queued for the asking teammate. Fields: `card`: [Card](#card) \| null (optional), `question_id`: string.
- `kind=team.transferred`: Cross-store ownership updates have completed. Fields: `card`: [Card](#card) \| null (optional), `from_agent`: string, `responsibilities`: string[], `routine_ids`: string[], `to_agent`: string.
- `kind=team.retired`: A teammate is retired after its ownership saga. Fields: `agent_id`: string, `card`: [Card](#card) \| null (optional), `successor`: string \| null (optional).
- `kind=team.restored`: A retired profile is active again without reclaiming transferred work. Fields: `agent_id`: string, `card`: [Card](#card) \| null (optional).

### EventPoll

A bounded durable event page for transports that buffer server-sent events.

| Field | Type | Required | Description |
|---|---|---|---|
| `events` | [Event](#event)[] | yes | Events in ascending per-session sequence order. |
| `last_seq` | integer (uint64) | yes | Exclusive replay cursor, including filtered subagent events. |

### ExchangePairingRequest

Exchange a pairing code for a device token (no bearer token needed).

| Field | Type | Required | Description |
|---|---|---|---|
| `code` | string | yes | Code from the pairing offer. |

Unknown fields are rejected.

### ExternalAgent

An agent CLI openside can install its browser skill for (`openside skills install`).

| Field | Type | Required | Description |
|---|---|---|---|
| `custom` | boolean | no | Added by the user rather than a preset: its folder can be changed or removed (`PUT`/`DELETE /v1/external-agents/{id}`). Default: `false`. |
| `detected` | boolean | yes | The agent's executable is on the daemon's `PATH` or its configuration directory exists. |
| `id` | string | yes | Preset name (`claude`, `codex`, `omp`) or an entry of `[skills.agents]`. |
| `label` | string | yes | Product name for display. |
| `skill_installed` | boolean | yes | `openside-browser` is already in the agent's skills directory. |
| `skills_dir` | string \| null | no | Where the skill would be (or is) installed, written `~/…` when under the daemon user's home; `null` when no directory is known. |

### ExternalAgentList

`GET /v1/external-agents`.

| Field | Type | Required | Description |
|---|---|---|---|
| `agents` | [ExternalAgent](#externalagent)[] | yes | The three built-in presets first, then configured agents by name. |

### FaviconRef

Canonical FaviconRef wire contract; presentation never grants authority.

- `type=origin`: Typed Origin presentation variant; source authorization remains unchanged. Fields: `origin`: string.
- `type=data`: Typed Data presentation variant; source authorization remains unchanged. Fields: `data_url`: string.

### FieldAnswerEncoding

Canonical FieldAnswerEncoding wire contract; presentation never grants authority.

| Value | Description |
|---|---|
| `"login"` | Typed Login presentation variant; source authorization remains unchanged. |
| `"form"` | Typed Form presentation variant; source authorization remains unchanged. |
| `"scalar"` | Typed Scalar presentation variant; source authorization remains unchanged. |

### FieldInitial

Canonical FieldInitial wire contract; presentation never grants authority.

- string Typed Text presentation variant; source authorization remains unchanged.
- boolean Typed Checked presentation variant; source authorization remains unchanged.

### Handoff

Durable one-assignee handoff, independent of its timeline presentation.

| Field | Type | Required | Description |
|---|---|---|---|
| `created_at` | string (date-time) | yes | Assignment creation time, preserved on status transitions. |
| `dedupe_key` | string | yes | Unique among open handoffs, preventing concurrent duplicate assignments. |
| `ended_at` | string (date-time) \| null (date-time) | no | Null until the handoff reaches a terminal state. |
| `from_agent` | string | yes | Chief identity in the first version's chief-mediated delegation model. |
| `id` | string | yes | Stable identity used for receipts and report delivery deduplication. |
| `kind` | [HandoffKind](#handoffkind) | yes | Review additionally requires an independently authored subject. |
| `message_id` | [MessageId](#messageid) | yes | Queued assignee message used to correlate settling with report delivery. |
| `packet` | [HandoffPacket](#handoffpacket) | yes | Full packet, not reconstructed from a rendered card. |
| `status` | [HandoffStatus](#handoffstatus) | yes | Terminal transitions release the open deduplication key. |
| `subject_handoff_id` | string \| null | no | Required review subject when independent-review checks apply. |
| `to_agent` | string | yes | Exactly one active assignee when created. |

### HandoffKind

Why the chief is handing work to one assignee.

| Value | Description |
|---|---|
| `"task"` | One self-contained job, not a lasting transfer. |
| `"ownership"` | Onboarding or a lasting responsibility transfer. |
| `"review"` | Independent review; the reviewer cannot be the subject's author. |

### HandoffList

History for profile and handoff navigation without polling model execution.

| Field | Type | Required | Description |
|---|---|---|---|
| `handoffs` | [Handoff](#handoff)[] | yes | Durable handoff records rather than transient progress-only events. |

### HandoffPacket

Self-contained work context; assignees do not inherit the chief's conversation.

| Field | Type | Required | Description |
|---|---|---|---|
| `acceptance` | string | yes | Required nonblank evidence that makes completion observable. |
| `constraints` | string | yes | Boundaries for this assignment; cannot widen daemon policy. |
| `context` | string | yes | Facts and prior results needed without reading another thread. |
| `goal` | string | yes | Required nonblank outcome the assignee should achieve. |

Unknown fields are rejected.

### HandoffStatus

Durable handoff state used for deduplication and terminal receipts.

| Value | Description |
|---|---|
| `"open"` | Holds its deduplication key until completion or cancellation. |
| `"done"` | The assignee delivered an outcome. |
| `"failed"` | The assignee could not meet the acceptance criteria. |
| `"cancelled"` | Explicitly ended without claiming successful execution. |

### Health

Health response without environment or credential details.

| Field | Type | Required | Description |
|---|---|---|---|
| `api_version` | string | yes | Public API version, currently `v1`. |
| `ok` | boolean | yes | Whether the daemon is ready to serve requests. |

### HireDecision

Person-authored response to a proposed hire, not authority supplied by a model.

| Value | Description |
|---|---|
| `"hire"` | Approve the authoritative draft with its permitted edits. |
| `"not_now"` | Decline this topic for the 30-day cooldown. |
| `"never"` | Suppress future proposals for the same topic permanently. |

### HireDraft

Authoritative hire payload shared by manual creation and approval tickets.

| Field | Type | Required | Description |
|---|---|---|---|
| `account_pool` | [ModelAccountRef](#modelaccountref)[] \| null | no | Null inherits provider membership; an empty list is rejected. |
| `channel` | [AgentChannelBinding](#agentchannelbinding) \| null | no | Validated against both channel settings and existing profile ownership. |
| `color` | string \| null | no | Null selects a stable id-derived value from ACCENT_COLORS. |
| `history_summary` | string | no | Initial handoff context; never injected as permanent persona rules. Default: `""`. |
| `initials` | string \| null | no | Null derives an initial from the supplied name. |
| `memory_ids` | string[] | no | Explicit memory transfers; unrelated notes remain with their current owners. Default: `[]`. |
| `mode` | [PolicyMode](#policymode) \| null | no | Null inherits the current default, bounded by the chief's ceiling. |
| `model` | string \| null | no | Null inherits the current default; it does not embed a fallback model. |
| `name` | string | yes | Validated against all existing profiles, including retired teammates. |
| `persona` | [AgentPersona](#agentpersona) | yes | Durable role rules, not the initial assignment's conversation history. |
| `proposal_key` | string \| null | no | Stable topic key for proposal cooldowns, absent for a manual hire without a topic. |
| `routine_ids` | string[] | no | Explicit ownership transfers recorded in the resumable hire saga. Default: `[]`. |
| `skills` | string[] \| null | no | Null inherits enabled skills; transfers add grants to explicit allowlists. |

Unknown fields are rejected.

### ImportAvailability

Known source-file presence; true does not guarantee successful decryption.

| Field | Type | Required | Description |
|---|---|---|---|
| `autofill` | boolean | yes | An address/form source exists. |
| `bookmarks` | boolean | yes | A bookmark source exists. |
| `cookies` | boolean | yes | A cookie source exists. |
| `extensions` | boolean | yes | An extension metadata source exists. |
| `history` | boolean | yes | A history source exists. |
| `passwords` | boolean | yes | A password record source exists. |
| `permissions` | boolean | yes | A site preference source exists. |
| `search_engines` | boolean | yes | A provider metadata source exists. |
| `tabs` | boolean | yes | An open-session source exists. |

### ImportBatchReport

Complete preview or final/partial summary; secret values are excluded.

| Field | Type | Required | Description |
|---|---|---|---|
| `dry_run` | boolean | yes | True for a read-only preview. |
| `extensions` | [ImportedExtension](#importedextension)[] | yes | Extension names and official store links, never binaries. |
| `permissions` | [ImportedPermission](#importedpermission)[] | yes | Site-preference inventory. |
| `profile` | string | yes | Agent destination, always default. |
| `profiles` | [ImportReport](#importreport)[] | yes | One outcome per selected source profile. |
| `restore_tabs` | [ImportedRestoreTab](#importedrestoretab)[] | yes | Current tabs for explicit user restoration. |
| `search_engines` | [ImportedSearchEngine](#importedsearchengine)[] | yes | Search-provider inventory. |
| `totals` | map of integer (uint64) | yes | Written/prospective counts summed by category. |
| `vault_locked` | boolean | yes | Selected passwords require unlocking Vault first. |
| `vault_needed` | boolean | yes | Selected passwords require creating Vault first. |
| `warnings` | string[] | yes | Safe per-kind and permission guidance. |

### ImportBrowser

Parser family; UI brand identity is carried separately on each profile.

| Value | Description |
|---|---|
| `"chrome"` | Google Chrome's profile formats. |
| `"chromium"` | Chromium-derived profile formats. |
| `"firefox"` | Firefox-derived profile formats. |
| `"safari"` | macOS Safari's permitted local files. |

### ImportCategory

Independent import categories; unavailable/protected kinds do not stop other kinds.

| Value | Description |
|---|---|
| `"history"` | Web visits retained for browser history. |
| `"bookmarks"` | Web bookmarks with folder paths. |
| `"cookies"` | Authenticated sessions written through the target browser. |
| `"passwords"` | Sign-ins saved exclusively into encrypted Vault. |
| `"autofill"` | Non-payment address/form fields. |
| `"tabs"` | Current tabs offered for explicit restoration. |
| `"search_engines"` | Search-provider inventory. |
| `"permissions"` | Site preference inventory, not openside authorization. |
| `"extensions"` | Extension labels/store links, never installations. |

### ImportCategoryReport

Per-kind outcome; credential preview counts source rows without decryption or key access.

| Field | Type | Required | Description |
|---|---|---|---|
| `imported` | integer (uint64) | yes | Written records, or source candidates during preview (credentials are not verified). |
| `note` | string \| null | no | Safe unavailable/failed guidance, or preview guidance about explicit import-time access. |
| `read` | integer (uint64) | yes | Source records successfully read or counted. |
| `rejected` | [ImportRejectedCookie](#importrejectedcookie)[] | yes | Rejected-cookie labels with no values. |
| `skipped` | integer (uint64) | yes | Excluded invalid, duplicate, expired or unmappable records. |

### ImportDeliveryAck

Acknowledgement sent only after the destination applied the batch.

| Field | Type | Required | Description |
|---|---|---|---|
| `target` | [ImportDeliveryTarget](#importdeliverytarget) | yes | Destination whose pending marker is cleared. |

Unknown fields are rejected.

### ImportDeliveryBatch

Native-delivery revision; cookie values exist only in the live local response.

| Field | Type | Required | Description |
|---|---|---|---|
| `bookmarks` | [ImportDeliveryBookmark](#importdeliverybookmark)[] | yes | Folder-aware web bookmarks. |
| `cookies` | [ImportDeliveryCookie](#importdeliverycookie)[] | yes | Live secret transport, excluded from the durable pending metadata. |
| `history` | [ImportDeliveryVisit](#importdeliveryvisit)[] | yes | Source visits for the native history API. |
| `id` | string | yes | Opaque revision id, replayed until that target acknowledges it. |
| `source` | [ImportDeliverySource](#importdeliverysource) | yes | Value-free source display metadata. |

### ImportDeliveryBookmark

Bookmark transported to the extension's supported native bookmarks API.

| Field | Type | Required | Description |
|---|---|---|---|
| `added_at_ms` | integer (int64) \| null (int64) | no | Source creation time in Unix milliseconds when known. |
| `folder` | string[] | yes | Folder names, outermost first. |
| `title` | string | yes | Source display title. |
| `url` | string | yes | HTTP(S), file or FTP URL, used with folder path for native dedupe. |

### ImportDeliveryCookie

Authenticated daemon-to-extension cookie transport; never persist, log or expose to pages.

| Field | Type | Required | Description |
|---|---|---|---|
| `domain` | string \| null | no | Domain scope; absent means host-only. |
| `expirationDate` | number (double) \| null (double) | no | Unix seconds; absent means a session cookie. |
| `httpOnly` | boolean | yes | Source HttpOnly attribute. |
| `name` | string | yes | Source cookie name. |
| `partitionKey` | [ImportDeliveryCookiePartitionKey](#importdeliverycookiepartitionkey) \| null | no | Live native partition boundary; current source parsers skip unmappable partitions. |
| `path` | string | yes | Source cookie path. |
| `sameSite` | string \| null | no | Chrome API SameSite enum string when recorded. |
| `secure` | boolean | yes | Source Secure attribute. |
| `url` | string | yes | URL establishing the cookie's supported origin. |
| `value` | string | yes | Secret value read live from the encrypted agent cookie store. |

### ImportDeliveryCookiePartitionKey

Chrome's partition boundary, preserved when supported rather than silently dropped.

| Field | Type | Required | Description |
|---|---|---|---|
| `hasCrossSiteAncestor` | boolean \| null | no | Chromium's cross-site-ancestor boundary when reported. |
| `topLevelSite` | string | yes | Schemeful top-level site owning the partition. |

### ImportDeliveryList

Value-free local-person delivery descriptors; the extension fetches the actual records.

| Field | Type | Required | Description |
|---|---|---|---|
| `batches` | [ImportDeliveryPending](#importdeliverypending)[] | yes | Pending revision descriptors, containing no native cookie values. |

### ImportDeliveryPending

A pending user-profile import revision with exact permission origins.

| Field | Type | Required | Description |
|---|---|---|---|
| `cookie_origins` | string[] | yes | Exact optional host patterns; no cookie values or source records. |
| `id` | string | yes | Stable native-delivery revision identifier. |

### ImportDeliveryQuery

Local-only delivery selector; it cannot name an arbitrary browser/profile path.

| Field | Type | Required | Description |
|---|---|---|---|
| `target` | [ImportDeliveryTarget](#importdeliverytarget) | no | Omitted means the dedicated desktop user profile. Default: `"user"`. |

### ImportDeliverySigned

Device-authenticated, HMAC-protected native import response.

| Field | Type | Required | Description |
|---|---|---|---|
| `batch_id` | string | yes | Native-delivery revision authenticated together with the payload. |
| `payload` | string | yes | Exact UTF-8 JSON signed with HMAC-SHA256; never forwarded to a page. |
| `signature` | string | yes | Lowercase hex HMAC-SHA256(ticket \|\| batch_id \|\| SHA256(payload) as hex). |
| `ticket` | string | yes | Exact single-use ticket consumed by this response. |

### ImportDeliverySource

Safe origin of a pending native-delivery batch.

| Field | Type | Required | Description |
|---|---|---|---|
| `brand` | string | yes | Stable source browser brand. |
| `id` | string | yes | Opaque discovered profile id. |
| `name` | string | yes | Profile display label. |

### ImportDeliveryTarget

Independently acknowledged destinations for the same migration revision.

| Value | Description |
|---|---|
| `"user"` | Dedicated desktop openside-browser profile. |
| `"agent"` | Managed agent browser profile and its host-side lookup store. |

### ImportDeliveryTicket

Short-lived authorization redeemed only by the designated extension device.

| Field | Type | Required | Description |
|---|---|---|---|
| `ticket` | string | yes | Single-use, short-lived capability bound to the paired extension device and batch. |

### ImportDeliveryTicketRequest

Local-person request to authorize one batch for a paired extension.

| Field | Type | Required | Description |
|---|---|---|---|
| `device_id` | string | yes | Extension's revocation identifier from its verified pairing. |

Unknown fields are rejected.

### ImportDeliveryVisit

Visit transported to the extension's supported native history API.

| Field | Type | Required | Description |
|---|---|---|---|
| `title` | string \| null | no | Source display title when known. |
| `url` | string | yes | Supported web URL used for native history dedupe. |
| `visited_at_ms` | integer (int64) | yes | Source time in Unix milliseconds. |

### ImportJob

Pollable job status; never contains source credential values.

| Field | Type | Required | Description |
|---|---|---|---|
| `completed_profiles` | integer (uint64) | yes | Fully processed source profiles. |
| `current_profile` | string \| null | no | Currently processed opaque source id. |
| `error` | string \| null | no | Stable failure code, never exception text. |
| `id` | string | yes | Random opaque job id. |
| `state` | [ImportJobState](#importjobstate) | yes | Current or terminal lifecycle state. |
| `summary` | [ImportBatchReport](#importbatchreport) \| null | no | Final or partial safe report once processing ends. |
| `total_profiles` | integer (uint64) | yes | Selected source profiles at admission. |

### ImportJobState

Durable background job lifecycle.

| Value | Description |
|---|---|
| `"running"` | Admitted job still processing profiles. |
| `"completed"` | Every selected profile was processed; category warnings may remain. |
| `"failed"` | Processing ended early; completed profiles remain in the summary. |

### ImportPasswordsRequest

Local-person CSV upload parsed in memory directly into encrypted Vault.

| Field | Type | Required | Description |
|---|---|---|---|
| `csv` | string | yes | User-selected UTF-8 export; never persist, log or echo this string. |

Unknown fields are rejected.

### ImportProfile

Safe local profile metadata; absolute source paths remain host-side.

| Field | Type | Required | Description |
|---|---|---|---|
| `available` | [ImportAvailability](#importavailability) | yes | Known source-file availability by category. |
| `avatar` | string \| null | no | Browser-local avatar identifier, never a remote image URL. |
| `brand` | string | yes | Stable browser brand/icon identifier. |
| `browser` | [ImportBrowser](#importbrowser) | yes | Family selecting the host-side parsers. |
| `email` | string \| null | no | Account label from Local State when present. |
| `id` | string | yes | Opaque, collision-safe selection identifier. |
| `name` | string | yes | Browser-provided display name with a directory-name fallback. |
| `warnings` | string[] | yes | Safe permission guidance, including Safari Full Disk Access. |

### ImportProfileList

Local detection response; preview supplies actual per-kind record counts.

| Field | Type | Required | Description |
|---|---|---|---|
| `cookies_writable` | boolean | yes | The target Chromium executable is available. |
| `profiles` | [ImportProfile](#importprofile)[] | yes | Every profile found in the supported standard locations. |
| `vault_locked` | boolean | yes | Detected password sources require unlocking the configured Vault. |
| `vault_needed` | boolean | yes | Detected password sources require creating Vault first. |
| `warnings` | string[] | yes | Safe whole-detection guidance, never filesystem paths. |

### ImportProgress

Credential-free durable SSE job progress payload.

| Field | Type | Required | Description |
|---|---|---|---|
| `completed_profiles` | integer (uint64) | yes | Fully processed profiles. |
| `current_profile` | string \| null | no | Currently processed source id, not a directory path. |
| `job_id` | string | yes | Job owning this progress record. |
| `state` | [ImportJobState](#importjobstate) | yes | Lifecycle after this record was committed. |
| `total_profiles` | integer (uint64) | yes | Profiles selected at job admission. |

### ImportRejectedCookie

Value-free label for a cookie the target browser rejected.

| Field | Type | Required | Description |
|---|---|---|---|
| `host` | string | yes | Source host/domain, never the cookie value. |
| `name` | string | yes | Source cookie name. |

### ImportReport

One selected profile's safe category reports.

| Field | Type | Required | Description |
|---|---|---|---|
| `autofill` | [ImportCategoryReport](#importcategoryreport) \| null | no | Selected address/form outcome. |
| `bookmarks` | [ImportCategoryReport](#importcategoryreport) \| null | no | Selected bookmark outcome. |
| `cookies` | [ImportCategoryReport](#importcategoryreport) \| null | no | Selected cookie outcome, never values. |
| `dry_run` | boolean | yes | True if no target data was written. |
| `extensions` | [ImportCategoryReport](#importcategoryreport) \| null | no | Selected extension-inventory outcome. |
| `history` | [ImportCategoryReport](#importcategoryreport) \| null | no | Selected history outcome. |
| `id` | string | yes | Opaque source profile id. |
| `passwords` | [ImportCategoryReport](#importcategoryreport) \| null | no | Selected Vault-password outcome, never values. |
| `permissions` | [ImportCategoryReport](#importcategoryreport) \| null | no | Selected site-preference outcome. |
| `profile` | string | yes | Agent destination profile, always default. |
| `search_engines` | [ImportCategoryReport](#importcategoryreport) \| null | no | Selected search-provider outcome. |
| `source` | string | yes | Source browser brand. |
| `tabs` | [ImportCategoryReport](#importcategoryreport) \| null | no | Selected current-tab outcome. |

### ImportRequest

Selection shared by preview and run; omitted/null means every profile/kind.

| Field | Type | Required | Description |
|---|---|---|---|
| `profiles` | string[] \| null | no | Discovered opaque ids; an explicit empty array is invalid. Default: `null`. |
| `what` | [ImportCategory](#importcategory)[] \| null | no | Category selection; an explicit empty array is invalid. Default: `null`. |

Unknown fields are rejected.

### ImportedExtension

Imported extension inventory, not an installation request.

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Browser/store extension id. |
| `name` | string | yes | Source human-readable label. |
| `profile_id` | string | yes | Source profile selector. |
| `store_url` | string \| null | no | Official store page when identifiable. |

### ImportedPermission

Source site-preference metadata, not a grant to openside tools.

| Field | Type | Required | Description |
|---|---|---|---|
| `origin` | string | yes | Source origin or complete primary/secondary match pattern. |
| `permission` | string | yes | Source capability label. |
| `profile_id` | string | yes | Source profile selector. |
| `setting` | string | yes | Source browser-specific allow/block/default value. |

### ImportedRestoreTab

A current tab offered as a restore link; it is never automatically opened.

| Field | Type | Required | Description |
|---|---|---|---|
| `profile_id` | string | yes | Opaque profile that supplied the tab. |
| `title` | string | yes | Source tab display title. |
| `url` | string | yes | Current supported web navigation. |

### ImportedSearchEngine

Search-provider metadata shown without altering the browser's default.

| Field | Type | Required | Description |
|---|---|---|---|
| `keyword` | string | yes | Source omnibox shortcut. |
| `name` | string | yes | Source provider display label. |
| `profile_id` | string | yes | Source profile selector. |
| `url` | string | yes | Provider substitution template. |

### InputField

Canonical InputField wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `autocomplete` | string \| null | no | Standard browser autocomplete metadata, not executable rules. |
| `card` | [CardField](#cardfield) \| null | no | Daemon-owned payment semantic; every payment field is secret. |
| `id` | string | yes | Stable identity within its card namespace; never an authorization token. |
| `initial` | [FieldInitial](#fieldinitial) \| null | no | Nonsecret initial value; prohibited for secret, OTP, and payment fields. |
| `kind` | [InputKind](#inputkind) | yes | Semantic presentation family, not permission to act. |
| `label` | string | yes | Plain-text accessible label; never markup. |
| `max_length` | integer (uint16) | yes | Maximum accepted characters, at most 4096 with tighter semantic limits. |
| `otp` | boolean | yes | Requires secret handling and bounded site-channel validation. |
| `placeholder` | string \| null | no | Nonsecret plain-text hint; never a saved answer. |
| `required` | boolean | yes | Empty input is rejected only when this flag is true. |
| `secret` | boolean | yes | Transient answer must never reach model, transcript, or logs. |
| `suggestions` | string[] | yes | Explicit nonsecret suggestions only, never derived from a transient answer. |
| `target_origin` | string \| null | no | Verified exact destination frame origin, not merely the merchant origin. |
| `validation` | [InputValidation](#inputvalidation) | yes | Fixed daemon validation vocabulary; no caller-defined regex. |

Unknown fields are rejected.

### InputKind

Canonical InputKind wire contract; presentation never grants authority.

| Value | Description |
|---|---|
| `"text"` | Typed Text presentation variant; source authorization remains unchanged. |
| `"email"` | Typed Email presentation variant; source authorization remains unchanged. |
| `"tel"` | Typed Tel presentation variant; source authorization remains unchanged. |
| `"password"` | Typed Password presentation variant; source authorization remains unchanged. |
| `"checkbox"` | Typed Checkbox presentation variant; source authorization remains unchanged. |
| `"textarea"` | Typed Textarea presentation variant; source authorization remains unchanged. |

### InputValidation

Canonical InputValidation wire contract; presentation never grants authority.

| Value | Description |
|---|---|
| `"text"` | Typed Text presentation variant; source authorization remains unchanged. |
| `"email"` | Typed Email presentation variant; source authorization remains unchanged. |
| `"phone"` | Typed Phone presentation variant; source authorization remains unchanged. |
| `"otp"` | Typed Otp presentation variant; source authorization remains unchanged. |
| `"card_number"` | Typed CardNumber presentation variant; source authorization remains unchanged. |
| `"card_expiry"` | Typed CardExpiry presentation variant; source authorization remains unchanged. |
| `"card_cvc"` | Typed CardCvc presentation variant; source authorization remains unchanged. |
| `"postal_code"` | Typed PostalCode presentation variant; source authorization remains unchanged. |

### InstallAgentSkillRequest

`POST /v1/external-agents/{id}/browser-skill` body.

| Field | Type | Required | Description |
|---|---|---|---|
| `force` | boolean | no | Replace an `openside-browser` skill that is already installed. Default: `false`. |

Unknown fields are rejected.

### InstalledAgentSkill

What the install wrote.

| Field | Type | Required | Description |
|---|---|---|---|
| `agent` | string | yes | Agent the skill was installed for. |
| `files` | integer (uint32) | yes | Files written. |
| `path` | string | yes | Final location of the skill directory. |
| `skill` | string | yes | Installed skill name (`openside-browser`). |

### KvRow

Canonical KvRow wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `label` | string | yes | Plain-text accessible label; never markup. |
| `value` | [CardValue](#cardvalue) | yes | Typed display value, never a transient secret answer. |

Unknown fields are rejected.

### ListItem

Canonical ListItem wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `facts` | string[] | yes | At most eight plain-text facts per item. |
| `id` | string | yes | Stable identity within its card namespace; never an authorization token. |
| `image` | [MediaRef](#mediaref) \| null | no | Daemon-materialized raster artifact in the current session. |
| `image_alt` | string \| null | no | Safe fallback text independent of successful image loading. |
| `link` | [CardLink](#cardlink) \| null | no | Explicit navigation only; never background fetching. |
| `name` | string | yes | Plain-text display name, not a verified identity claim. |
| `price` | [Money](#money) \| null | no | Captured price; not an authorization to purchase. |
| `quantity` | integer (uint32) \| null (uint32) | no | Optional item count between 1 and 9999. |
| `rating` | [Rating](#rating) \| null | no | Bounded integer rating metadata. |
| `site` | [CardSite](#cardsite) \| null | no | Source attribution with an exact origin boundary. |

Unknown fields are rejected.

### LocalAccess

Socket-only bootstrap for a desktop browser; never returned to TCP callers.

| Field | Type | Required | Description |
|---|---|---|---|
| `device_id` | string | yes | Local person-device identifier. |
| `kind` | [DeviceKind](#devicekind) | yes | Fixed person-device identity. |
| `scopes` | [CapabilityScope](#capabilityscope)[] | yes | Token scopes. |
| `secret` | string | yes | Per-boot locality proof; keep only in browser sessionStorage. |
| `token` | string | yes | Bearer token for the local person, distinct from the per-boot secret. |
| `url` | string \| null | no | Loopback launch URL with credentials in its fragment, or null without a listener. |

### LoginAlternativeKind

Kind of an alternative sign-in control on the page.

| Value | Description |
|---|---|
| `"provider"` | Sign in with another provider's account. |
| `"passkey"` | Sign in with a passkey or security key. |
| `"qr"` | Sign in by scanning a QR code. |
| `"other"` | Any other sign-in mode the page offers (one-time number, a login-mode tab). |

### LoginFieldKind

Kind of a field on a `login` ticket.

| Value | Description |
|---|---|
| `"identifier"` | The account name: user id, email address or phone number. |
| `"password"` | The password. |
| `"text"` | Any other text the form asks for. |
| `"checkbox"` | A tick box such as "keep me signed in"; answered with a boolean. |

### LoginProvider

Identity provider behind an alternative sign-in control.

| Value | Description |
|---|---|
| `"google"` | Google. |
| `"apple"` | Apple. |
| `"kakao"` | Kakao. |
| `"naver"` | Naver. |
| `"facebook"` | Facebook. |
| `"github"` | GitHub. |
| `"microsoft"` | Microsoft. |

### MediaRef

Canonical MediaRef wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Session-relative raster artifact; remote URLs and traversal are forbidden. |

Unknown fields are rejected.

### Memory

One stored memory. Content never contains a value matched by secret detection.

| Field | Type | Required | Description |
|---|---|---|---|
| `content` | string | yes | Remembered text. |
| `created_at` | string (date-time) | yes | First write. |
| `expires_at` | string (date-time) \| null (date-time) | no | Purge deadline; absent for permanent memories. |
| `id` | string | yes | Opaque memory identifier. |
| `kind` | [MemoryKind](#memorykind) | yes | Memory origin. |
| `owner` | string \| null | no | Null means shared; otherwise this memory belongs to one teammate. Default: `null`. |
| `retention` | [Retention](#retention) | yes | Retention policy. |
| `tags` | string[] | yes | Free-form labels. |
| `title` | string \| null | no | Page title, for history memories. |
| `updated_at` | string (date-time) | yes | Last write; retention counts from here. |
| `url` | string \| null | no | Public page URL, for history memories. |
| `version` | integer (uint32) | yes | Current version number, starting at one. |

### MemoryHealth

Response of `GET /v1/memory/health`.

| Field | Type | Required | Description |
|---|---|---|---|
| `count` | integer (uint64) | yes | Live (unexpired) memories. |
| `mode` | [MemorySearchMode](#memorysearchmode) | yes | Search strategy in effect. |
| `model_id` | string \| null | no | Content-derived identifier of the loaded local embedding model. |
| `next_step` | string \| null | no | Action that enables hybrid search. |
| `problem` | string \| null | no | Why embeddings are unavailable, without file contents. |

### MemoryHistory

Response of `GET /v1/memory/{id}/history`, oldest first.

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Memory identifier. |
| `versions` | [MemoryVersion](#memoryversion)[] | yes | Changes, oldest first. |

### MemoryHit

Ranked memory.

| Field | Type | Required | Description |
|---|---|---|---|
| `matched` | [MemoryMatch](#memorymatch) | yes | Index that matched. |
| `memory` | [Memory](#memory) | yes | The memory. |
| `score` | number (double) | yes | Fused rank score; larger is better and only comparable within one response. |

### MemoryKind

Origin of a memory: agent/user notes or automatically indexed browsing history.

| Value | Description |
|---|---|
| `"note"` | Written by the agent (`memory_write`) or a user (API/CLI). |
| `"history"` | Indexed page visit: public URL (no query/fragment/userinfo), title, short text. |

### MemoryMatch

Which index produced a search hit.

| Value | Description |
|---|---|
| `"lexical"` | Full-text match only. |
| `"vector"` | Embedding similarity only. |
| `"both"` | Both indexes. |
| `"recent"` | No query: most recently updated memories. |

### MemoryModelSource

Where the embedding model comes from.

| Field | Type | Required | Description |
|---|---|---|---|
| `endpoint` | string | yes | Model hub base URL. |
| `repo` | string | yes | Repository, `owner/name`. |
| `revision` | string | yes | Branch, tag or commit. |

### MemoryModelState

Stage of the embedding model download.

| Value | Description |
|---|---|
| `"idle"` | No download has run since the daemon started. |
| `"resolving"` | Reading the source's file list and checksums. |
| `"downloading"` | Transferring the model files. |
| `"verifying"` | Comparing checksums and loading the staged model. |
| `"installing"` | Swapping the verified model in. |
| `"installed"` | The last download installed a model. |
| `"failed"` | The last download failed; `error` says why. |
| `"cancelled"` | The last download was cancelled. |

### MemoryModelStatus

`GET /v1/memory/model`: the embedding model and the download job.

| Field | Type | Required | Description |
|---|---|---|---|
| `bytes_done` | integer (uint64) | yes | Bytes transferred so far. |
| `bytes_total` | integer (uint64) | yes | Bytes expected in total; zero until the source's file list is known. |
| `error` | [ApiError](#apierror) \| null | no | Why the last download failed or was cancelled (`code` such as `checksum_mismatch`). |
| `file` | string \| null | no | File being transferred. |
| `files_done` | integer (uint32) | yes | Files fully transferred. |
| `files_total` | integer (uint32) | yes | Files the model needs. |
| `finished_at` | string (date-time) \| null (date-time) | no | When the last download ended. |
| `installed` | boolean | yes | Both model files are present in `model_dir`. |
| `model_dir` | string | yes | Where the model lives (`memory.embedding.model_dir`). |
| `model_id` | string \| null | no | Id of the model the last download installed. |
| `restart_required` | boolean | yes | A model was installed after the daemon loaded its memory store; vector search starts after a restart. |
| `source` | [MemoryModelSource](#memorymodelsource) | yes | What a download without arguments fetches. |
| `state` | [MemoryModelState](#memorymodelstate) | yes | Stage of the current or last download. |

### MemoryOp

Change recorded in memory history.

| Value | Description |
|---|---|
| `"create"` | First write. |
| `"update"` | Edit. |
| `"delete"` | Explicit deletion. |
| `"purge"` | Removal by the retention job. |

### MemorySearchMode

Search strategy in effect.

| Value | Description |
|---|---|
| `"hybrid"` | FTS5 bm25 and local-embedding cosine, fused by reciprocal rank. |
| `"lexical_only"` | FTS5 bm25 only, because no local embedding model is loaded. |

### MemorySearchResponse

Response of `GET /v1/memory`.

| Field | Type | Required | Description |
|---|---|---|---|
| `hits` | [MemoryHit](#memoryhit)[] | yes | Hits, best first. |
| `mode` | [MemorySearchMode](#memorysearchmode) | yes | Strategy used for this query. |

### MemoryVersion

One history entry. After delete/purge every earlier snapshot is scrubbed and only operation, actor and timestamp remain.

| Field | Type | Required | Description |
|---|---|---|---|
| `actor` | string | yes | Who made the change, e.g. `api`, `agent:<session>`, `retention`. |
| `at` | string (date-time) | yes | When the change happened. |
| `content` | string \| null | no | Content after the change; absent once scrubbed. |
| `op` | [MemoryOp](#memoryop) | yes | Operation. |
| `retention` | [Retention](#retention) \| null | no | Retention after the change. |
| `tags` | string[] \| null | no | Labels after the change; absent once scrubbed. |
| `version` | integer (uint32) | yes | Version number this change produced. |

### Message

A message with durable lifecycle status.

| Field | Type | Required | Description |
|---|---|---|---|
| `author_kind` | [MessageAuthorKind](#messageauthorkind) | no | Preserves the durable caller authority rather than inferring it from message text. Default: `"internal"`. |
| `from_agent` | string \| null | no | Present only for daemon-authored team deliveries. Default: `null`. |
| `id` | [MessageId](#messageid) | yes | Idempotency key, assigned by the client or daemon. |
| `status` | [MessageStatus](#messagestatus) | yes | Pending or terminal disposition. |
| `text` | string | yes | Latest text, including edits made before delivery. |

### MessageAuthorKind

Assigned by the daemon from authenticated ingress; historical messages fail closed.

| Value | Description |
|---|---|
| `"person"` | Authenticated person ingress; eligible for immediate-hire provenance checks. |
| `"agent_client"` | A paired agent client, never equivalent to person approval. |
| `"internal"` | Schedules, team messages, and historical messages with unknown provenance. |

### MessageId

Identifies one user message. Assigned by the client or daemon and used for idempotent delivery.

Type: string

### MessageMode

Public delivery mode; `now` is queued durably before immediate steering.

| Value | Description |
|---|---|
| `"queue"` | Follow-up after the current turn. |
| `"steer"` | Inject at the running turn's next boundary. |
| `"now"` | Start or steer immediately. |

### MessageOutcome

Terminal runtime disposition, distinct from dispatch to the runtime.

| Value | Description |
|---|---|
| `"completed"` | The run completed successfully. |
| `"aborted"` | The user or runtime cancelled the message. |
| `"failed"` | Runtime failure or an interrupted, ambiguous dispatch. |
| `"deferred"` | The run paused on tickets; resolution resumes without a new message. |

### MessageStatus

Lifecycle of one message.

- `variant`: Waiting for delivery. Fields: `queued`: [QueueMode](#queuemode).
- `"delivered"` Handed to the agent runtime.
- `variant`: Will never be delivered. Fields: `dropped`: [DropReason](#dropreason).

### ModelAccount

A candidate account without token, email or upstream identity.

| Field | Type | Required | Description |
|---|---|---|---|
| `account_id` | string | yes | Opaque source-local reference; no upstream account id. |
| `available` | boolean | yes | Readiness after applying higher-priority fail-close barriers. |
| `blocked_until_ms` | number (double) \| null (double) | no | Latest applicable block expiry in Unix milliseconds, or unknown. Default: `null`. |
| `enabled` | boolean | no | Operator enablement, separate from credential readiness. Default: `false`. |
| `in_pool` | boolean | no | Explicit provider-pool membership. Default: `false`. |
| `label` | string | no | Person-assigned display label or an opaque source ordinal. Default: `""`. |
| `ready` | boolean | no | Local source readiness before pool membership/model blocks. Default: `false`. |
| `reason` | string | yes | Sanitized refusal or readiness explanation, never raw CLI output. |
| `source` | string | yes | Stable credential plugin name, distinct from provider namespace. |
| `usage_windows` | [AccountUsageWindow](#accountusagewindow)[] | no | Per-account measured windows; unknown usage remains null. Default: `[]`. |

Unknown fields are rejected.

### ModelAccountRef

Durable source-local account reference, without upstream identity or credential material.

| Field | Type | Required | Description |
|---|---|---|---|
| `account_id` | string | yes | Opaque durable id; reordering must never change the account it identifies. |
| `source` | string | yes | Credential plugin namespace, not an upstream authentication scheme. |

Unknown fields are rejected.

### ModelCatalog

Local credential readiness, never a guarantee of upstream model entitlement.

| Field | Type | Required | Description |
|---|---|---|---|
| `cached_for_ms` | integer (uint32) | yes | Maximum metadata cache lifetime; managed writes invalidate it immediately. |
| `default_model` | string \| null | no | Current default for new sessions and unoverridden Team homes on their next idle turn. |
| `fallback_models` | string[] | yes | Running model fallback chain, independent of credential-source order. |
| `providers` | [ModelProvider](#modelprovider)[] | yes | Provider groups sorted by identifier, with deduplicated model ids. |
| `truncated` | boolean | yes | An input or output metadata limit omitted entries. |

Unknown fields are rejected.

### ModelProvider

Chat/tool-capable candidates for one Pydantic AI provider namespace.

| Field | Type | Required | Description |
|---|---|---|---|
| `models` | [CatalogModel](#catalogmodel)[] | yes | Configured, curated and account-reported candidates, deduplicated by id. |
| `pool` | [ProviderPool](#providerpool) | yes | Person-built ordered account membership; empty explicitly disables routing. |
| `provider` | string | yes | SDK provider namespace, not an upstream account identifier. |

Unknown fields are rejected.

### ModelSelection

Result of a dedicated model-selection write, not raw config PUT semantics.

| Field | Type | Required | Description |
|---|---|---|---|
| `default_model` | string | yes | The newly running default, after successful private-file persistence. |
| `effective` | string | yes | new_sessions_and_home_next_turn: ordinary sessions retain stored models; homes reopen idle. |
| `fallback_models` | string[] | yes | The newly running ordered fallback chain. |
| `restart_required` | boolean | yes | Always false: dedicated selection applies without restarting the daemon. |

Unknown fields are rejected.

### Money

Canonical Money wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `currency` | string | yes | Upper-case ISO currency code. |
| `minor_units` | integer (int64) | yes | Signed integer amount in currency minor units, without floating point. |

Unknown fields are rejected.

### MoveMessageRequest

Pending message reorder within its delivery lane.

| Field | Type | Required | Description |
|---|---|---|---|
| `position` | integer (uint32) | yes | Zero-based index inside the message's own lane (steer or queue); values past the end move it last. A follow-up becomes a steer only through promote. |

Unknown fields are rejected.

### MyDevice

The calling device's own record.

| Field | Type | Required | Description |
|---|---|---|---|
| `device_id` | string | yes | Revocation identifier. |
| `kind` | [DeviceKind](#devicekind) | yes | Pairing-time trust role. |
| `name` | string | yes | Label given at pairing. |
| `request` | [AccessRequest](#accessrequest) \| null | no | This device's pending request for more access, if one is open. |
| `scopes` | [CapabilityScope](#capabilityscope)[] | yes | Scopes the token holds now. |

### NetworkPolicy

`GET`/`PUT /v1/policy/network`: where the daemon and its agents may connect, beyond this computer and the configured model and channel endpoints.

| Field | Type | Required | Description |
|---|---|---|---|
| `allow` | string[] | yes | Saved `network.allow` list. |
| `restart_required` | boolean | yes | Whether the saved list differs from the one the running daemon enforces; it applies after a restart. |

### NoticePlacement

Canonical NoticePlacement wire contract; presentation never grants authority.

| Value | Description |
|---|---|
| `"inline"` | Typed Inline presentation variant; source authorization remains unchanged. |
| `"bubble"` | Typed Bubble presentation variant; source authorization remains unchanged. |

### NoticeTone

Canonical NoticeTone wire contract; presentation never grants authority.

| Value | Description |
|---|---|
| `"neutral"` | Typed Neutral presentation variant; source authorization remains unchanged. |
| `"info"` | Typed Info presentation variant; source authorization remains unchanged. |
| `"working"` | Typed Working presentation variant; source authorization remains unchanged. |
| `"success"` | Typed Success presentation variant; source authorization remains unchanged. |
| `"attention"` | Typed Attention presentation variant; source authorization remains unchanged. |
| `"danger"` | Typed Danger presentation variant; source authorization remains unchanged. |

### OnboardingState

Where the first-run guide stands. `pending` until a person finishes or skips it once; the flag lives on the daemon, so every device and browser agrees.

| Value | Description |
|---|---|
| `"pending"` | Never finished or skipped. |
| `"completed"` | Walked through to the end. |
| `"skipped"` | Dismissed before the end. |

### OnboardingStatus

`GET /v1/onboarding` and the `PUT` response.

| Field | Type | Required | Description |
|---|---|---|---|
| `state` | [OnboardingState](#onboardingstate) | yes | Current state. |
| `updated_at` | string (date-time) \| null (date-time) | no | When the state last changed; `null` while pending. |

### OtpChannel

Where the site sends the one-time code a `request_otp` ticket asks for, as the agent read it on the page.

| Value | Description |
|---|---|
| `"authenticator"` | An authenticator app (TOTP). |
| `"sms"` | A text message. |
| `"email"` | An email. |
| `"backup_code"` | A printed backup or recovery code. |
| `"unknown"` | The page does not say. |

### OtpIngestRequest

A forwarded SMS message.

| Field | Type | Required | Description |
|---|---|---|---|
| `sender` | string | yes | Sender as the phone reports it; must be in `otp.senders`. |
| `text` | string | yes | Message text; only the code extracted by a configured site pattern is kept. |

Unknown fields are rejected.

### OtpIngestResponse

Outcome of an OTP ingest; never echoes the code.

| Field | Type | Required | Description |
|---|---|---|---|
| `matched_sites` | string[] | yes | Site origins whose pattern matched the text. |
| `resolved_tickets` | string[] | yes | Pending `secret_input` tickets filled with the code now. |
| `stored` | integer (uint32) | yes | Codes kept in memory for a step that has not opened yet. |

### PageFacts

Facts extracted from the page, bounded for display.

| Field | Type | Required | Description |
|---|---|---|---|
| `error_text` | string \| null | no | Error text shown by the page. |
| `fields` | [PageField](#pagefield)[] | no | Form fields by accessible label (at most 8). |
| `heading` | string \| null | no | Main heading. |
| `result_count` | integer (uint32) \| null (uint32) | no | Number of items in the main list or results. |

### PageField

One form field.

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Input kind: `text`, `email`, `password`, `code`, `checkbox`, `radio`, `select`, `button`, …. |
| `label` | string | yes | Accessible label. |

### PageKind

Page classification for card layout.

| Value | Description |
|---|---|
| `"login"` | Sign-in form (password field). |
| `"otp"` | One-time code entry. |
| `"choice"` | A set of options to pick from. |
| `"captcha"` | Human verification challenge. |
| `"form"` | Other form. |
| `"list"` | Search results or another list of items. |
| `"article"` | Reading content. |
| `"checkout"` | Payment or order confirmation. |
| `"error"` | Error page. |
| `"other"` | Nothing more specific. |

### PairDeviceRequest

Administrative device pairing request.

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | [DeviceKind](#devicekind) | no | Fixed at token creation; agent clients cannot approve their own work. Default: `"person_device"`. |
| `name` | string | yes | User-visible device label. |
| `scopes` | [CapabilityScope](#capabilityscope)[] | yes | Explicit capabilities to grant. |

Unknown fields are rejected.

### PairDeviceResponse

Administrative pairing result; the secret token is returned once.

| Field | Type | Required | Description |
|---|---|---|---|
| `device_id` | string | yes | Stable revocation identifier. |
| `import_secret` | string \| null | no | One-time extension import authenticator; never returned to a web page or logged. |
| `kind` | [DeviceKind](#devicekind) | no | Operator-selected trust role fixed when this token was paired. Default: `"person_device"`. |
| `scopes` | [CapabilityScope](#capabilityscope)[] | yes | Granted capabilities. |
| `token` | string | yes | Bearer capability token; never persist in event or application logs. |

### PairSyncRequest

`POST /v1/sync/pair`.

| Field | Type | Required | Description |
|---|---|---|---|
| `pairing_code` | string | yes | Code shown by the first device's sync setup. |
| `passphrase` | string \| null | no | Sync passphrase typed in the app; needs the computer's local access. |
| `webdav_password` | string \| null | no | WebDAV password typed in the app; needs the computer's local access. |

Unknown fields are rejected.

### PairingOffer

A one-time pairing offer, valid for [`PAIRING_TTL_SECS`].

| Field | Type | Required | Description |
|---|---|---|---|
| `code` | string | yes | One-time code; exchanged at most once. Show it only to the person pairing. |
| `expires_at` | string (date-time) | yes | Instant after which the code is refused. |
| `kind` | [DeviceKind](#devicekind) | no | Operator-selected role the resulting token will hold. Default: `"person_device"`. |
| `path` | string | yes | Pairing path on the daemon's origin, or the relay origin for relay links. The code is a fragment, absent from ordinary HTTP requests and Referer headers but readable by the page's scripts, including relay-served code. |
| `scopes` | [CapabilityScope](#capabilityscope)[] | yes | Scopes the resulting token will hold. |
| `transport` | [PairingTransport](#pairingtransport) \| null | no | Transport `url` uses; `null` for a plain `remote.public_url` or no URL. |
| `url` | string \| null | no | Link for the QR code: `<base>/pair#<code>` for direct transports, or `<relay>/pair#c=<code>&t=relay&d=<daemon id>&k=<daemon key>` for the relay. `null` when no transport or public URL is configured. |

### PairingTransport

How a paired device reaches the daemon from outside its network.

| Value | Description |
|---|---|
| `"tailscale"` | Tailscale Serve at `remote.public_url`. |
| `"cloudflare"` | Cloudflare Tunnel run by the daemon. |
| `"relay"` | CLI/MCP use Noise with the pairing-pinned daemon key; relay-served PWA code still requires trust in the relay operator. |

### PinnedTab

A tab the user pinned; synced between devices when sync is set up.

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Stable id derived from the URL, identical on every device. |
| `pinned_at` | string (date-time) | yes | When it was pinned (on the device that pinned it). |
| `title` | string \| null | no | Optional display title. |
| `url` | string | yes | http(s) URL. |

### PinnedTabList

`GET /v1/pinned-tabs`.

| Field | Type | Required | Description |
|---|---|---|---|
| `tabs` | [PinnedTab](#pinnedtab)[] | yes | Pinned tabs, oldest first. |

### PolicyCapability

What an action does; targets are absolute paths for `fs_*`/`exec`, URLs or origins otherwise.

| Value | Description |
|---|---|
| `"fs_read"` | Read a file or directory. |
| `"fs_write"` | Write, delete or rename a file. |
| `"exec"` | Run a command (target: working directory). |
| `"browse"` | Drive a browser page. |
| `"fetch"` | Network fetch outside the browser. |
| `"vault_use"` | Use a saved credential through the vault. |
| `"sensitive"` | Payment, purchase, posting or sending a message. |
| `"privacy_read"` | Access private imported records subject to an explicit session person grant. |

### PolicyDecision

Result of a [`PolicyTestRequest`].

| Field | Type | Required | Description |
|---|---|---|---|
| `effect` | [PolicyEffect](#policyeffect) | yes | Final effect. |
| `reason` | string | yes | `deny_rule`, `origin_denied`, `origin_not_allowed`, `rule`, `mode` or `sensitive_floor`. |

### PolicyEffect

Rule or decision outcome, ordered `allow < ask < deny`.

| Value | Description |
|---|---|
| `"allow"` | Proceed without asking. |
| `"ask"` | Open a confirm ticket first. |
| `"deny"` | Block; approval cannot override it. |

### PolicyList

Where a list of a policy edit is wrong.

| Value | Description |
|---|---|
| `"allow_origins"` | `allow_origins`. |
| `"deny_origins"` | `deny_origins`. |
| `"rules"` | `rules`. |
| `"network_allow"` | `network.allow` (outbound destinations). |

### PolicyMode

Public permission modes, mirroring `openside_policy::Mode`.

| Value | Description |
|---|---|
| `"read-only"` | Read only; deny mutations. |
| `"guard"` | Guard; work inside approved roots, ask elsewhere. |
| `"full"` | Full access; sensitive operations still require approval. |

### PolicyProblem

One validation problem, pointing at the entry to fix.

| Field | Type | Required | Description |
|---|---|---|---|
| `index` | integer (uint32) | yes | Zero-based position within the list. |
| `list` | [PolicyList](#policylist) | yes | List the entry belongs to. |
| `message` | string | yes | What is wrong, for the person editing. |

### PolicyRule

One Allow/Ask/Deny rule.

| Field | Type | Required | Description |
|---|---|---|---|
| `capability` | [PolicyCapability](#policycapability) | yes | Capability the rule applies to. |
| `effect` | [PolicyEffect](#policyeffect) | yes | Outcome when the rule matches. |
| `pattern` | string | yes | Glob over the action target: `*` within a `/` segment, `**` across segments. |

Unknown fields are rejected.

### PolicySettings

Live daemon policy: `GET`/`PUT /v1/policy`. A `PUT` replaces the whole document and applies to the next tool call of every session, including running ones.

| Field | Type | Required | Description |
|---|---|---|---|
| `allow_origins` | string[] | no | When non-empty, browser/fetch/vault/sensitive actions may only target these origin globs (`https://*.example.com`, `http://127.0.0.1:*`). Never grants anything. Default: `[]`. |
| `default_mode` | [PolicyMode](#policymode) | yes | Mode for sessions created without an explicit mode. |
| `deny_origins` | string[] | no | Origin globs that are always denied for browser/fetch/vault/sensitive actions. Default: `[]`. |
| `rules` | [PolicyRule](#policyrule)[] | no | Agent-level rules beneath each session's rules. A matching `deny` anywhere wins; `sensitive` actions can be tightened to `deny` but never allowed. Default: `[]`. |

Unknown fields are rejected.

### PolicyTestRequest

`POST /v1/policy/test`: evaluate one action against the live policy without running it.

| Field | Type | Required | Description |
|---|---|---|---|
| `capability` | [PolicyCapability](#policycapability) | yes | Capability to evaluate. |
| `mode` | [PolicyMode](#policymode) \| null | no | Session mode to evaluate under; defaults to `default_mode`. |
| `target` | string | yes | Path, URL or origin the action touches. |
| `workspace` | string \| null | no | Approved workspace root for Guard mode (absolute). |

Unknown fields are rejected.

### PolicyValidation

Result of a [`ValidatePolicyRequest`].

| Field | Type | Required | Description |
|---|---|---|---|
| `problems` | [PolicyProblem](#policyproblem)[] | yes | Every problem found, in list order. |
| `valid` | boolean | yes | No problems. |

### PresentImage

Canonical PresentImage wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `key` | string | yes | Unique image alias matching ASCII letters, digits, underscore, or hyphen. |
| `url` | string | yes | Validated HTTP(S) navigation URL without credentials or secret material. |

Unknown fields are rejected.

### PresentRequest

Canonical PresentRequest wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `card` | [Card](#card) | yes | Daemon-owned payment semantic; every payment field is secret. |
| `images` | [PresentImage](#presentimage)[] | yes | Transport-only image sources; never persisted as remote references. |

Unknown fields are rejected.

### PresentResult

Canonical PresentResult wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `card_id` | string | yes | Daemon-assigned presentation identity returned without duplicating the card. |

Unknown fields are rejected.

### PreviewEndReason

Terminal causes for a read-only stream; reconnecting does not revive a stopped session.

| Value | Description |
|---|---|
| `"revoked"` | The viewing device lost its capability, without affecting other viewers. |
| `"session_stopped"` | The session's stop event was committed. |
| `"session_deleted"` | The session became a tombstone and cannot expose browser content. |

### PreviewPauseReason

Temporary capture exclusions that leave the viewing connection open.

| Value | Description |
|---|---|
| `"takeover"` | A pending human step owns the tab's sole Chromium screencast. |

### PreviewServerMessage

Server-only JSON messages for the browse-scoped session preview WebSocket.

- `type=frame`: JPEG of the latest tab used successfully by this session's browser tools. Fields: `data`: string, `height`: integer (uint32), `tab_id`: string, `width`: integer (uint32).
- `type=withheld`: No pixels are disclosed after any secret fill in this tab's history. Fields: `code`: string, `tab_id`: string.
- `type=paused`: Capture resumes automatically when the human step releases the tab. Fields: `reason`: [PreviewPauseReason](#previewpausereason), `tab_id`: string.
- `type=no_tab`: No current live owned tab can be selected; no browser is launched by preview.
- `type=ended`: Final message before the socket's bounded close handshake. Fields: `reason`: [PreviewEndReason](#previewendreason).

### ProviderKeyInfo

The only provider-key metadata allowed in public responses.

| Field | Type | Required | Description |
|---|---|---|---|
| `created_at` | string (date-time) | yes | UTC creation/replacement time, encoded as RFC 3339. |
| `enabled` | boolean | yes | Operator enablement, independent of quota/authentication status. |
| `key_id` | string | yes | Durable opaque key account id, not upstream identity. |
| `label` | string | yes | Person-assigned display label. |
| `last_four` | string | yes | Exactly four trailing characters, never the complete key. |
| `provider` | string | yes | Managed API provider: anthropic, openai or google. |
| `reserve_percent` | integer (uint8) | yes | Remaining measured usage protected from automatic selection. |

Unknown fields are rejected.

### ProviderKeyList

Administrator-visible inventory; no credential values.

| Field | Type | Required | Description |
|---|---|---|---|
| `keys` | [ProviderKeyInfo](#providerkeyinfo)[] | yes | Metadata only; unavailable Vaults fail rather than returning fake entries. |

Unknown fields are rejected.

### ProviderPool

Ordered trust boundary for one SDK provider namespace.

| Field | Type | Required | Description |
|---|---|---|---|
| `accounts` | [ModelAccountRef](#modelaccountref)[] | yes | Explicit person-built membership/order; empty disables routing. |
| `provider` | string | yes | Exact SDK namespace; no implicit OpenAI/Codex aliasing. |

Unknown fields are rejected.

### ProviderPoolList

Public provider-pool inventory, without credential material.

| Field | Type | Required | Description |
|---|---|---|---|
| `pools` | [ProviderPool](#providerpool)[] | yes | Provider-sorted trust boundaries. |

Unknown fields are rejected.

### PushKeys

Subscription keys as `PushSubscription.toJSON().keys` reports them.

| Field | Type | Required | Description |
|---|---|---|---|
| `auth` | string | yes | 16-octet authentication secret, base64url. |
| `p256dh` | string | yes | Subscription P-256 public key, base64url. |

### PushNotification

Decrypted push payload the service worker receives.

| Field | Type | Required | Description |
|---|---|---|---|
| `body` | string | yes | Notification body (redacted, at most 200 characters). |
| `kind` | string | yes | `ticket` (a human step or approval is waiting) or `settled` (a run finished). |
| `session_id` | string | yes | Session concerned. |
| `ticket_id` | string \| null | no | Ticket concerned, for `ticket`. |
| `title` | string | yes | Notification title. |
| `url` | string | yes | Deep link opened on click: `/sessions/{id}` or `/sessions/{id}?ticket={tid}`. |

### PushPresenceRequest

Report whether the client owning a subscription is in the foreground. A focused client gets no push (it already shows the event); focus lapses after [`PUSH_FOCUS_TTL_SECS`] without a renewed report.

| Field | Type | Required | Description |
|---|---|---|---|
| `focused` | boolean | yes | `document.visibilityState === "visible" && document.hasFocus()`. |
| `subscription_id` | string | yes | The caller's subscription. |

Unknown fields are rejected.

### PushSubscription

A stored push subscription (keys omitted).

| Field | Type | Required | Description |
|---|---|---|---|
| `created_at` | string (date-time) | yes | Registration time. |
| `device_id` | string \| null | no | Device whose token registered it; `null` for the local socket. |
| `endpoint` | string | yes | Push service endpoint. |
| `id` | string | yes | Subscription identifier. |

### PushSubscriptionList

Push subscriptions visible to the caller.

| Field | Type | Required | Description |
|---|---|---|---|
| `subscriptions` | [PushSubscription](#pushsubscription)[] | yes | Subscriptions in registration order. |

### QueueMode

How a message should be delivered while a turn is running.

| Value | Description |
|---|---|
| `"queue"` | After the current turn finishes. |
| `"steer"` | Into the running turn at its next boundary. |

### Rating

Canonical Rating wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `count` | integer (uint32) \| null (uint32) | no | Optional public rating count. |
| `milli_max` | integer (uint32) | yes | Positive rating scale in thousandths, at most 10000. |
| `milli_value` | integer (uint32) | yes | Rating in thousandths, between zero and milli_max. |

Unknown fields are rejected.

### RejectTicketRequest

Explicit ticket rejection.

| Field | Type | Required | Description |
|---|---|---|---|
| `reason` | string \| null | no | Optional sanitized rejection detail. |

Unknown fields are rejected.

### RelayStatus

Relay registration state.

| Field | Type | Required | Description |
|---|---|---|---|
| `clients` | integer (uint32) | yes | Devices connected through the relay now. |
| `daemon_id` | string | yes | Routing id the relay knows this daemon by. |
| `daemon_key` | string | yes | Daemon X25519 public key (base64url) carried in relay pairing links. |
| `e2ee` | boolean | yes | Noise encrypts every daemon/device stream end-to-end. |
| `last_error` | string \| null | no | Latest failure, without secrets. |
| `state` | string | yes | `connecting`, `connected`, `backoff` or `failed` (local registration setup failed). |
| `url` | string | yes | Relay origin. |

### ReloginConnectionRequest

Select an existing shared session, or the default sign-in session.

| Field | Type | Required | Description |
|---|---|---|---|
| `session_id` | [SessionId](#sessionid) \| null | no | Omission selects the chief agent's durable home session. |

Unknown fields are rejected.

### ReloginResult

Relogin can finish silently using the Vault or require the person.

- `status=signed_in`: The existing sign-in flow completed without a human ticket.
- `status=needs_you`: Continue through an existing human-step or approval ticket. Fields: `ticket`: [TicketRef](#ticketref).

### RemoteConfiguration

Transport settings safe to inspect; no registration or tunnel token values.

| Field | Type | Required | Description |
|---|---|---|---|
| `cloudflare` | [SetCloudflareRequest](#setcloudflarerequest) | yes | Cloudflare settings. |
| `relay` | [SetRelayRequest](#setrelayrequest) | yes | Relay settings. |
| `tailscale` | [TailscaleConfiguration](#tailscaleconfiguration) | yes | Tailscale settings. |

### RemoteStatus

Remote transports the daemon runs (`GET /v1/remote/status`).

| Field | Type | Required | Description |
|---|---|---|---|
| `cloudflare` | [CloudflareStatus](#cloudflarestatus) \| null | no | The supervised `cloudflared`, when enabled. |
| `configuration` | [RemoteConfiguration](#remoteconfiguration) | yes | Persisted transport settings, without secret values or executable overrides. |
| `configure_allowed` | boolean | yes | This request proved local-person authority to change transport settings. |
| `modes` | [PairingTransport](#pairingtransport)[] | yes | Enabled transports in pairing preference order. |
| `pairing_allowed` | boolean | yes | This request proved local-person authority to issue an offer. |
| `public_url` | string \| null | no | `remote.public_url`. |
| `relay` | [RelayStatus](#relaystatus) \| null | no | The relay registration, when enabled. |
| `restart_required` | boolean | yes | A missing listener prevents a persisted transport from applying live. |
| `tailscale` | [TailscaleStatus](#tailscalestatus) | yes | Tailscale installation and the openside Serve handler. |

### ReplRequest

Run one Playwright-syntax snippet in a session-owned agent tab (`browser_repl`).

| Field | Type | Required | Description |
|---|---|---|---|
| `approval_ticket` | [TicketId](#ticketid) \| null | no | Approved `confirm` ticket from an earlier `deferred` response; the identical `code` and `tab_id` are rerun from the start under that approval. |
| `code` | string | yes | JavaScript using `page`, `expect`, and `console`; top-level `await` is allowed and the last expression statement is the return value. Each request gets a fresh context. |
| `tab_id` | string \| null | no | Agent tab to drive; optional when the session owns exactly one tab or none (a blank tab is opened). |

Unknown fields are rejected.

### ReplResponse

Result of a [`ReplRequest`], recorded as a `browser_repl` tool call in the session.

| Field | Type | Required | Description |
|---|---|---|---|
| `hint` | string \| null | no | How to proceed after a denial. |
| `reason` | string \| null | no | Denial reason or failure message. |
| `result` | any | no | Redacted, size-limited tool result when `status` is `done`. |
| `status` | [ReplStatus](#replstatus) | yes | Outcome category. |
| `ticket_id` | [TicketId](#ticketid) \| null | no | Approval ticket when `status` is `deferred`. |
| `tool_call_id` | string | yes | Tool call identifier recorded in the session's `tool.started`/`tool.ended` events. |

### ReplStatus

Outcome category of a [`ReplRequest`].

| Value | Description |
|---|---|
| `"done"` | The snippet ran; `result` holds `return_value`, `console`, optional `error`, `tab_id`. |
| `"deferred"` | Human approval is pending in `ticket_id`; rerun with `approval_ticket` once approved. |
| `"denied"` | Policy or validation refused the call; see `reason` and `hint`. |
| `"failed"` | The daemon could not run the snippet. |

### RequestAccessRequest

Ask for scopes the calling device does not hold yet.

| Field | Type | Required | Description |
|---|---|---|---|
| `scopes` | [CapabilityScope](#capabilityscope)[] | yes | Scopes wanted in addition to the ones the device holds; at least one. |

Unknown fields are rejected.

### ResolveTicketRequest

Human response interpreted according to the ticket kind. Secret-input values are daemon-only and must never be persisted or sent to the model.

| Field | Type | Required | Description |
|---|---|---|---|
| `hire` | [HireDraft](#hiredraft) \| null | no | Optional complete edited proposal, accepted only for a hire decision. |
| `value` | any | yes | Confirmation, non-secret information, or transient daemon-only secret input: a string, or `{"totp": "<otpauth:// URI or base32 key>"}` for a `secret_input` ticket whose `otp.can_register` is true. A `confirm` ticket carrying `otp_register` takes `true` or `false` like any confirmation. A `login` ticket takes `{"values": {<field id>: string \| bool}, "save": bool}`, `{"account": <item_id>}` or `{"alternative": <id>}`; a `choice` ticket takes `{"option": <id>}`. |

Unknown fields are rejected.

### RestartDaemonResponse

`POST /v1/daemon/restart` response: the restart has been requested, not finished.

| Field | Type | Required | Description |
|---|---|---|---|
| `boot_id` | string | yes | `boot_id` of the process being replaced; the restart is done when `GET /v1/daemon` answers with another one. |
| `method` | [RestartMethod](#restartmethod) | yes | How the restart is carried out. |

### RestartMethod

How the daemon restarts itself.

| Value | Description |
|---|---|
| `"re_exec"` | The daemon shuts down gracefully and executes itself in place with the same PID and arguments, including when supervised by systemd or launchd. |

### Retention

How long a memory is kept after its last write.

| Value | Description |
|---|---|
| `"permanent"` | Never purged automatically. |
| `"days_90"` | Purged 90 days after the last write. |
| `"days_30"` | Purged 30 days after the last write. |

### RetireAgentRequest

Retirement keeps history and records ownership movement for resumable recovery.

| Field | Type | Required | Description |
|---|---|---|---|
| `pause_routines` | boolean | no | Pause owned schedules instead of silently dropping them. Default: `false`. |
| `revert_hire` | boolean | no | Undo uses recorded pre-hire owners rather than guessing a successor. Default: `false`. |
| `successor` | string \| null | no | New owner; omission selects the chief unless routines are explicitly paused. |

Unknown fields are rejected.

### RevokeConnectionRequest

Forget is the default; stronger actions must be explicitly requested.

| Field | Type | Required | Description |
|---|---|---|---|
| `delete_vault_item` | boolean | no | Also delete the linked saved login, if present. Default: `false`. |
| `sign_out` | boolean | no | Also clear every shared-profile account's site data. Default: `false`. |

Unknown fields are rejected.

### Routine

A scheduled routine.

| Field | Type | Required | Description |
|---|---|---|---|
| `action` | [RoutineAction](#routineaction) | yes | Action performed on each run. |
| `catch_up` | boolean | yes | Run once for the latest occurrence missed while the daemon was down. |
| `created_at` | string (date-time) | yes | Creation time. |
| `enabled` | boolean | yes | Whether the scheduler fires this routine; automatic pauses clear it too. |
| `id` | string | yes | Opaque routine identifier assigned by the daemon. |
| `name` | string | yes | Human-readable name. |
| `next_run_at` | string (date-time) \| null (date-time) | no | Next scheduled occurrence; absent while the routine is not enabled. |
| `owner_agent_id` | string \| null | no | Derived from the action; not a separately stored ownership record. Default: `null`. |
| `paused_reason` | [RoutinePauseReason](#routinepausereason) \| null | no | Set when the daemon paused the routine on its own; cleared by resuming. |
| `schedule` | [RoutineSchedule](#routineschedule) | yes | Firing schedule. |

### RoutineAction

What a routine does when it fires.

- `type=new_session`: Create a fresh session and submit the prompt as its first message. Fields: `mode`: [PolicyMode](#policymode) \| null (optional), `model`: string \| null (optional), `prompt`: string, `workspace`: string \| null (optional).
- `type=agent`: Deliver to the active owner's permanent home thread. Fields: `agent_id`: string, `prompt`: string.
- `type=heartbeat`: Inject a follow-up (queue lane) message into an existing session. Fields: `prompt`: string, `session_id`: [SessionId](#sessionid).

### RoutineDraft

A repeated task found in the user's messages; never persisted until the user creates it.

| Field | Type | Required | Description |
|---|---|---|---|
| `first_seen` | string (date-time) | yes | First occurrence. |
| `last_seen` | string (date-time) | yes | Latest occurrence. |
| `name` | string | yes | Suggested routine name. |
| `occurrences` | integer (uint32) | yes | Number of occurrences the guess is based on. |
| `prompt` | string | yes | Most recent wording of the repeated prompt. |
| `schedule` | [RoutineSchedule](#routineschedule) | yes | Guessed schedule: cron when the spacing fits one, otherwise an interval. |
| `session_ids` | [SessionId](#sessionid)[] | yes | Sessions the occurrences came from. |

### RoutineList

Routine listing response, sorted by creation time.

| Field | Type | Required | Description |
|---|---|---|---|
| `routines` | [Routine](#routine)[] | yes | All routines. |

### RoutinePauseReason

Why a routine is not firing although the user did not disable it.

| Value | Description |
|---|---|
| `"target_missing"` | The heartbeat target session no longer exists. |
| `"agent_retired"` | The assigned agent is retired or no longer exists. |

### RoutineSchedule

When a routine fires.

- `type=cron`: POSIX five-field cron expression (minute hour day-of-month month day-of-week). Fields: `expr`: string, `timezone`: string \| null (optional).
- `type=interval`: Fixed period anchored at the moment the routine was created or last rescheduled. Fields: `secs`: integer (uint64).

### RoutineSuggestionList

Repetition candidates response.

| Field | Type | Required | Description |
|---|---|---|---|
| `suggestions` | [RoutineDraft](#routinedraft)[] | yes | Draft routines, most frequent first; none of them exists yet. |

### RunList

Run history response, newest first.

| Field | Type | Required | Description |
|---|---|---|---|
| `runs` | [RunRecord](#runrecord)[] | yes | Runs, newest first. |

### RunRecord

One entry of a routine's run history.

| Field | Type | Required | Description |
|---|---|---|---|
| `detail` | string \| null | no | Short sanitized explanation, e.g. why a run was skipped or failed. |
| `finished_at` | string (date-time) \| null (date-time) | no | When the run reached a terminal status. |
| `id` | integer (uint64) | yes | Monotonic run identifier, unique across routines. |
| `message_id` | [MessageId](#messageid) \| null | no | Message that carried the prompt, used to decide when the run has settled. |
| `routine_id` | string | yes | Routine that produced the run. |
| `scheduled_at` | string (date-time) | yes | Occurrence the run was scheduled for; the request time for manual runs. |
| `session_id` | [SessionId](#sessionid) \| null | no | Session that received the prompt, when one did. |
| `started_at` | string (date-time) \| null (date-time) | no | When the prompt was dispatched; absent for runs that never started. |
| `status` | [RunStatus](#runstatus) | yes | Current disposition. |

### RunStatus

Disposition of one scheduled or manual run.

| Value | Description |
|---|---|
| `"started"` | The prompt was handed to a session that has not settled yet. |
| `"completed"` | The session finished the run's message successfully. |
| `"failed"` | The run could not start, or its message failed, was aborted or dropped. |
| `"skipped_overlap"` | The previous run was still in progress, so this one was skipped. |
| `"paused_target_missing"` | The heartbeat target no longer exists; the routine was paused. |
| `"missed"` | The daemon was not running at the scheduled time and the run was not replayed. |

### Session

Session metadata and pending queue, in delivery order.

| Field | Type | Required | Description |
|---|---|---|---|
| `activity` | string | no | Short, redacted description of the current work, rebuilt from session events. Default: `""`. |
| `agent_id` | string \| null | no | Immutable owner tag; null keeps earlier conversations separate from agent homes. Default: `null`. |
| `archived` | boolean | no | Hidden from the default session list. Default: `false`. |
| `browser_space` | [BrowserSpace](#browserspace) | no | Browser space fixed at creation; historical sessions are shared. Default: `"shared"`. |
| `id` | [SessionId](#sessionid) | yes | Stable session identifier. |
| `incognito` | boolean | no | Private session fixed at creation: its browsing is never written to memory. Default: `false`. |
| `last_seq` | integer (uint64) | yes | Latest durable sequence, or zero before creation. |
| `mode` | [PolicyMode](#policymode) | yes | Permission ceiling selected when the session was created. |
| `model` | string | yes | Provider-qualified model. |
| `pinned` | boolean | no | Listed before unpinned sessions. Default: `false`. |
| `queue` | [Message](#message)[] | yes | Steering first, then follow-up messages. |
| `status` | [SessionStatus](#sessionstatus) | yes | Rebuilt from turn and session events. |
| `title` | string | yes | User-visible conversation title. |
| `workspace` | string \| null | no | Canonical absolute workspace path used as the Guard root and file-tool cwd. |

### SessionAccountSelection

Account preference result, applied at the next request boundary.

| Field | Type | Required | Description |
|---|---|---|---|
| `account` | [ModelAccountRef](#modelaccountref) \| null | no | Preferred authorized account, or cleared. |
| `effective` | string | yes | Always next_request; no daemon or sidecar restart required. |
| `explicit` | boolean | yes | Whether a person/request explicitly set this preference. |
| `provider` | string | yes | Provider namespace governed by the preference. |
| `session_id` | [SessionId](#sessionid) | yes | Durable session whose preference changed. |

Unknown fields are rejected.

### SessionId

Identifies a session (one conversation / task).

Type: string

### SessionList

Session listing response.

| Field | Type | Required | Description |
|---|---|---|---|
| `sessions` | [Session](#session)[] | yes | Sessions sorted by identifier for stable results. |

### SessionStatus

Durable session lifecycle.

| Value | Description |
|---|---|
| `"idle"` | No running turn. |
| `"running"` | A turn has started but not ended. |
| `"waiting_human"` | Idle with at least one pending human-intervention ticket. |
| `"stopped"` | Explicitly stopped. |

### SetCloudflareRequest

Configure the managed Cloudflare Tunnel; token values remain file/env-only.

| Field | Type | Required | Description |
|---|---|---|---|
| `enabled` | boolean | yes | Whether the transport should run. |
| `hostname` | string \| null | no | Named tunnel hostname. |
| `token_file` | string \| null | no | Local file containing the named tunnel's token, or null to use the environment. |
| `tunnel` | string | yes | `quick` or `named`. |

Unknown fields are rejected.

### SetExternalAgentRequest

`PUT /v1/external-agents/{id}`: where another agent keeps its skills.

| Field | Type | Required | Description |
|---|---|---|---|
| `skills_dir` | string | yes | Absolute path of the agent's skills folder, or `~/…`; must lie inside the user's home, config or data folders. |

Unknown fields are rejected.

### SetProviderKeyRequest

Secret input only: intentionally no Debug or Serialize implementation.

| Field | Type | Required | Description |
|---|---|---|---|
| `key` | string | yes | 8–8192 ASCII graphic bytes; never echoed or written to config. |
| `label` | string \| null | no | Optional replacement label; absent preserves the existing account label. |

Unknown fields are rejected.

### SetRelayRequest

Configure a self-hosted relay; its registration token remains file/env-only.

| Field | Type | Required | Description |
|---|---|---|---|
| `enabled` | boolean | yes | Whether the daemon should register with the relay. |
| `token_file` | string \| null | no | Local registration-token file, or null to use the environment. |
| `url` | string \| null | no | Relay origin. |

Unknown fields are rejected.

### Settings

Minimal daemon settings, with no credentials.

| Field | Type | Required | Description |
|---|---|---|---|
| `default_model` | string \| null | no | Provider-qualified default model. |
| `fallback_models` | string[] | yes | Configured fallback model selectors, in priority order. |
| `policy_mode` | [PolicyMode](#policymode) | yes | Default permission ceiling for new sessions. |

### SettingsDocument

The persisted configuration document (`config.toml`) as JSON.

| Field | Type | Required | Description |
|---|---|---|---|
| `config` | any | yes | Every configuration key with its stored or default value, except `file_only` sections. Shape: the openside configuration JSON Schema. |
| `file_only` | string[] | yes | Sections only editable in the file (`credentials`: opaque, may hold values). |
| `remote` | boolean | yes | Whether this caller lacks local authority, i.e. is limited to `remote_editable`. |
| `remote_editable` | string[] | yes | Dotted keys a paired person device may change without local authority; an entry ending in `.` covers every key under that prefix. Every other key (executables, paths, listeners, trust lists, sandbox, network, credentials, remote access) needs local authority: the Unix socket, or a browser opened from the launcher on this computer. Empty for an agent client that has no local authority. |
| `restart_required` | boolean | yes | Whether the stored document differs from what the running daemon loaded; changes apply on the next daemon start, except the live policy (`/v1/policy`). |
| `schema` | any | yes | JSON Schema of `config`, so a client can render and validate every key. |

### SkillDetail

Response of `GET /v1/skills/{name}` and the write endpoints.

| Field | Type | Required | Description |
|---|---|---|---|
| `allowed_tools` | string[] | yes | `allowed-tools` frontmatter; informational, never grants anything. |
| `body` | string | yes | Markdown instructions after the frontmatter. |
| `compatibility` | string \| null | no | `compatibility` frontmatter. |
| `description` | string | yes | What the skill does and when to use it. |
| `enabled` | boolean | yes | Whether the skill is offered to sessions. |
| `license` | string \| null | no | `license` frontmatter. |
| `name` | string | yes | Skill name. |
| `resources` | [SkillResource](#skillresource)[] | yes | Bundled files, at most 200. |
| `source` | [SkillSource](#skillsource) | yes | Where the skill lives. |

### SkillFile

A text file bundled into a new skill.

| Field | Type | Required | Description |
|---|---|---|---|
| `content` | string | yes | UTF-8 text of at most 24 KiB. |
| `path` | string | yes | Relative path inside the skill: no `..`, no hidden components, not `SKILL.md`. |

Unknown fields are rejected.

### SkillList

Response of `GET /v1/skills`.

| Field | Type | Required | Description |
|---|---|---|---|
| `problems` | [SkillProblem](#skillproblem)[] | yes | Rejected directories, so a settings screen can show them without failing. |
| `skills` | [SkillSummary](#skillsummary)[] | yes | Valid skills sorted by name (or by match quality when `q` is given). |

### SkillProblem

A skill directory that failed validation. The rest of the list is unaffected.

| Field | Type | Required | Description |
|---|---|---|---|
| `error` | string | yes | Why it was rejected; never contains file content. |
| `name` | string | yes | Directory name. |
| `source` | [SkillSource](#skillsource) | yes | Where the directory is. |

### SkillResource

A bundled file next to `SKILL.md`.

| Field | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Path relative to the skill directory. |
| `size` | integer (uint64) | yes | Size in bytes. |

### SkillSource

Where a skill is stored.

| Value | Description |
|---|---|
| `"user"` | The user's library; editable. |
| `"project"` | `<workspace>/.openside/skills`; read-only. |

### SkillSummary

Name and description of one skill, as shown in lists and the `/` mention picker.

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | string | yes | What the skill does and when to use it. |
| `enabled` | boolean | yes | Disabled skills are neither offered to the model nor loadable by the `skill` tool. |
| `name` | string | yes | Lowercase letters, digits and hyphens; also the directory name and the `/name` mention. |
| `source` | [SkillSource](#skillsource) | yes | Where the skill lives. |

### StartMemoryModelRequest

`POST /v1/memory/model/download`. Without fields it fetches the default model from the default source; naming another source needs the computer's local access.

| Field | Type | Required | Description |
|---|---|---|---|
| `endpoint` | string \| null | no | Model hub base URL; https, or http on loopback. |
| `repo` | string \| null | no | Repository, `owner/name`. |
| `revision` | string \| null | no | Branch, tag or commit. |

Unknown fields are rejected.

### SteerSubagentRequest

Steering text delivered at the child's next model-request boundary.

| Field | Type | Required | Description |
|---|---|---|---|
| `text` | string | yes | Steering text. |

Unknown fields are rejected.

### Subagent

One child agent started by the parent session's `delegate` tool.

| Field | Type | Required | Description |
|---|---|---|---|
| `background` | boolean | no | A background Task started by the front agent (`task_start`). |
| `ended_at` | string (date-time) \| null (date-time) | no | When the child reached a terminal state. |
| `id` | string | yes | Daemon-assigned subagent identifier, also used as `ticket.origin.subagent_id`. |
| `last_progress` | any | no | Latest `subagent.progress` payload. |
| `model` | string | yes | Model selector the child runs on. |
| `output` | string \| null | no | Final answer of a completed child, truncated for display. |
| `parent_tool_call_id` | string | yes | The parent's `delegate` tool call, which stays pending until the child ends. |
| `reason` | string \| null | no | Sanitized detail for failed or cancelled children. |
| `started_at` | string (date-time) | yes | When the daemon admitted the child. |
| `status` | [SubagentStatus](#subagentstatus) | yes | Current state. |
| `task` | string | yes | Delegated task, truncated for display. |
| `tools` | string[] | yes | Tools the child may call; a subset of the parent's tools. |

### SubagentList

Subagent listing response, in start order.

| Field | Type | Required | Description |
|---|---|---|---|
| `subagents` | [Subagent](#subagent)[] | yes | Every subagent the session started. |

### SubagentStatus

Subagent state as projected from the parent session's event log.

| Value | Description |
|---|---|
| `"running"` | The child is running a model turn or tool. |
| `"waiting_human"` | The child waits on a ticket listed under the parent session. |
| `"completed"` | The child finished; its output was returned to the parent's `delegate` call. |
| `"failed"` | The child failed or was interrupted; the parent received a failed result. |
| `"cancelled"` | The child was cancelled; the parent received a cancelled result. |

### SubmitMessageRequest

Message submission with an optional client-generated idempotency key.

| Field | Type | Required | Description |
|---|---|---|---|
| `attachments` | [AttachmentUpload](#attachmentupload)[] | no | Files for the model and the transcript, at most [`crate::MAX_ATTACHMENTS`] with [`crate::MAX_ATTACHMENT_BYTES`] decoded bytes in total (413 `attachments_too_large`). |
| `message_id` | [MessageId](#messageid) \| null | no | Reusing this id never changes or redelivers the existing message. |
| `mode` | [MessageMode](#messagemode) | yes | Requested delivery timing. |
| `text` | string | yes | User-authored text. |

Unknown fields are rejected.

### SubscriptionAccount

Administrator-visible external CLI account, never an OAuth value.

| Field | Type | Required | Description |
|---|---|---|---|
| `account_id` | string | yes | Durable source-local account id. |
| `available` | boolean | yes | Current local credential and account-policy readiness. |
| `created_at` | string (date-time) | yes | Registry creation time, RFC3339 UTC. |
| `enabled` | boolean | yes | Operator-controlled enablement. |
| `installed` | boolean | yes | CLI executable discovery, without running login. |
| `label` | string | yes | Person-assigned display label. |
| `logged_in` | boolean | yes | Detected usable external login, without public identity/token. |
| `login_command` | string | yes | Exact external home/environment command, never credentials. |
| `managed` | boolean | yes | True only for an openside-managed isolated CLI home. |
| `provider` | string | yes | Exact SDK provider namespace. |
| `reason` | string | yes | Sanitized status/recovery guidance. |
| `reserve_percent` | integer (uint8) | yes | Remaining measured usage protected from automatic selection. |
| `source` | string | yes | CLI source: claude-code or codex-cli. |

Unknown fields are rejected.

### SubscriptionList

Administrator-visible subscription source groups.

| Field | Type | Required | Description |
|---|---|---|---|
| `sources` | [SubscriptionSource](#subscriptionsource)[] | yes | Sources in stable CLI-name order. |

Unknown fields are rejected.

### SubscriptionSource

Installed CLI source with independently managed labelled accounts.

| Field | Type | Required | Description |
|---|---|---|---|
| `accounts` | [SubscriptionAccount](#subscriptionaccount)[] | yes | Includes the read-only default-home account unless hidden by the person. |
| `installed` | boolean | yes | Executable discovery, not an OAuth request. |
| `provider` | string | yes | Exact SDK namespace. |
| `source` | string | yes | CLI plugin name. |

Unknown fields are rejected.

### SuppressedProposal

A topic the person explicitly turned off; clearing it allows new proposals.

| Field | Type | Required | Description |
|---|---|---|---|
| `decided_at` | string (date-time) | yes | Time the person made the decision. |
| `key` | string | yes | Stable proposal identity used by the clear endpoint. |
| `label` | string \| null | no | Human-readable role captured at the decision; absent on legacy records. |
| `status` | string | yes | Persisted suppression decision, currently `never`. |

### SuppressedProposalList

Type: [SuppressedProposal](#suppressedproposal)[]

### SyncBackendKind

Configured sync backend.

| Value | Description |
|---|---|
| `"webdav"` | WebDAV share (`sync.webdav_url`). |
| `"dir"` | Shared directory (`sync.dir`). |

### SyncReport

`POST /v1/sync/run`: counts only, never record contents.

| Field | Type | Required | Description |
|---|---|---|---|
| `applied` | integer (uint32) | yes | Remote versions applied locally. |
| `conflicts` | integer (uint32) | yes | Local unsynced edits replaced by a newer remote version (conflict rules). |
| `deferred` | integer (uint32) | yes | Remote records waiting because their kind is unavailable here (locked vault, settings sync off). |
| `pulled` | integer (uint32) | yes | Remote records authenticated and decrypted. |
| `pushed` | integer (uint32) | yes | Records stored on the backend. |
| `rejected` | integer (uint32) | yes | Remote records that failed authentication or validation and were not applied. |
| `vault_available` | boolean | yes | Whether vault items took part in this run. |

### SyncSecretSource

How a sync secret reaches the daemon.

| Value | Description |
|---|---|
| `"command"` | Read by running the command configured in `[sync]`; nothing to type. |
| `"entered"` | Typed in the app and kept in daemon memory only: it must be entered again after the daemon restarts. |

### SyncSecrets

Secrets typed in the app for `setup`, `pair` and `unlock`. They stay in daemon memory, are never written to disk or returned, and need the computer's local access.

| Field | Type | Required | Description |
|---|---|---|---|
| `passphrase` | string \| null | no | Sync passphrase; used instead of `sync.passphrase_cmd` when given. |
| `webdav_password` | string \| null | no | WebDAV password; used instead of `sync.webdav_password_cmd` when given. |

Unknown fields are rejected.

### SyncSetupResponse

`POST /v1/sync/setup`: a new account. Give `pairing_code` (and the same passphrase) to the other device.

| Field | Type | Required | Description |
|---|---|---|---|
| `pairing_code` | string | yes | `osync1-…` code: account id plus a passphrase check; not a secret on its own. |
| `status` | [SyncStatus](#syncstatus) | yes | State after setup. |

### SyncStatus

`GET /v1/sync`.

| Field | Type | Required | Description |
|---|---|---|---|
| `account_id` | string \| null | no | Account id (not secret). |
| `backend` | [SyncBackendKind](#syncbackendkind) \| null | no | The configured backend. |
| `configured` | boolean | yes | Whether `[sync]` names a backend. |
| `device_id` | string \| null | no | This device's id. |
| `exhausted` | boolean | no | The sync counter reached its ceiling: runs fail with `sync_counter_exhausted` and local changes stay pending until a new account is set up. Default: `false`. |
| `last_run` | [SyncReport](#syncreport) \| null | no | Counts of the last run since the daemon started, conflicts included. Default: `null`. |
| `last_sync` | string (date-time) \| null (date-time) | no | Last completed sync. |
| `location` | string \| null | no | Shared directory or WebDAV URL of the configured backend; never credentials. Default: `null`. |
| `memory_tags` | string[] | no | Memories with any of these tags are synced. Default: `[]`. |
| `passphrase_source` | [SyncSecretSource](#syncsecretsource) | no | Where the sync passphrase comes from. Default: `"entered"`. |
| `password_source` | [SyncSecretSource](#syncsecretsource) \| null | no | Where the WebDAV password comes from; absent without a WebDAV username. Default: `null`. |
| `pending` | integer (uint64) | yes | Local changes not yet stored on the backend. |
| `records` | integer (uint64) | yes | Records known to this device, tombstones included. |
| `set_up` | boolean | yes | Whether this device joined an account (`setup` or `pair`). |
| `settings` | boolean | no | Whether the live policy document is synced. Default: `false`. |
| `unlocked` | boolean | yes | Whether the sync key is derived and held in memory. |
| `webdav_username` | string \| null | no | WebDAV username, when the share needs one. Default: `null`. |

### TableColumn

Canonical TableColumn wire contract; presentation never grants authority.

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Stable identity within its card namespace; never an authorization token. |
| `label` | string | yes | Plain-text accessible label; never markup. |

Unknown fields are rejected.

### TableLayout

Canonical TableLayout wire contract; presentation never grants authority.

| Value | Description |
|---|---|
| `"grid"` | Typed Grid presentation variant; source authorization remains unchanged. |
| `"scroll"` | Typed Scroll presentation variant; source authorization remains unchanged. |

### TailscaleConfiguration

Tailscale configuration presented to the web setup screen.

| Field | Type | Required | Description |
|---|---|---|---|
| `enabled` | boolean | yes | Whether Tailscale is configured. |
| `public_url` | string \| null | no | Persisted direct pairing origin. |

### TailscaleStatus

Tailscale Serve state for the openside handler, not unrelated services.

| Field | Type | Required | Description |
|---|---|---|---|
| `installed` | boolean | yes | Whether tailscale is available on PATH. |
| `last_error` | string \| null | no | Latest failure, without command output or secrets. |
| `state` | string | yes | `unavailable`, `stopped`, `running` or `failed`. |
| `url` | string \| null | no | HTTPS origin of the active openside handler. |

### TakeoverEndReason

Why a takeover stream ended.

| Value | Description |
|---|---|
| `"done"` | The ticket was completed with `done`. |
| `"rejected"` | The ticket was rejected (including session stop). |
| `"expired"` | The ticket reached its deadline. |
| `"idle"` | No input arrived within the idle limit. |
| `"revoked"` | The controlling device's token was revoked. |
| `"tab_unavailable"` | The tab or its browser is gone. |

### TakeoverInput

Client-to-daemon takeover input, forwarded to the ticket's tab only. Typed text and key values are never logged or persisted.

- `type=mouse`: Pointer press, release or move. Fields: `button`: [TakeoverMouseButton](#takeovermousebutton) \| null (optional), `click_count`: integer (uint32) \| null (uint32) (optional), `event`: [TakeoverMouseEvent](#takeovermouseevent), `modifiers`: integer (uint32) \| null (uint32) (optional), `x`: number (double), `y`: number (double).
- `type=wheel`: Scroll at a point. Fields: `delta_x`: number (double), `delta_y`: number (double), `x`: number (double), `y`: number (double).
- `type=key`: Raw key press or release (navigation and editing keys, shortcuts). Fields: `code`: string \| null (optional), `event`: [TakeoverKeyEvent](#takeoverkeyevent), `key`: string, `key_code`: integer (uint32) \| null (uint32) (optional), `modifiers`: integer (uint32) \| null (uint32) (optional), `text`: string \| null (optional).
- `type=text`: Insert text at the focused element (IME-composed or pasted text). Fields: `text`: string.

### TakeoverKeyEvent

Key event phase.

| Value | Description |
|---|---|
| `"down"` | Key pressed. |
| `"up"` | Key released. |

### TakeoverMouseButton

Mouse button of a pointer event.

| Value | Description |
|---|---|
| `"none"` | No button (moves). |
| `"left"` | Primary button. |
| `"middle"` | Auxiliary button. |
| `"right"` | Secondary button. |

### TakeoverMouseEvent

Pointer event phase.

| Value | Description |
|---|---|
| `"down"` | Button pressed. |
| `"up"` | Button released. |
| `"move"` | Pointer moved. |

### TakeoverServerMessage

Daemon-to-client takeover message.

- `type=frame`: One frame of the ticket's tab. Fields: `data`: string, `height`: number (double), `width`: number (double).
- `type=withheld`: A frame was withheld; the stream and input continue. Fields: `code`: string.
- `type=input_error`: The daemon refused one input message; the stream continues. Fields: `code`: string.
- `type=ended`: The stream ended; the daemon closes the socket after this message. Fields: `reason`: [TakeoverEndReason](#takeoverendreason).

### TeamCardCommand

Canonical TeamCardCommand wire contract; presentation never grants authority.

| Value | Description |
|---|---|
| `"undo_hire"` | Typed UndoHire presentation variant; source authorization remains unchanged. |
| `"retire"` | Typed Retire presentation variant; source authorization remains unchanged. |

### Ticket

Durable human-intervention ticket correlated with the same deferred tool call.

| Field | Type | Required | Description |
|---|---|---|---|
| `approval` | [TicketApproval](#ticketapproval) \| null | no | Set on `device_approval` tickets that wait for a phone sign-in approval (`wait_for_approval`). |
| `card` | [Card](#card) \| null | no | Daemon projection; absence in historical logs is hydrated without rewriting them. |
| `choice` | [TicketChoice](#ticketchoice) \| null | no | Set on `choice` tickets. |
| `created_at` | string (date-time) | yes | Creation time in RFC 3339 UTC. |
| `deferral` | [TicketDeferral](#ticketdeferral) | yes | Approval or external-result resumption semantics. |
| `expires_at` | string (date-time) | yes | Deadline in RFC 3339 UTC. |
| `form` | [TicketForm](#ticketform) \| null | no | Bound page-field metadata for fill-only requests; never contains entered values. |
| `hire` | [HireDraft](#hiredraft) \| null | no | Authoritative teammate proposal; card text alone cannot authorize a hire. |
| `id` | [TicketId](#ticketid) | yes | Stable ticket identifier. |
| `instructions` | string \| null | no | Optional redacted instructions for completing the action. |
| `kind` | [TicketKind](#ticketkind) | yes | Requested human action. |
| `login` | [TicketLogin](#ticketlogin) \| null | no | Set on `login` tickets. |
| `options` | [TicketOption](#ticketoption)[] | yes | Allowed non-secret responses; empty allows free-form information input. |
| `origin` | [TicketOrigin](#ticketorigin) | yes | Originating agent. |
| `otp` | [TicketOtp](#ticketotp) \| null | no | Set on `secret_input` tickets that ask for a one-time code (`request_otp`). |
| `otp_register` | [TicketOtpRegister](#ticketotpregister) \| null | no | Set on `confirm` tickets that ask to save a site's authenticator (`register_authenticator`). |
| `resolution` | any | no | Redacted resolution; never a secret-input value. |
| `resume_when` | any | no | Optional condition for detecting completion. |
| `session_id` | [SessionId](#sessionid) | yes | Parent session, including for subagent requests. |
| `status` | [TicketStatus](#ticketstatus) | yes | Pending or terminal state. |
| `summary` | string | yes | Redacted human-facing reason for asking. |
| `target` | [TicketTarget](#tickettarget) \| null | no | Browser destination, when applicable. |
| `tool` | [TicketTool](#tickettool) \| null | no | Redacted invocation, required for approval deferrals. |
| `tool_call_id` | string | yes | Stable model tool-call identifier preserved during resumption. |

### TicketAnswer

Canonical TicketAnswer wire contract; presentation never grants authority.

- `type=confirm`: Typed Confirm presentation variant; source authorization remains unchanged. Fields: `approved`: boolean.
- `type=information`: Typed Information presentation variant; source authorization remains unchanged. Fields: `field_id`: string.
- `type=selected_choice`: Typed SelectedChoice presentation variant; source authorization remains unchanged. Fields: `block_id`: string, `encoding`: [ChoiceAnswerEncoding](#choiceanswerencoding).
- `type=fields`: Typed Fields presentation variant; source authorization remains unchanged. Fields: `block_ids`: string[], `encoding`: [FieldAnswerEncoding](#fieldanswerencoding), `save_field_id`: string \| null (optional).
- `type=login_account`: Typed LoginAccount presentation variant; source authorization remains unchanged. Fields: `item_id`: string.
- `type=login_alternative`: Typed LoginAlternative presentation variant; source authorization remains unchanged. Fields: `alternative_id`: string.
- `type=otp_code`: Typed OtpCode presentation variant; source authorization remains unchanged. Fields: `field_id`: string.
- `type=otp_authenticator`: Typed OtpAuthenticator presentation variant; source authorization remains unchanged. Fields: `field_id`: string, `save_field_id`: string.
- `type=hire`: Decision and edited draft are governed by the team contract, not card text. Fields: `decision`: string, `edited_block_id`: string \| null (optional).

### TicketApproval

What a `device_approval` ticket opened by `wait_for_approval` shows: the site asks the person to approve the sign-in on their phone (a push notification in the site's own app). Nobody answers it in openside: the daemon watches the page and resolves the ticket with `{"status":"done","next":…}` once the page moves on; rejecting it cancels the wait.

| Field | Type | Required | Description |
|---|---|---|---|
| `icon` | string \| null | no | The site's icon, as on [`TicketLogin::icon`]. |
| `message` | string | yes | The page's heading and instruction for the approval (untrusted page text, capped). |
| `number` | string \| null | no | The number the page asks the person to tap or type on the phone, when it shows one. |
| `origin` | string | yes | Exact page origin. |
| `site_name` | string | yes | The site's name as the page gives it (untrusted page text), or its host. |

### TicketChoice

What a `choice` ticket (`request_choice`) shows: a decision the page asks for, such as which account, which verification method, or whether to trust this device.

| Field | Type | Required | Description |
|---|---|---|---|
| `icon` | string \| null | no | The site's icon, as on [`TicketLogin::icon`]. |
| `multiple` | boolean | yes | Always false: one option is picked. |
| `options` | [TicketChoiceOption](#ticketchoiceoption)[] | yes | Options in page order. |
| `origin` | string | yes | Exact page origin. |
| `question` | string | yes | The page's heading for the choice. |
| `site_name` | string | yes | The site's name as the page gives it (untrusted page text), or its host. |

### TicketChoiceOption

One option of a `choice` ticket.

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | string \| null | no | Secondary page text of the option, when it has some. |
| `id` | string | yes | Daemon-assigned id; answer with `{"option": id}`. |
| `label` | string | yes | The control's accessible name on the page (untrusted page text), never the model's. |

### TicketDeferral

Resumption semantics for the suspended tool call.

| Value | Description |
|---|---|
| `"approval"` | Approval permits the original tool call to execute again. |
| `"external"` | An external action supplies the original tool call's result. |

### TicketForm

Verified page form metadata; values are transient and never part of this record.

| Field | Type | Required | Description |
|---|---|---|---|
| `fields` | [InputField](#inputfield)[] | yes | Metadata in page order, with exact destination origins for embedded frames. Input metadata only; transient answers are never serialized here. |
| `origin` | string | yes | Exact verified top-document origin, not a registrable domain. Exact canonical URL origin, including nondefault port. |
| `payment` | boolean | yes | Requires recognized card autocomplete fields and secret handling for every value. Value-free metadata governed by the containing contract. |
| `site_name` | string | yes | Untrusted page display name; not an identity attestation. Value-free metadata governed by the containing contract. |
| `submit_label` | string | yes | Daemon-owned fill-only action label; never authorizes submission. Value-free metadata governed by the containing contract. |

Unknown fields are rejected.

### TicketId

Identifies a durable human-intervention ticket.

Type: string

### TicketKind

Human action required to resolve a suspended tool call.

| Value | Description |
|---|---|
| `"confirm"` | Authorize or reject a sensitive operation. |
| `"secret_input"` | Provide a secret directly to the daemon, never to the model or event log. |
| `"device_approval"` | Complete approval on another device. |
| `"info"` | Answer a non-secret question. |
| `"takeover"` | Perform an action directly in the browser. |
| `"login"` | Sign in to a site through a form the daemon built from the page's own login form (`request_login`); the values go straight into the page, never to the model. |
| `"choice"` | Pick one of the choices a page offers (`request_choice`); the daemon clicks the page's own control. |
| `"hire"` | Approve a proposed teammate using the attached draft. |

### TicketList

Ticket list, optionally filtered by status.

| Field | Type | Required | Description |
|---|---|---|---|
| `tickets` | [Ticket](#ticket)[] | yes | Matching durable tickets. |

### TicketLogin

What a `login` ticket (`request_login`) shows: the page's login form, rebuilt by the daemon. Answer with `{"values": {<field id>: string \| bool}, "save": bool}`, `{"account": <item_id>}` or `{"alternative": <id>}`.

| Field | Type | Required | Description |
|---|---|---|---|
| `accounts` | [TicketLoginAccount](#ticketloginaccount)[] \| null | no | Logins saved for this origin, when there are several to pick from. |
| `alternatives` | [TicketLoginAlternative](#ticketloginalternative)[] | yes | Other ways to sign in the page offers. |
| `can_save` | boolean | yes | Whether `save: true` may keep the identifier and password in the vault for this origin (it also needs the `approve` scope and a person device). |
| `error_text` | string \| null | no | The page's own error message next to the form (e.g. a wrong password on the previous attempt; untrusted page text, redacted and capped), when it shows one. |
| `fields` | [TicketLoginField](#ticketloginfield)[] | yes | Fields in page order. |
| `icon` | string \| null | no | The site's icon as a `data:image/...;base64,` URL (at most 8 KiB, fetched by the daemon from the page's own origin), when it has one. |
| `origin` | string | yes | Exact page origin the values go to. |
| `site_name` | string | yes | The site's name as the page gives it (untrusted page text), or its host. |
| `submit_label` | string | yes | The page's own text for its sign-in button. |

### TicketLoginAccount

A login saved in the vault for the ticket's origin, offered instead of typing.

| Field | Type | Required | Description |
|---|---|---|---|
| `item_id` | string | yes | Vault item id; answer with `{"account": item_id}`. |
| `username_hint` | string | yes | Masked account name, e.g. `j***@naver.com`. |

### TicketLoginAlternative

An alternative sign-in control on the page; answering with its id clicks it.

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Daemon-assigned id. |
| `kind` | [LoginAlternativeKind](#loginalternativekind) | yes | What kind of sign-in it starts. |
| `label` | string | yes | The control's accessible name on the page (untrusted page text). |
| `provider` | [LoginProvider](#loginprovider) \| null | no | The provider, for `provider` controls the daemon recognized. Default: `null`. |

### TicketLoginField

One field of the page's login form, in page order.

| Field | Type | Required | Description |
|---|---|---|---|
| `autocomplete` | string \| null | no | Autocomplete token for the card's input (`username`, `email`, `tel`, `current-password`, `one-time-code`), so the person's own password manager can fill it. |
| `id` | string | yes | Daemon-assigned id the answer's `values` are keyed by. |
| `kind` | [LoginFieldKind](#loginfieldkind) | yes | What the field holds. |
| `label` | string | yes | The page's own label for the field (untrusted page text, trimmed and capped). |
| `masked` | boolean | yes | Whether the input hides what is typed (passwords). |
| `placeholder` | string \| null | no | The page's placeholder, when it has one. |
| `required` | boolean | yes | Whether the page requires a value. |
| `suggestions` | string[] | no | Imported non-secret address values offered only when the person chooses one. |

### TicketOption

One selectable non-secret response.

| Field | Type | Required | Description |
|---|---|---|---|
| `label` | string | yes | Human-facing label. |
| `value` | string | yes | Stable machine-facing answer value. |

### TicketOrigin

Identity of the agent that requested human intervention.

| Field | Type | Required | Description |
|---|---|---|---|
| `device_id` | string \| null | no | Device behind a direct API tool call; `local` identifies the Unix socket. |
| `driver_ids` | string[] | no | Agent-client identities that drove this session before the ticket opened. |
| `subagent_id` | string \| null | no | Absent for the parent agent, otherwise the originating subagent. |

### TicketOtp

One-time-code details of a `secret_input` ticket opened by `request_otp`.

| Field | Type | Required | Description |
|---|---|---|---|
| `can_register` | boolean | yes | Besides a code (a string `value`), the person may resolve with `{"totp": "<otpauth:// URI or base32 setup key>"}`: the daemon stores the authenticator in the vault for the ticket's origin and fills the code it generates, now and on later `request_otp` calls for that origin. Registration requires `approve` and a person device. |
| `channel` | [OtpChannel](#otpchannel) | yes | Where the code comes from. |

### TicketOtpRegister

What a `confirm` ticket opened by `register_authenticator` asks the person to approve: saving the authenticator a site's two-factor setup page shows into the vault, bound to exactly `origin`. The setup key itself is held by the daemon only and is never part of the ticket. Approving (`true`) saves it and fills the page's confirmation code; declining (`false` or rejecting the ticket) discards it.

| Field | Type | Required | Description |
|---|---|---|---|
| `account` | string \| null | no | Account named by the setup page's `otpauth://` URI, when it has one. |
| `issuer` | string \| null | no | Issuer named by the setup page's `otpauth://` URI, when it has one. |
| `origin` | string | yes | Exact page origin the authenticator will be bound to. |

### TicketRef

Link into the existing human-step ticket UI.

| Field | Type | Required | Description |
|---|---|---|---|
| `session_id` | [SessionId](#sessionid) | yes | Session containing the ticket. |
| `ticket_id` | [TicketId](#ticketid) | yes | Stable ticket ID. |

### TicketStatus

Durable ticket lifecycle; only pending tickets accept a terminal transition.

| Value | Description |
|---|---|
| `"pending"` | Awaiting human intervention. |
| `"resolved"` | The requested intervention completed. |
| `"rejected"` | The user or daemon rejected the intervention. |
| `"expired"` | The deadline passed. |
| `"needs_desktop"` | The intervention requires local desktop access. |

### TicketTarget

Browser destination for human intervention and origin validation.

| Field | Type | Required | Description |
|---|---|---|---|
| `field_ref` | string \| null | no | Snapshot field reference receiving daemon-only input. |
| `origin` | string \| null | no | Expected page origin, verified before secret input. |
| `tab_id` | string \| null | no | Browser target identifier. |

### TicketTool

Redacted invocation displayed for an approval deferral.

| Field | Type | Required | Description |
|---|---|---|---|
| `args` | any | yes | Invocation arguments with secrets already removed. |
| `name` | string | yes | Registered tool name. |

### ToolCallRequest

Run one daemon tool in a session through the same host pipeline as an agent call.

| Field | Type | Required | Description |
|---|---|---|---|
| `approval_ticket` | [TicketId](#ticketid) \| null | no | Approved `confirm` ticket from an earlier `deferred` response with deferral `approval`; the identical `args` run again under that approval. |
| `args` | any | no | Tool arguments matching the descriptor's `input_schema`. Default: `{}`. |

Unknown fields are rejected.

### ToolCallResponse

Result of a [`ToolCallRequest`], recorded as a tool call in the session's events.

| Field | Type | Required | Description |
|---|---|---|---|
| `artifacts` | string[] | no | Delivered artifact paths, readable through `GET /v1/sessions/{id}/artifacts/{path}`. Observation screenshots have no artifact; browser_screenshot needs output=true to save one. |
| `deferral` | [TicketDeferral](#ticketdeferral) \| null | no | How the pending ticket resumes the call when `status` is `deferred`. |
| `hint` | string \| null | no | How to proceed after a denial. |
| `reason` | string \| null | no | Denial reason or failure message. |
| `result` | any | no | Redacted tool result when `status` is `done`. |
| `status` | [ToolCallStatus](#toolcallstatus) | yes | Outcome category. |
| `ticket_id` | [TicketId](#ticketid) \| null | no | Pending ticket when `status` is `deferred`. |
| `tool_call_id` | string | yes | Tool call identifier recorded in the session's `tool.started`/`tool.ended` events. |

### ToolCallStatus

Outcome category of a [`ToolCallRequest`].

| Value | Description |
|---|---|
| `"done"` | The tool ran; `result` holds its redacted result. |
| `"deferred"` | A ticket in `ticket_id` is pending. With deferral `approval`, call again with `approval_ticket` once it is approved; with `external`, the ticket's resolution is the tool's result. |
| `"denied"` | Policy or validation refused the call; see `reason` and `hint`. |
| `"failed"` | The daemon could not run the tool. |

### ToolCatalog

`GET /v1/tools` response: every directly callable tool, in name order. A session may still refuse a listed tool (its mode, an incognito session, the configured agent profile).

| Field | Type | Required | Description |
|---|---|---|---|
| `tools` | [ToolDescriptor](#tooldescriptor)[] | yes | Callable tools. |

### ToolDescriptor

One daemon tool an API caller may run directly (`POST /v1/sessions/{id}/tools/{name}`).

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | string | yes | The description the agent's model sees. |
| `human` | boolean | yes | Whether the call opens a person-facing ticket and its result arrives on resolution. |
| `input_schema` | any | yes | JSON Schema of `ToolCallRequest.args`. |
| `name` | string | yes | Tool name, identical to the name the in-app agent calls. |
| `scope` | [CapabilityScope](#capabilityscope) | yes | Capability a token needs to call it. |

### TransferRequest

Explicit cross-store ownership transaction, persisted as a recoverable saga.

| Field | Type | Required | Description |
|---|---|---|---|
| `memory_ids` | string[] | no | Only the selected notes change ownership. Default: `[]`. |
| `responsibilities` | string[] | no | Existing responsibility lines to remove from the source and add to the destination. Default: `[]`. |
| `routine_ids` | string[] | no | Only the selected schedules change ownership. Default: `[]`. |
| `skills` | string[] | no | Allowlist grants; installed skill files never move between agents. Default: `[]`. |
| `to` | string | yes | Exactly one active destination agent. |

Unknown fields are rejected.

### UpdateAgentRequest

Partial profile edit; nullable overrides distinguish omission from explicit inheritance.

| Field | Type | Required | Description |
|---|---|---|---|
| `account_pool` | [ModelAccountRef](#modelaccountref)[] \| null | no | Explicit null inherits, a nonempty list restricts, omission preserves membership. |
| `channel` | [AgentChannelBinding](#agentchannelbinding) \| null | no | Explicit null clears the binding, never historical session tags. |
| `color` | string \| null | no | Only the shipped avatar palette is accepted. |
| `hidden` | boolean \| null | no | Sidebar visibility only, without pausing scheduled work. |
| `initials` | string \| null | no | Explicit override; omission preserves existing initials when renaming. |
| `mode` | [PolicyMode](#policymode) \| null | no | Widening additionally needs Admin/person authority and cannot exceed policy. |
| `model` | string \| null | no | Explicit null returns to the default model; omission leaves the override unchanged. |
| `name` | string \| null | no | Renaming preserves stable identity and the home conversation. |
| `persona` | [AgentPersona](#agentpersona) \| null | no | Replaces the complete persona, not individual responsibility lines. |
| `pinned` | boolean \| null | no | Changes roster priority, not conversation content or authority. |
| `skills` | string[] \| null | no | Explicit null returns to enabled defaults; changes require Admin and a person. |

Unknown fields are rejected.

### UpdateChannelAccountRequest

Partial account change; credential replacement rotates only this identity.

| Field | Type | Required | Description |
|---|---|---|---|
| `agent_id` | string \| null | no | Empty string removes the assignment; omitted leaves it unchanged. Default: `null`. |
| `credentials` | [ChannelCredentials](#channelcredentials) \| null | no | Replace this identity's credentials, stopping old tasks before persistence. Default: `null`. |
| `enabled` | boolean \| null | no | Omitted keeps existing desired enablement. Default: `null`. |
| `label` | string \| null | no | Omitted keeps the existing label. Default: `null`. |
| `options` | [ChannelAccountOptions](#channelaccountoptions) \| null | no | Replace the complete non-secret options object when present. Default: `null`. |

Unknown fields are rejected.

### UpdateConnectionRequest

Admin metadata patch; omitted fields remain unchanged.

| Field | Type | Required | Description |
|---|---|---|---|
| `grants` | [ConnectionGrant](#connectiongrant)[] \| null | no | Upsert these grants without replacing unrelated agents. |
| `home_url` | string \| null | no | New HTTP(S) home URL, restricted to the connection's site. |

Unknown fields are rejected.

### UpdateMemoryRequest

`PATCH /v1/memory/{id}` body; absent fields are unchanged.

| Field | Type | Required | Description |
|---|---|---|---|
| `content` | string \| null | no | Replacement text. |
| `retention` | [Retention](#retention) \| null | no | Replacement retention; the purge deadline restarts from this write. |
| `tags` | string[] \| null | no | Replacement labels. |

Unknown fields are rejected.

### UpdateModelsRequest

Persist available catalog ids for new sessions and the next idle turn of Team homes.

| Field | Type | Required | Description |
|---|---|---|---|
| `default_model` | string | yes | Available provider-qualified catalog id; must not be a fallback duplicate. |
| `fallback_models` | string[] | yes | At most 16 distinct available catalog ids in fallback order. |

Unknown fields are rejected.

### UpdateNetworkPolicyRequest

`PUT /v1/policy/network`.

| Field | Type | Required | Description |
|---|---|---|---|
| `allow` | string[] | yes | Replacement `network.allow` list: `host`, `*.domain`, `host:port` or `scheme://host[:port]`. |

Unknown fields are rejected.

### UpdateOnboardingRequest

`PUT /v1/onboarding`: record how the guide ended. Only `completed` and `skipped` are accepted; reopening the guide from Settings does not reset the flag.

| Field | Type | Required | Description |
|---|---|---|---|
| `state` | [OnboardingState](#onboardingstate) | yes | `completed` or `skipped`. |

Unknown fields are rejected.

### UpdateProviderKeyRequest

Account metadata change, preserving the encrypted key.

| Field | Type | Required | Description |
|---|---|---|---|
| `enabled` | boolean \| null | no | New operator enablement; missing keeps the current setting. |
| `label` | string \| null | no | New person-assigned label; missing keeps the current label. |
| `reserve_percent` | integer (uint8) \| null (uint8) | no | Protected remaining percentage0–99; missing keeps the current threshold. |

Unknown fields are rejected.

### UpdateProviderPoolRequest

Replace the provider's explicit trust boundary; new sources are never implicit.

| Field | Type | Required | Description |
|---|---|---|---|
| `accounts` | [ModelAccountRef](#modelaccountref)[] | yes | Known compatible opaque accounts, ordered without duplicates. |

Unknown fields are rejected.

### UpdateRoutineRequest

Partial routine update; omitted fields are unchanged.

| Field | Type | Required | Description |
|---|---|---|---|
| `action` | [RoutineAction](#routineaction) \| null | no | New action. |
| `catch_up` | boolean \| null | no | New catch-up policy. |
| `enabled` | boolean \| null | no | Pause (`false`) or resume (`true`); resuming clears `paused_reason`. |
| `name` | string \| null | no | New name. |
| `schedule` | [RoutineSchedule](#routineschedule) \| null | no | New schedule; the next run is recomputed from now. |

Unknown fields are rejected.

### UpdateSessionAccountRequest

Select/clear a preferred sticky account within the already authorized effective pool.

| Field | Type | Required | Description |
|---|---|---|---|
| `account` | [ModelAccountRef](#modelaccountref) \| null | no | Null clears preference; never adds or enables an account. |

Unknown fields are rejected.

### UpdateSessionRequest

Partial session metadata update; omitted fields keep their current value. Applied as one durable `session.updated` event, so list state replays from the log.

| Field | Type | Required | Description |
|---|---|---|---|
| `archived` | boolean \| null | no | Archived sessions are hidden from the default session list. |
| `pinned` | boolean \| null | no | Pinned sessions are listed before unpinned ones. |
| `title` | string \| null | no | New non-blank title, at most [`MAX_SESSION_TITLE_CHARS`] characters after trimming. |

Unknown fields are rejected.

### UpdateSettingsRequest

Replace the configuration document; validated like `config.toml` before writing.

| Field | Type | Required | Description |
|---|---|---|---|
| `config` | any | yes | Complete document; `file_only` sections are kept from the file. |

Unknown fields are rejected.

### UpdateSkillRequest

`PATCH /v1/skills/{name}` body; absent fields are unchanged.

| Field | Type | Required | Description |
|---|---|---|---|
| `body` | string \| null | no | Replacement Markdown instructions. |
| `description` | string \| null | no | Replacement description. |
| `enabled` | boolean \| null | no | Switch the skill on or off. |

Unknown fields are rejected.

### UpdateSubscriptionAccountRequest

Mutate registry metadata without modifying a default user's credential home.

| Field | Type | Required | Description |
|---|---|---|---|
| `enabled` | boolean \| null | no | New enablement; missing keeps the current value. |
| `label` | string \| null | no | New label; missing keeps the current value. |
| `reserve_percent` | integer (uint8) \| null (uint8) | no | Protected remaining percentage0–99; missing keeps the current threshold. |

Unknown fields are rejected.

### UpdateSyncConfigRequest

`PUT /v1/sync/config`: choose where synced data is stored. Exactly one of `dir` and `webdav_url`, or neither to turn sync off. The backend of a device that already joined an account cannot change; `settings` and `memory_tags` always can.

| Field | Type | Required | Description |
|---|---|---|---|
| `dir` | string \| null | no | Shared directory synced by other means. |
| `memory_tags` | string[] | no | Memories with any of these tags are synced. Default: `[]`. |
| `settings` | boolean | no | Sync the live policy document. Default: `false`. |
| `webdav_url` | string \| null | no | WebDAV collection URL; https, or http on loopback. |
| `webdav_username` | string \| null | no | WebDAV username; its password is typed on unlock or read from `webdav_password_cmd`. |

Unknown fields are rejected.

### UpdateVaultItemRequest

`PATCH /v1/vault/items/{id}`.

| Field | Type | Required | Description |
|---|---|---|---|
| `policy` | [VaultPolicyOverride](#vaultpolicyoverride) | yes | New override. |

Unknown fields are rejected.

### ValidatePolicyRequest

`POST /v1/policy/validate`: check an edit without saving it.

| Field | Type | Required | Description |
|---|---|---|---|
| `network_allow` | string[] \| null | no | Outbound network allow-list to check. |
| `policy` | [PolicySettings](#policysettings) \| null | no | Live policy document to check. |

Unknown fields are rejected.

### VapidPublicKey

The daemon's VAPID public key, the PWA's `applicationServerKey`.

| Field | Type | Required | Description |
|---|---|---|---|
| `public_key` | string | yes | Uncompressed P-256 public key, base64url without padding. |

### VaultAuditList

`GET /v1/vault/audit`, oldest first.

| Field | Type | Required | Description |
|---|---|---|---|
| `records` | [VaultAuditRecord](#vaultauditrecord)[] | yes | Matching records. |

### VaultAuditRecord

One vault audit record. Never contains a secret value.

| Field | Type | Required | Description |
|---|---|---|---|
| `at` | string (date-time) | yes | When the decision was recorded (second precision). |
| `detail` | string \| null | no | Denial reason (e.g. `origin_mismatch`, `policy_never`) or new policy. |
| `item` | string | yes | Item id, `unknown-site`, or `*` for listings and lifecycle steps. |
| `origin` | string | yes | Normalized page origin, `invalid-origin`, or `-` when not page-bound. |
| `result` | string | yes | `allowed`, `denied`, `filled`, `fill-failed`, `completed`, `failed`, `updated`, `registered`, `added`, `deleted`, `unlocked` or `locked`. |
| `session_id` | string \| null | no | Agent session (task) id for tool calls. |
| `tool_call_id` | string \| null | no | Model tool-call id for tool calls. |
| `what` | string | yes | `fill`, `otp`, `otp_register`, `passkey_register`, `passkey_login`, `login_save`, `get`, `policy_update`, `item_add`, `item_delete`, or the lifecycle steps `setup`, `unlock` and `lock`. |
| `who` | string | yes | Caller label: `opensided` for tools and the daemon's own startup unlock, `device:<id>` for a paired device's API call, `local` for the Unix socket. |

### VaultBackend

How the vault file is unlocked.

| Value | Description |
|---|---|
| `"passphrase"` | A passphrase a person chooses (or `vault.passphrase_cmd`); never stored. |
| `"secret_service"` | A random key in the Linux Secret Service keyring (GNOME Keyring, KWallet). |
| `"native"` | A random key in the macOS Keychain or Windows Credential Manager. |

### VaultItem

Value-free metadata of one vault item. Usernames, passwords, TOTP seeds and passkey private keys are never part of the API.

| Field | Type | Required | Description |
|---|---|---|---|
| `effective_policy` | [VaultPolicy](#vaultpolicy) | yes | Policy actually enforced. |
| `has_totp` | boolean | yes | Whether a TOTP seed is stored. |
| `id` | string | yes | Item id used by `vault_*` tools (`site`). |
| `origins` | string[] | yes | Allowed origins, normalized `scheme://[*.]host:port`. |
| `passkey_rp_ids` | string[] | yes | RP IDs of stored passkeys. |
| `policy` | [VaultPolicy](#vaultpolicy) \| null | no | Per-item override; null inherits `default_policy`. |

### VaultItemList

`GET /v1/vault/items`.

| Field | Type | Required | Description |
|---|---|---|---|
| `available` | boolean | yes | Whether the daemon has an unlocked vault; false lists no items. |
| `default_policy` | [VaultPolicy](#vaultpolicy) | yes | Policy for items without an override (`vault.default_policy`). |
| `items` | [VaultItem](#vaultitem)[] | yes | Items, in vault order. |

### VaultPolicy

Vault access policy: when an item's secrets may be released to a fill/OTP/passkey tool.

| Value | Description |
|---|---|
| `"always"` | Whenever the vault file is decrypted. |
| `"while_unlocked"` | Only while the platform lock (keyring/passphrase session) is unlocked. |
| `"never"` | Never. |

### VaultPolicyOverride

Per-item policy change; `default` removes the override so the vault default applies.

| Value | Description |
|---|---|
| `"always"` | Override to `always`. |
| `"while_unlocked"` | Override to `while_unlocked`. |
| `"never"` | Override to `never`. |
| `"default"` | Inherit the vault default. |

### VaultState

Vault lifecycle state.

| Value | Description |
|---|---|
| `"uninitialized"` | No vault file yet; `POST /v1/vault/setup` (or adding the first item with a keyring backend) creates it. |
| `"locked"` | The file exists but is not decrypted; `POST /v1/vault/unlock`. |
| `"unlocked"` | Decrypted; vault tools and item routes work. |
| `"unavailable"` | The configured method cannot open this vault here; see `reason` and `openside doctor`. |

### VaultStatus

`GET /v1/vault`: lifecycle state without any value.

| Field | Type | Required | Description |
|---|---|---|---|
| `backend` | [VaultBackend](#vaultbackend) \| null | no | Unlock method in effect; null when `unavailable` has no usable method. |
| `default_policy` | [VaultPolicy](#vaultpolicy) | yes | Policy for items without an override (`vault.default_policy`). |
| `reason` | string \| null | no | Why the vault is locked or unavailable: `keyring_unreachable`, `keyring_locked`, `keyring_key_mismatch`, `passphrase_cmd_failed` (from the daemon-start attempt), `unsupported_platform`, `format_mismatch`, `unreadable` or `probe_timeout`. |
| `state` | [VaultState](#vaultstate) | yes | Current state. |

### VaultUnlockRequest

`POST /v1/vault/setup` and `POST /v1/vault/unlock`. The passphrase is required for a passphrase vault and must be absent for a keyring vault. It only travels to the daemon, which never returns, logs or stores it.

| Field | Type | Required | Description |
|---|---|---|---|
| `passphrase` | string \| null | no | Vault passphrase (setup: at least 8 characters). Default: `null`. |

Unknown fields are rejected.

### Visual

A visual part of an assistant message with its source and a sandboxed preview.

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Stable id: `<assistant.final seq>-<block index>`. |
| `kind` | [VisualKind](#visualkind) | yes | Visual kind. |
| `path` | string | yes | Artifact path of the materialized source. |
| `preview_url` | string | yes | Time-limited, unauthenticated URL path for sandboxed rendering. |
| `seq` | integer (uint64) | yes | Sequence of the `assistant.final` event containing the block. |
| `source` | string | yes | Block source text, exactly as written by the assistant. |

### VisualKind

Renderable visual block kinds. Assistants mark them with a fenced code block whose info string is the kind followed by `visual`, e.g. "```svg visual".

| Value | Description |
|---|---|
| `"html"` | Self-contained HTML document; inline scripts run only inside the sandbox. |
| `"svg"` | SVG image. |

### VisualList

`GET /v1/sessions/{id}/visuals` response in message order.

| Field | Type | Required | Description |
|---|---|---|---|
| `visuals` | [Visual](#visual)[] | yes | Visual parts, oldest first. |
