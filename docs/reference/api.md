# API reference

Generated from `docs/specs/openapi.json` by `tools/gen-api-docs.py`; do not edit by hand (`tools/gen-docs.sh` regenerates it). openside API v1.

## Authentication

- `capabilityToken` (http, bearer): Device token from POST /v1/devices/pair or `openside token create --scope <scope>`. Each operation's x-required-capability names the scope it needs.

## Endpoints

| Group | Operations |
|---|---|
| accounts | 1 |
| browser | 1 |
| devices | 3 |
| health | 1 |
| memory | 7 |
| otp | 1 |
| pairing | 2 |
| pinned-tabs | 3 |
| policy | 3 |
| push | 5 |
| routines | 8 |
| sessions | 21 |
| settings | 3 |
| skills | 5 |
| sync | 4 |
| tickets | 6 |
| vault | 9 |

## accounts

### GET /v1/accounts

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [AccountsResponse](#accountsresponse) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## browser

### GET /v1/browser/tabs

Required capability: `read`

Metadata-only ledger of explicitly agent-owned targets with a known non-incognito owner. Omits user/external targets and removes URL credentials, query and fragment. Empty before browser launch; never launches or reconnects the browser.

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [BrowserTabList](#browsertablist) |
| 503 | The previously started browser controller is disconnected | `application/json` [ApiError](#apierror) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## devices

### GET /v1/devices

Required capability: `admin`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [DeviceList](#devicelist) |
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

## health

### GET /v1/health

Required capability: `read`

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Health](#health) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

## memory

### GET /v1/memory

Required capability: `read`

Hybrid search: FTS5 bm25 and local-embedding cosine ranks fused by reciprocal rank (k=60); lexical-only without a local model. An empty q lists recent memories.

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
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

Issue a one-time pairing code valid for 180 seconds. Only callable over the Unix socket (TCP callers get 403 local_socket_required). Issuing, exchanging and revoking are written to audit.jsonl.

Request body (required): `application/json` [CreatePairingRequest](#createpairingrequest)

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [PairingOffer](#pairingoffer) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/pairing/exchange

Required capability: `admin`

No bearer token: the one-time code is the credential. 404 pairing_not_found for an unknown or already used code, 410 pairing_expired after 180 seconds. A code is consumed by its first exchange attempt.

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

Replace the live policy. Invalid globs or a sensitive allow rule return 400 invalid_policy and change nothing. Applies to the next tool call of every session and is persisted in <data_dir>/policy.json, which overrides [policy] in config.toml.

Request body (required): `application/json` [PolicySettings](#policysettings)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [PolicySettings](#policysettings) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/policy/test

Required capability: `read`

Evaluate one capability/target against the live policy without running anything.

Request body (required): `application/json` [PolicyTestRequest](#policytestrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [PolicyDecision](#policydecision) |
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
| `q` | query | string | no | Case-insensitive substring of the title or first message. |
| `archived` | query | boolean | no | List archived (true) or unarchived (false, default) sessions; pinned sessions come first. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SessionList](#sessionlist) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### POST /v1/sessions

Required capability: `steer`

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

Update title/pinned/archived as a durable session.updated event.

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

### GET /v1/sessions/{id}/events

Required capability: `read`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [SessionId](#sessionid) | yes |  |
| `Last-Event-ID` | header | string | no | Last seen per-session sequence; only later events are returned. |
| `subagents` | query | boolean | no | Set false to omit subagent.* events from the stream. |

| Status | Description | Body |
|---|---|---|
| 200 | Success | `text/event-stream` string; each event is [Event](#event) |
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

### GET /v1/settings/config

Required capability: `admin`

The whole config.toml as JSON, with defaults filled in. PUT validates exactly like loading the file (400 invalid_config with the offending key) and writes it atomically with mode 0600; the credentials section is file-only. Changes apply when the daemon restarts, except the live policy (/v1/policy).

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SettingsDocument](#settingsdocument) |
| default | Sanitized error | `application/json` [ApiError](#apierror) |

### PUT /v1/settings/config

Required capability: `admin`

The whole config.toml as JSON, with defaults filled in. PUT validates exactly like loading the file (400 invalid_config with the offending key) and writes it atomically with mode 0600; the credentials section is file-only. Changes apply when the daemon restarts, except the live policy (/v1/policy).

Request body (required): `application/json` [UpdateSettingsRequest](#updatesettingsrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [SettingsDocument](#settingsdocument) |
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

### POST /v1/sync/pair

Required capability: `admin`

Join the account of a pairing code with the same passphrase. 400 invalid_pairing_code, sync_wrong_passphrase; 409 as setup; 502 sync_backend_unavailable, sync_unauthorized.

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

Create a sync account for this device: derive the key from sync.passphrase_cmd (kept in memory), create the account collection on the WebDAV share or directory and return the pairing code for the second device. 409 sync_not_configured, sync_already_set_up, sync_passphrase_unavailable, sync_password_unavailable; 502 sync_backend_unavailable, sync_unauthorized.

| Status | Description | Body |
|---|---|---|
| 201 | Success | `application/json` [SyncSetupResponse](#syncsetupresponse) |
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

The daemon checks the ticket kind: confirm/info require approve; secret_input/device_approval/takeover require human_step. A secret_input answer that registers an authenticator ({"value":{"totp":…}} on /resolve) also requires approve.

`x-capability-by-ticket-kind`: `{"confirm": "approve", "device_approval": "human_step", "info": "approve", "secret_input": "human_step", "takeover": "human_step"}`

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

The daemon checks the ticket kind: confirm/info require approve; secret_input/device_approval/takeover require human_step. A secret_input answer that registers an authenticator ({"value":{"totp":…}} on /resolve) also requires approve.

`x-capability-by-ticket-kind`: `{"confirm": "approve", "device_approval": "human_step", "info": "approve", "secret_input": "human_step", "takeover": "human_step"}`

| Parameter | In | Type | Required | Description |
|---|---|---|---|---|
| `id` | path | [TicketId](#ticketid) | yes |  |

Request body (required): `application/json` [ResolveTicketRequest](#resolveticketrequest)

| Status | Description | Body |
|---|---|---|
| 200 | Success | `application/json` [Ticket](#ticket) |
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

### AccountSource

One credential plugin's availability.

| Field | Type | Required | Description |
|---|---|---|---|
| `accounts` | [AccountStatus](#accountstatus)[] | yes | Registered broker accounts or configured local provider accounts. |
| `available` | boolean | yes | At least one account is usable (command results remain unverified). |
| `kind` | string | yes | Plugin kind: omp-broker, env, command, codex-cli or builtin. |
| `name` | string | yes | Stable plugin name. |
| `reason` | string | yes | Credential-value-free diagnostic. |

Unknown fields are rejected.

### AccountStatus

Public account metadata; ids are opaque and labels never contain raw email addresses.

| Field | Type | Required | Description |
|---|---|---|---|
| `blocked_until_ms` | number (double) \| null (double) | no | Latest active block expiry, as Unix milliseconds, or null. |
| `id` | string | yes | Opaque account reference, not an upstream account id. |
| `label` | string | yes | Safe display label. |
| `models` | string[] | yes | Configured model identifiers for this provider, not an invented broker catalog. |
| `provider` | string | yes | Provider identifier. |
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
| `size` | integer (uint64) | yes | Byte size. |
| `source` | [ArtifactSource](#artifactsource) | yes | Origin of the file. |
| `tool` | string \| null | no | Tool name that last wrote it, for tool artifacts. |
| `tool_call_id` | string \| null | no | Tool call that last wrote it, for tool artifacts. |

### ArtifactList

`GET /v1/sessions/{id}/artifacts` response, oldest first.

| Field | Type | Required | Description |
|---|---|---|---|
| `artifacts` | [Artifact](#artifact)[] | yes | Artifacts in creation order. |

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
| `"approve"` | Resolve or reject confirmation and information tickets. |
| `"human_step"` | Resolve, complete, or reject secret-input, device-approval, and takeover tickets. |
| `"admin"` | Pair and revoke devices. |
| `"browse"` | Drive session browser routes (`browser_repl`) without steering the agent; for sandboxed tools, MCP servers and extensions. |
| `"vault_use"` | Use saved vault credentials through vault routes; never reveals values. |
| `"otp_ingest"` | Deliver one-time codes to the OTP ingest route. |

### ContentKind

Broad media class of one content part; the runtime maps it to model input types.

| Value | Description |
|---|---|
| `"text"` | UTF-8 text document. |
| `"image"` | Raster image (PNG, JPEG, GIF, WebP). |
| `"audio"` | Audio clip (WAV, MP3, OGG, FLAC). |
| `"file"` | Any other document, e.g. PDF. |

### CreateMemoryRequest

`POST /v1/memory` body; creates a note.

| Field | Type | Required | Description |
|---|---|---|---|
| `content` | string | yes | Text to remember; rejected with `secret_detected` when it looks like a secret. |
| `retention` | [Retention](#retention) \| null | no | Defaults to `memory.default_retention`. |
| `tags` | string[] | no | Optional labels. Default: `[]`. |

Unknown fields are rejected.

### CreatePairingRequest

Create a one-time pairing offer (Unix socket only).

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Label the paired device will carry. |
| `scopes` | [CapabilityScope](#capabilityscope)[] | yes | Scopes the device token will hold; at least one. |

Unknown fields are rejected.

### CreatePinnedTabRequest

`POST /v1/pinned-tabs`. Pinning an already pinned URL updates its title.

| Field | Type | Required | Description |
|---|---|---|---|
| `title` | string \| null | no | Optional display title (at most 512 characters). Default: `null`. |
| `url` | string | yes | http(s) URL. |

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

### Device

A paired device (never its token).

| Field | Type | Required | Description |
|---|---|---|---|
| `created_at` | string (date-time) \| null (date-time) | no | Pairing time, when known. |
| `device_id` | string | yes | Revocation identifier (`DELETE /v1/devices/{id}`). |
| `name` | string | yes | Label given at pairing; empty for devices paired before labels were stored. |
| `scopes` | [CapabilityScope](#capabilityscope)[] | yes | Scopes the token holds. |

### DeviceList

Paired devices.

| Field | Type | Required | Description |
|---|---|---|---|
| `devices` | [Device](#device)[] | yes | Devices in pairing order. |

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

- `kind=message.queued`: Message accepted durably into the queue. Fields: `attachments`: [Attachment](#attachment)[] (optional), `message_id`: [MessageId](#messageid), `mode`: [QueueMode](#queuemode), `text`: string.
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
- `kind=tool.ended`: Tool completed or failed. Fields: `is_error`: boolean, `result`: any, `tool_call_id`: string.
- `kind=artifact.saved`: A tool created or modified a file now kept in the session's artifact directory. Fields: `mime`: string, `path`: string, `size`: integer (uint64), `tool`: string, `tool_call_id`: string.
- `kind=ticket.opened`: A durable pending human-intervention ticket was created. Fields: `ticket`: [Ticket](#ticket).
- `kind=ticket.resolved`: Pending ticket resolved. Fields: `resolution`: any, `ticket_id`: [TicketId](#ticketid).
- `kind=ticket.rejected`: Pending ticket rejected. Fields: `reason`: string, `ticket_id`: [TicketId](#ticketid).
- `kind=ticket.expired`: Pending ticket expired. Fields: `ticket_id`: [TicketId](#ticketid).
- `kind=subagent.started`: The parent's `delegate` call was admitted and a child agent started. Fields: `model`: string, `parent_tool_call_id`: string, `subagent_id`: string, `task`: string, `tools`: string[].
- `kind=subagent.ended`: A child agent reached a terminal state; its result went to the parent. Fields: `reason`: string \| null (optional), `status`: [SubagentStatus](#subagentstatus), `subagent_id`: string.
- `kind=subagent.lifecycle`: Subagent lifecycle transition. Fields: `status`: string, `subagent_id`: string.
- `kind=subagent.progress`: Subagent progress update. Fields: `progress`: any, `subagent_id`: string.
- `kind=tab.opened`: Agent-owned browser tab created. Fields: `tab_id`: string, `url`: string.
- `kind=tab.closed`: Browser tab closed. Fields: `tab_id`: string.
- `kind=tab.asleep`: Browser tab discarded or unloaded. Fields: `tab_id`: string.
- `kind=session.created`: First event in a session log. Fields: `incognito`: boolean (optional), `mode`: [PolicyMode](#policymode) (optional), `model`: string, `title`: string, `workspace`: string \| null (optional).
- `kind=session.settled`: Runtime has no active turn.
- `kind=session.stopped`: User explicitly stopped the session; pending messages are retained.
- `kind=session.deleted`: Session removed by an administrator; the log remains as a tombstone and nothing can be appended after it.
- `kind=session.updated`: User-visible list metadata changed; omitted fields are unchanged. Fields: `archived`: boolean \| null (optional), `pinned`: boolean \| null (optional), `title`: string \| null (optional).
- `kind=runtime.started`: Sidecar ready handshake completed with an exact tool-gate match. Fields: `startup_ms`: integer (uint64), `tools`: string[].
- `kind=runtime.error`: Sanitized runtime failure; credentials and secrets must not appear here. Fields: `code`: string, `message`: string.

### EventKind

Public event payloads with stable `kind` identifiers, dotted except for `usage`.

- `kind=message.queued`: Message accepted durably into the queue. Fields: `attachments`: [Attachment](#attachment)[] (optional), `message_id`: [MessageId](#messageid), `mode`: [QueueMode](#queuemode), `text`: string.
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
- `kind=tool.ended`: Tool completed or failed. Fields: `is_error`: boolean, `result`: any, `tool_call_id`: string.
- `kind=artifact.saved`: A tool created or modified a file now kept in the session's artifact directory. Fields: `mime`: string, `path`: string, `size`: integer (uint64), `tool`: string, `tool_call_id`: string.
- `kind=ticket.opened`: A durable pending human-intervention ticket was created. Fields: `ticket`: [Ticket](#ticket).
- `kind=ticket.resolved`: Pending ticket resolved. Fields: `resolution`: any, `ticket_id`: [TicketId](#ticketid).
- `kind=ticket.rejected`: Pending ticket rejected. Fields: `reason`: string, `ticket_id`: [TicketId](#ticketid).
- `kind=ticket.expired`: Pending ticket expired. Fields: `ticket_id`: [TicketId](#ticketid).
- `kind=subagent.started`: The parent's `delegate` call was admitted and a child agent started. Fields: `model`: string, `parent_tool_call_id`: string, `subagent_id`: string, `task`: string, `tools`: string[].
- `kind=subagent.ended`: A child agent reached a terminal state; its result went to the parent. Fields: `reason`: string \| null (optional), `status`: [SubagentStatus](#subagentstatus), `subagent_id`: string.
- `kind=subagent.lifecycle`: Subagent lifecycle transition. Fields: `status`: string, `subagent_id`: string.
- `kind=subagent.progress`: Subagent progress update. Fields: `progress`: any, `subagent_id`: string.
- `kind=tab.opened`: Agent-owned browser tab created. Fields: `tab_id`: string, `url`: string.
- `kind=tab.closed`: Browser tab closed. Fields: `tab_id`: string.
- `kind=tab.asleep`: Browser tab discarded or unloaded. Fields: `tab_id`: string.
- `kind=session.created`: First event in a session log. Fields: `incognito`: boolean (optional), `mode`: [PolicyMode](#policymode) (optional), `model`: string, `title`: string, `workspace`: string \| null (optional).
- `kind=session.settled`: Runtime has no active turn.
- `kind=session.stopped`: User explicitly stopped the session; pending messages are retained.
- `kind=session.deleted`: Session removed by an administrator; the log remains as a tombstone and nothing can be appended after it.
- `kind=session.updated`: User-visible list metadata changed; omitted fields are unchanged. Fields: `archived`: boolean \| null (optional), `pinned`: boolean \| null (optional), `title`: string \| null (optional).
- `kind=runtime.started`: Sidecar ready handshake completed with an exact tool-gate match. Fields: `startup_ms`: integer (uint64), `tools`: string[].
- `kind=runtime.error`: Sanitized runtime failure; credentials and secrets must not appear here. Fields: `code`: string, `message`: string.

### ExchangePairingRequest

Exchange a pairing code for a device token (no bearer token needed).

| Field | Type | Required | Description |
|---|---|---|---|
| `code` | string | yes | Code from the pairing offer. |

Unknown fields are rejected.

### Health

Health response without environment or credential details.

| Field | Type | Required | Description |
|---|---|---|---|
| `api_version` | string | yes | Public API version, currently `v1`. |
| `ok` | boolean | yes | Whether the daemon is ready to serve requests. |

### Memory

One stored memory. Content never contains a value matched by secret detection.

| Field | Type | Required | Description |
|---|---|---|---|
| `content` | string | yes | Remembered text. |
| `created_at` | string (date-time) | yes | First write. |
| `expires_at` | string (date-time) \| null (date-time) | no | Purge deadline; absent for permanent memories. |
| `id` | string | yes | Opaque memory identifier. |
| `kind` | [MemoryKind](#memorykind) | yes | Memory origin. |
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
| `id` | [MessageId](#messageid) | yes | Idempotency key, assigned by the client or daemon. |
| `status` | [MessageStatus](#messagestatus) | yes | Pending or terminal disposition. |
| `text` | string | yes | Latest text, including edits made before delivery. |

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

### MoveMessageRequest

Pending message reorder within its delivery lane.

| Field | Type | Required | Description |
|---|---|---|---|
| `position` | integer (uint32) | yes | Zero-based index inside the message's own lane (steer or queue); values past the end move it last. A follow-up becomes a steer only through promote. |

Unknown fields are rejected.

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

### PairDeviceRequest

Administrative device pairing request.

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | User-visible device label. |
| `scopes` | [CapabilityScope](#capabilityscope)[] | yes | Explicit capabilities to grant. |

Unknown fields are rejected.

### PairDeviceResponse

Administrative pairing result; the secret token is returned once.

| Field | Type | Required | Description |
|---|---|---|---|
| `device_id` | string | yes | Stable revocation identifier. |
| `scopes` | [CapabilityScope](#capabilityscope)[] | yes | Granted capabilities. |
| `token` | string | yes | Bearer capability token; never persist in event or application logs. |

### PairSyncRequest

`POST /v1/sync/pair`.

| Field | Type | Required | Description |
|---|---|---|---|
| `pairing_code` | string | yes | Code printed by `openside sync setup` on the first device. |

Unknown fields are rejected.

### PairingOffer

A one-time pairing offer, valid for [`PAIRING_TTL_SECS`].

| Field | Type | Required | Description |
|---|---|---|---|
| `code` | string | yes | One-time code; exchanged at most once. Show it only to the person pairing. |
| `expires_at` | string (date-time) | yes | Instant after which the code is refused. |
| `path` | string | yes | Path the phone opens on the daemon's origin; the code is in the fragment, so it never reaches server logs or `Referer` headers. |
| `scopes` | [CapabilityScope](#capabilityscope)[] | yes | Scopes the resulting token will hold. |
| `url` | string \| null | no | `remote.public_url` + `path`, when a public URL is configured. |

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

### PolicyMode

Public permission modes, mirroring `openside_policy::Mode`.

| Value | Description |
|---|---|
| `"read-only"` | Read only; deny mutations. |
| `"guard"` | Guard; work inside approved roots, ask elsewhere. |
| `"full"` | Full access; sensitive operations still require approval. |

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

### RejectTicketRequest

Explicit ticket rejection.

| Field | Type | Required | Description |
|---|---|---|---|
| `reason` | string \| null | no | Optional sanitized rejection detail. |

Unknown fields are rejected.

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

### ResolveTicketRequest

Human response interpreted according to the ticket kind. Secret-input values are daemon-only and must never be persisted or sent to the model.

| Field | Type | Required | Description |
|---|---|---|---|
| `value` | any | yes | Confirmation, non-secret information, or transient daemon-only secret input: a string, or `{"totp": "<otpauth:// URI or base32 key>"}` for a `secret_input` ticket whose `otp.can_register` is true. A `confirm` ticket carrying `otp_register` takes `true` or `false` like any confirmation. |

Unknown fields are rejected.

### Retention

How long a memory is kept after its last write.

| Value | Description |
|---|---|
| `"permanent"` | Never purged automatically. |
| `"days_90"` | Purged 90 days after the last write. |
| `"days_30"` | Purged 30 days after the last write. |

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
| `paused_reason` | [RoutinePauseReason](#routinepausereason) \| null | no | Set when the daemon paused the routine on its own; cleared by resuming. |
| `schedule` | [RoutineSchedule](#routineschedule) | yes | Firing schedule. |

### RoutineAction

What a routine does when it fires.

- `type=new_session`: Create a fresh session and submit the prompt as its first message. Fields: `mode`: [PolicyMode](#policymode) \| null (optional), `model`: string \| null (optional), `prompt`: string, `workspace`: string \| null (optional).
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
| `archived` | boolean | no | Hidden from the default session list. Default: `false`. |
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
| `remote` | boolean | yes | Whether this caller is a paired device over TCP, i.e. limited to `remote_editable`. |
| `remote_editable` | string[] | yes | Dotted keys a TCP (paired-device) caller may change; an entry ending in `.` covers every key under that prefix. Everything else changes only over the local Unix socket. |
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
| `ended_at` | string (date-time) \| null (date-time) | no | When the child reached a terminal state. |
| `id` | string | yes | Daemon-assigned subagent identifier, also used as `ticket.origin.subagent_id`. |
| `last_progress` | any | no | Latest `subagent.progress` payload. |
| `model` | string | yes | Model selector the child runs on. |
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
| `last_sync` | string (date-time) \| null (date-time) | no | Last completed sync. |
| `pending` | integer (uint64) | yes | Local changes not yet stored on the backend. |
| `records` | integer (uint64) | yes | Records known to this device, tombstones included. |
| `set_up` | boolean | yes | Whether this device joined an account (`setup` or `pair`). |
| `unlocked` | boolean | yes | Whether the sync key is derived and held in memory. |

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

### Ticket

Durable human-intervention ticket correlated with the same deferred tool call.

| Field | Type | Required | Description |
|---|---|---|---|
| `created_at` | string (date-time) | yes | Creation time in RFC 3339 UTC. |
| `deferral` | [TicketDeferral](#ticketdeferral) | yes | Approval or external-result resumption semantics. |
| `expires_at` | string (date-time) | yes | Deadline in RFC 3339 UTC. |
| `id` | [TicketId](#ticketid) | yes | Stable ticket identifier. |
| `instructions` | string \| null | no | Optional redacted instructions for completing the action. |
| `kind` | [TicketKind](#ticketkind) | yes | Requested human action. |
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

### TicketDeferral

Resumption semantics for the suspended tool call.

| Value | Description |
|---|---|
| `"approval"` | Approval permits the original tool call to execute again. |
| `"external"` | An external action supplies the original tool call's result. |

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

### TicketList

Ticket list, optionally filtered by status.

| Field | Type | Required | Description |
|---|---|---|---|
| `tickets` | [Ticket](#ticket)[] | yes | Matching durable tickets. |

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
| `subagent_id` | string \| null | no | Absent for the parent agent, otherwise the originating subagent. |

### TicketOtp

One-time-code details of a `secret_input` ticket opened by `request_otp`.

| Field | Type | Required | Description |
|---|---|---|---|
| `can_register` | boolean | yes | Besides a code (a string `value`), the person may resolve with `{"totp": "<otpauth:// URI or base32 setup key>"}`: the daemon stores the authenticator in the vault for the ticket's origin and fills the code it generates, now and on later `request_otp` calls for that origin. |
| `channel` | [OtpChannel](#otpchannel) | yes | Where the code comes from. |

### TicketOtpRegister

What a `confirm` ticket opened by `register_authenticator` asks the person to approve: saving the authenticator a site's two-factor setup page shows into the vault, bound to exactly `origin`. The setup key itself is held by the daemon only and is never part of the ticket. Approving (`true`) saves it and fills the page's confirmation code; declining (`false` or rejecting the ticket) discards it.

| Field | Type | Required | Description |
|---|---|---|---|
| `account` | string \| null | no | Account named by the setup page's `otpauth://` URI, when it has one. |
| `issuer` | string \| null | no | Issuer named by the setup page's `otpauth://` URI, when it has one. |
| `origin` | string | yes | Exact page origin the authenticator will be bound to. |

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

### UpdateMemoryRequest

`PATCH /v1/memory/{id}` body; absent fields are unchanged.

| Field | Type | Required | Description |
|---|---|---|---|
| `content` | string \| null | no | Replacement text. |
| `retention` | [Retention](#retention) \| null | no | Replacement retention; the purge deadline restarts from this write. |
| `tags` | string[] \| null | no | Replacement labels. |

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

### UpdateVaultItemRequest

`PATCH /v1/vault/items/{id}`.

| Field | Type | Required | Description |
|---|---|---|---|
| `policy` | [VaultPolicyOverride](#vaultpolicyoverride) | yes | New override. |

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
| `what` | string | yes | `fill`, `otp`, `otp_register`, `passkey_register`, `passkey_login`, `get`, `policy_update`, `item_add`, `item_delete`, or the lifecycle steps `setup`, `unlock` and `lock`. |
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
