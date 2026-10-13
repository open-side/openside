# Connections

Connections are metadata about sign-ins in the managed agent browser. Cookies remain in Chromium; passwords, authenticators and passkeys remain in the Vault. A connection groups a registrable site and account, not an API credential. Public-suffix grouping uses `psl`, including private suffixes so hosted tenants remain separate.

## Storage and privacy

`connections.db` stores masked identities, HMAC account keys, status, source, cookie-key fingerprints and per-agent grants. Cookie-key fingerprints contain domain, name, path, expiry and flags, never cookie values or value hashes. Identity normalization is trim plus lowercase; the identity HMAC uses a random 32-byte `connections.key` protected by the credential file/inode/sandbox guards. Raw identities are transient and masked before SQLite writes. The identity HMAC is not returned by the API.

A sign-in baseline may temporarily compare HMAC-SHA256 cookie values under a fresh, random in-memory key. Trusted CDP cookie values are zeroized after hashing. Neither this key nor its digests are serialized, logged, returned, or persisted. This detects rotation even when name, domain, path and expiry are unchanged. The durable fingerprint contains only the changed key metadata, preferring secure HttpOnly cookies. With no change, the previous fingerprint is reused; otherwise persistent secure HttpOnly site keys provide liveness after an authoritative sign-in. Baselines expire after ten minutes and are not restored after restart.

`connections-audit.jsonl` records detected, verified, expired, needs-login, grant, forget and sign-out metadata. Events contain only connection ID, site and masked identity. URL queries and fragments are removed before storing a home URL. `browser_repl` redacts `value` from every `Network.getCookies` result, including explicitly supplied frame URLs; metadata remains available.

## Security

Cookie APIs never expose cookie values. Credential-like cookie outputs are guarded in REPL and `connection_fetch`; page-rendered text remains page content.

Before and after a REPL snippet, the host validates tab ownership and reads every cookie in that tab's managed browser context through trusted `Storage.getCookies`, with no URL filter. This includes path-scoped cookies and cookies sent by credentialed cross-site fetches, not just the current frames' cookies. Cookie output guards use token-boundary matches for values of at least twelve UTF-8 bytes, avoiding short benign preference values corrupting normal cards or tool results. Temporary cookie values are zeroized. Result values, console output, errors and durable tool events use these guards; direct CDP cookie results independently replace every value with `[REDACTED]`. Failure to establish the trusted cookie inventory fails closed. Reused request headers retain strict substring guards rather than the cookie-specific filter.

Ordinary page-content tools such as text and snapshots render page content as-is; they do not newly classify page-rendered text as a secret merely because the page printed its own JavaScript-readable cookie. Existing registered secret redactors still apply.

Sign-out remembers committed top-frame, subframe, and worker origins for tracked shared-profile sites. It supplements that registry with read-only SQLite inventories of the managed profile's `WebStorage/QuotaManager` bucket storage keys and `History` URLs. Online backup provides a WAL-consistent snapshot when available. If Chromium exclusively locks a source, the host copies its main database and WAL into a private temporary directory and reads that copy only after `PRAGMA quick_check` succeeds. An unreadable or invalid inventory is logged and skipped, never a reason to fail sign-out. Matching live tabs (including tabs embedding a matching frame) close before origin service-worker registrations are removed; only then are cookies and the union of known origins' storage cleared. Unrelated sites' stored data is retained.

Residual limitation: localStorage for an origin that was never recorded, never visited as a top-level history entry, and has no readable quota bucket cannot be inventoried by these bounded sources. A validated copy of a locked database can still omit concurrent changes; the committed-origin registry remains the primary source. Sign-out does not claim to erase undiscoverable origins. No live LevelDB files are decoded, copied, or opened for writing.

## Detection

Observed login, vault, takeover and completed OTP flows capture a baseline before sign-in and register an active connection only after the page is no longer a login, OTP, challenge or error page. Incognito and isolated spaces do not enter the shared-profile registry. Imported persistent HttpOnly cookie keys are grouped into unverified connections. Import metadata never carries the original cookie values and cannot downgrade an already active connection or reset its grants.

The weak sweep runs on listing, debounced to sixty seconds, and every fifteen minutes while the managed browser is running. It does not launch a browser. A connection expires when all fingerprint keys are absent or expired. Secure persistent HttpOnly cookies plus recorded browsing history can create a candidate, never an active connection. Dismissing a candidate suppresses later heuristic rediscovery.

Check opens a background tab under Browse policy, checks the page classifier, reads generic account controls/landmarks in an isolated world, and closes the tab. If its navigation needs a connection grant, the blocked blank tab is retained so approval and a retry can use the same tab-bound once grant; a successful retry closes it. Identity labels are not authorization. Multiple identities discovered in the site's own account menu share a cookie group; grants remain per connection. The default chief grant is allowed; unspecified teammates use ask. Signing in permits the session's signing agent; legacy sessions without an agent tag belong to chief.

## Agent tools

`connections_list` returns metadata for connections not denied to the calling agent. The chief is allowed by default; teammates ask unless explicitly allowed or they performed the sign-in. Retired agents cannot use retained grants. Tasks and helpers use their parent session's immutable agent identity.

`connection_open` opens or reuses an owned same-site tab. An ask grant produces a person-only Confirm offering **Allow once** or **Always for this agent**, and displays the requesting tab's origin. Shared-jar prompts list every matching account; both open and fetch require all matching grants, with denied winning. The grant is separate from Browse policy and expiry checks. Once grants are bound to the listed connection IDs, agent, session, requesting origin, tool and Task principal; durable consumption survives a restart and occurs only after action-policy admission. A newly discovered account is not covered by an older once grant. Always writes the listed per-agent connection grants. `connection_check` performs the existing background probe under Browse policy; `connection_confirm` requires allowed access and stores only a masked page-derived identity.

Connection grants fence every request from a restricted agent's tabs: documents, subframes, fetch/XHR, subresources and owned workers. An agent with any non-allowed connection uses all-resource interception; chief and fully-allowed agents keep Document-only interception. Each matching shared-jar account must permit the session's immutable agent, with denied winning; separate account profiles use their own rows and cannot restrict the shared jar. A denied request fails with `connection_denied`. Ask fails immediately with `connection_grant_required` and asynchronously opens one deduplicated, person-only combined-account Confirm, never holding a browser request for a person.

After approval the agent explicitly retries. Once is durably claimed by the first matching request and authorizes the ticket's exact session and tab for that top-level document's lifetime, including subresources, worker requests and redirect hops. A new top-level document or tab closure ends that authorization. Always persists all listed connection grants. `connection_open` uses only its consumed ticket-bound IDs and probes an unverified account in that same owned tab, not a second tab. A grant downgrade, revoke or Forget invalidates document authorizations, closes the affected agent's tabs whose top or frame origin matches the site, and re-arms interception before publishing the change. Restricted tabs bypass service workers so a cached response cannot bypass admission.

Chromium workers do not support per-target Fetch interception. While a restricted owned worker is alive, the browser-level fence additionally intercepts only its protected site URLs, using bounded request-ID-to-owner metadata to apply the same grant checks. A chief request to the same URL still passes, but pays this narrow interception overhead until the restricted worker closes. Worker requests whose restricted ownership cannot be established fail closed; ordinary fully-allowed targets retain their Document-only patterns.

`connection_observe` returns fetch/XHR request ID, method, path without query, status, content type and header names, never header values or bodies. It only observes an owned tab. Request header values stay in a zeroizing, daemon-only cache of at most 512 tracked requests per tab; they are absent from `NetworkEvent`. Cookie headers are not retained for reuse, and authentication/cookie response headers are scrubbed from network metadata.

`connection_fetch` runs in a fresh isolated world on the owned tab's exact top frame, with pristine globals unreachable by page monkey-patching. It refuses another origin, profile or account space. Before injecting reused headers, request-stage interception is armed for the tab and aborts every request leaving the exact origin. Chromium sends its own cookies with `credentials: 'include'`; redirects stay manual and are refused. Service workers are bypassed only for the scoped request, and the prior bypass setting is restored even on errors. GET/HEAD use Browse. Other methods always require a Confirm bound to the complete call, method, path, origin and tab, even in Full mode; labels matching sensitive actions also use Sensitive policy. Deny cannot be overridden. Reused non-cookie headers are injected only through a sensitive daemon-to-page CDP call, after redactors are installed. Cookie guards use the full request URL before and after the request, including observed same-origin hop/redirect target URLs, so path-scoped cookies and rotated cookies are redacted. Live values are transient trusted-CDP inputs, never returned or persisted.

Response text is streamed up to 64 KiB, redacted, and capped again on UTF-8 boundaries after redaction. The tab is tainted before the request, withholding screenshots and previews. A 401/403 or login redirect marks the connection as needing login and tells the agent to use `request_login`. Connection-use events and audit entries contain metadata; fetch audit detail is method plus path only, never query values, bodies or header values. Browser profiles expose the six tools; the chief front can list connections and observe request metadata, while acting tools remain Task work.

## Admin API

Every route below requires Admin, including reads and audit. Every mutating route additionally requires a person-device caller (the local socket is exempt): grant/home-URL updates, checks, relogin, revoke, candidate dismissal, and account-space creation/deletion. Admin agent clients may read metadata and audit but cannot grant themselves access or mutate connections.

| Method | Path | Result |
|---|---|---|
| GET | `/v1/connections?agent=&status=` | `ConnectionList`, with candidates separated |
| GET | `/v1/connections/{id}` | Metadata only |
| PATCH | `/v1/connections/{id}` | Grants and same-site home URL |
| POST | `/v1/connections/{id}/check` | Refreshed connection |
| POST | `/v1/connections/{id}/relogin` | `ReloginResult` |
| POST | `/v1/connections/{id}/revoke` | 204 |
| POST | `/v1/connections/candidates/{id}/dismiss` | 204 |
| GET | `/v1/connections/audit?connection=&agent=&action=&limit=` | Newest-first audit records |
| POST | `/v1/connections/spaces` | `{space_id, connection_id, result: ReloginResult}`; body `{site}` |
| DELETE | `/v1/connections/spaces/{space_id}` | 204; stop the space, delete its profile and connection |

Relogin accepts an optional `session_id`. Without one, it uses the chief agent's durable home session. Silent Vault completion is preferred: the response is `{status:"signed_in"}` or `{status:"needs_you",ticket:{session_id,ticket_id}}`.

When silent sign-in needs another step, relogin opens the existing matching OTP, device-approval or Login ticket. Unrecognized actionable pages use a **Finish signing in** takeover ticket. These continuations pass through the normal policy, browser ownership and ticket gates; they do not expose an extra model tool.

Forget removes the connection and its grants without clearing browser authentication. When no shared-profile connection rows remain for the site, it is an ordinary site again: agents can still browse it with the retained browser sign-in. Use Sign-out to end the sign-in. Sign-out closes managed tabs on the site and clears site cookies and origin storage through host-only CDP while browser/connection mutation gates are held. Other shared-profile accounts on that site need to sign in again. An optional Vault deletion is separately requested with `delete_vault_item`.

## Separate account spaces

The site's own account switcher remains the default. A person can opt in to an additional persistent profile using `POST /v1/connections/spaces`. Creating or deleting a space requires Admin plus a person-device caller (the local socket is exempt). Creation records a **Needs login** connection and starts the existing login flow in the chief's home session. The response identifies both the space and connection, so an interrupted sign-in can be retried from the Connections page.

Profiles live under `browser-profiles/accounts/<space_id>/` with mode `0700`. They launch only when used; a restart never restores running spaces. A space closes after ten minutes without a tool call or owned open tab. Opening another space for the same site closes the first process and its tabs before launching the replacement. The shared browser's sign-in is unaffected.

Each active space costs roughly 200–400 MB RAM, a 1–3 second cold start, and 50–150 MB disk, and requires its own sign-in and two-factor authentication. It uses the same Chromium resource limits and process priority as the main agent browser. Imports, history indexing, prewarm and pinned-tab restoration do not target account spaces.

Tab-specific tools, Vault fills, human-step targets, takeover and previews resolve the tab's browser instance rather than assuming the session's shared browser. Connection checks and relogin select the profile from `account_space`; shared-profile cookie sweeps never expire account-space rows. Both revoke modes on a space connection stop its process and delete its entire directory, not the shared profile's site data.

## CLI

All connection commands use the same Admin API routes as the web page. IDs are opaque connection IDs; the grant's agent argument is a stable team agent ID.

```text
openside connections list [--site <site>]
openside connections show <id>
openside connections check <id>
openside connections relogin <id>
openside connections revoke <id> [--keep-signed-in]
openside connections grant <id> <agent> allowed|ask|denied
openside connections audit [--connection <id>] [--agent <agent>] [--action <action>]
openside connections spaces add <site>
openside connections spaces remove <space_id>
```

`list --site` filters both ordinary connections and candidates by the exact registrable site, preserving `ConnectionList` and its `browser_available` flag. The API list has no site query, so this filter is applied by the CLI. `show`, `check` and `grant` return the canonical `Connection`; grant patches one agent without replacing other grants.

`relogin` uses the chief's durable home session and preserves the canonical `ReloginResult`. On `needs_you`, stderr prints the ticket/session IDs and an `openside ticket resolve` continuation with the same socket or remote endpoint. The JSON summary reports `waiting_human`; sign-in is not claimed complete. `spaces add` returns `CreateAccountSpaceResult` and prints the same continuation when needed.

**Revoke signs out by default**, sending `{sign_out:true,delete_vault_item:false}`. It removes the connection and all grants and clears every shared-profile account's sign-in on that site. `--keep-signed-in` sends `sign_out:false` to forget only; it does not delete a linked Vault login. For a separate account space, either revoke mode deletes the whole profile, as described above. `spaces remove` stops and deletes the named profile and its connection.

Audit filters are combined. Action names are `detected`, `verified`, `expired`, `needs_login`, `relogin`, `open`, `fetch`, `grant`, `forget` and `sign_out`; records remain newest-first. Metadata audit responses never contain cookies, request header values, query values or bodies.

The global `--json` flag emits canonical API objects followed by the existing CLI summary, one object per line. A 204 revoke or space removal emits only that summary, not a fabricated response object. Human continuations stay on stderr so stdout remains machine-readable.

Focused checks:

```sh
cargo test -p openside-cli connections -j 2 -- --test-threads=1
cargo test -p openside-e2e --test connections -j 2 -- --test-threads=1
```

The e2e test uses disposable HOME/XDG directories, a real daemon and scripted Python provider, headless Chromium, and loopback login/account pages. It follows a Login ticket into an active row, asks before a teammate opens the sign-in, persists **Always for this agent**, fetches authenticated same-origin data, compares CLI list/show/check/grant/audit to the API, revokes with sign-out, and verifies both cookies and origin storage are gone. Cookie/password canaries are checked against metadata stores, events, audit, sidecar state and daemon logs; Chromium's private authentication storage and the fixture's login input are intentional secret destinations.

