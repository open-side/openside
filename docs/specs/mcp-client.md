# MCP client — external MCP servers as agent tools

openside can connect to MCP servers (Settings › Plugins & MCPs, `/v1/mcp/servers`) and offer their tools to agents in every chat. This is the opposite direction of [`mcp.md`](mcp.md), where openside itself is the MCP server.

## Where it runs

The daemon (`crates/opensided/src/mcp/`) owns every connection, not the agent sidecar. Secrets therefore never leave the daemon, and every call goes through `host::execute` like a built-in tool: profile and mode filtering, policy decision, approval ticket, argument binding, redaction of Vault values, `tool.started`/`tool.ended` events and the audit trail. The sidecar only proxies the daemon's `ToolSpec`s and needs no change.

## Transports

| `transport` | Configuration | Behaviour |
|---|---|---|
| `stdio` | `command` (argv, no shell), optional absolute `cwd` (default: the data directory) | The program runs in its own process group with a cleared environment: only `PATH`, `HOME`, `USER`, `LOGNAME`, `LANG`, `LC_ALL`, `LC_CTYPE`, `TMPDIR`, `SHELL`, the `XDG_*` base directories (and the Windows profile variables) are inherited, plus the server's secrets. The group is killed when the server is disabled, changed, removed or the daemon stops. The last 1 KiB of stderr is attached (redacted) to a failed start. |
| `http` | `url`: `https://` without user info, query or fragment, or `http://` on a loopback host; optional `allow_private_network` (default `false`) | Streamable HTTP: every message is a POST with `Accept: application/json, text/event-stream`; responses may be JSON or an SSE stream, notifications get 202. `Mcp-Session-Id` and `MCP-Protocol-Version` are sent after the handshake. The host is resolved once per connection and every address must pass the address check below; the connection is pinned to the checked address so a later DNS answer cannot move it. Redirects are never followed, so configured credentials cannot be forwarded to another host. The legacy two-endpoint HTTP+SSE transport (2024-11-05) is not supported. |

The client speaks protocol version `2025-06-18`: `initialize` → `notifications/initialized` → `tools/list` (paginated, at most 512 tools) → `tools/call`. Server requests other than `ping` get `-32601`; `notifications/tools/list_changed` re-lists tools. Cancelling a call sends `notifications/cancelled`. Timeouts: 30 s for the handshake, listing and every notification, 120 s per call (an http request is cut off after 125 s whatever it carries); messages are capped at 8 MiB in both directions.

### Address check

Every resolved address of an http server must be loopback or globally routable (`net_guard::public_ip`, shared with Web Push delivery, card images and web fetches). Not globally routable: `0.0.0.0/8`, `10.0.0.0/8`, `100.64.0.0/10`, `127.0.0.0/8`, `169.254.0.0/16`, `172.16.0.0/12`, `192.0.0.0/24`, `192.0.2.0/24`, `192.88.99.0/24`, `192.168.0.0/16`, `198.18.0.0/15`, `198.51.100.0/24`, `203.0.113.0/24`, `224.0.0.0/3` (multicast, reserved, broadcast) and Azure's `168.63.129.16`; for IPv6 everything outside `2000::/3` (loopback, IPv4-compatible and mapped, NAT64 `64:ff9b::/96` and `64:ff9b:1::/48`, discard `100::/64`, unique-local, link-local, site-local, multicast) plus `2001::/23`, `2001:db8::/32`, `2002::/16` (6to4), `3ffe::/16` (returned 6bone) and `3fff::/20`. IPv4-mapped addresses are checked as IPv4.

`allow_private_network: true` additionally accepts the networks a person runs servers on: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, the shared `100.64.0.0/10` range (Tailscale) and unique-local `fc00::/7`, except the metadata services clouds place there (`100.100.100.200`, `fd00:ec2::254`). Link-local addresses, where most cloud metadata services answer, and every other range above stay refused. The TLS rule for the URL does not change. Stdio servers cannot set it. Like every MCP change it needs local access.

## Secrets

`secrets` in the configuration lists names only: environment variables for stdio servers, header names (for example `Authorization`) for http servers. Values are typed `mcp_secret` Vault items (`mcp:<server>:<lower-case name>`, no origins, never synced or offered for browser fill), written by `POST`/`PATCH /v1/mcp/servers` and deleted with the server. Values must be 1–8192 bytes without CR, LF or NUL. Saving a value installs its redactor first, so it is masked in every later tool result, event and error. A server with secrets waits in `vault_locked` while the Vault is locked and is retried every 30 seconds.

## Tools

- Name: `mcp__<server id>__<tool>`, cut at 64. Tools whose protocol name is not 1–128 characters of `[A-Za-z0-9_.-]` are skipped, so a name cannot carry control characters or delimiters into policy targets, logs or approval prompts. Within a server the first tool keeps a duplicate name; across servers the first server in configuration order wins.
- Description: `[MCP: <label>] <server description>`, every control character (newlines included) replaced with a space, at most 2048 characters. Labels cannot contain control characters.
- Everything the server reports about itself (server name, tool names, descriptions, schemas) is masked with the Vault redactors before it is kept, because the server can read its own secrets.
- Tools whose `inputSchema` is not an object schema or is larger than 64 KiB are skipped.
- Listed for the `full` profile only and hidden in read-only sessions (approval tier `exec`).
- Policy action: `exec` on `mcp:<server id>/<tool>`. Guard mode asks the person ("Allow openside to use the <tool> tool from <label>?"), full access runs, read-only denies. `readOnlyHint` and other annotations are reported in the API but never relax approval.
- Result for the model: `{"source":"mcp","server","tool","untrusted":true,"authority":"data_only","content","structured"?}`; text items are joined, PNG/JPEG/GIF/WebP images become image parts, other items are passed as JSON text. `isError: true` becomes a failed tool call whose text is the same envelope with `"is_error": true`. Text is masked like every tool result; image parts are binary and cannot be masked, so a server that draws a secret into an image is outside what redaction covers.
- Adding, changing or removing a server, or a server's `notifications/tools/list_changed`, bumps a generation; each chat reopens its runtime before the next turn so the tool list is current. The generation is checked again right before the turn is sent, and a change that landed while the turn was being prepared reopens the runtime once more, so that turn already sees it.

## API

| Method | Path | Scope | Notes |
|---|---|---|---|
| GET | `/v1/mcp/servers` | read | Servers with `status` (`disabled`, `connecting`, `connected`, `vault_locked`, `failed`), `error`, `server_name`, `allow_private_network`, `tools` |
| POST | `/v1/mcp/servers` | admin + local access | 201; `id` is derived from `label` when absent. 409 `mcp_server_exists`, `mcp_server_limit` (32), `vault_unavailable`; 403 `biometric_required` when a new keyring vault would be needed while biometric unlock is on |
| GET | `/v1/mcp/servers/{id}` | read | 404 `mcp_server_not_found` |
| PATCH | `/v1/mcp/servers/{id}` | admin + local access | `secrets: {name: value \| null}` sets or removes values; `allow_private_network` turns the private-network opt-in on or off; reconnects |
| DELETE | `/v1/mcp/servers/{id}` | admin + local access | 204; stops the server and deletes its secrets |
| POST | `/v1/mcp/servers/{id}/refresh` | admin + local access | Reconnect and re-list |

Local access (`403 local_access_required` otherwise) is required for changes because a stdio server runs a program on this computer. The configuration is stored as `[[mcp.servers]]` in `config.toml`. `[mcp]` is not remotely editable through `PUT /v1/settings`; a local edit there (or of the file) takes effect after a daemon restart, while the MCP API applies at once. Presets offered by the Settings page (for example a local Figma or Blender server) live in the web app.
