# Read-only share links

A person can publish a frozen, read-only snapshot of one chat and hand out its link. Anyone
holding the link reads the snapshot in a browser without a device token, until the link is
revoked, expires, or the chat is deleted. Code: `crates/opensided/src/shares.rs` (daemon),
`crates/openside-proto/src/shares.rs` (wire types), `web/src/components/ShareDialog.tsx`
(chat menu → Share…) and `web/src/pages/SharePage.tsx` (the public `/share` page).

## Routes

| Route | Scope | Notes |
| --- | --- | --- |
| `GET /v1/sessions/{id}/shares` | `read` | Live links of the chat, newest first. |
| `POST /v1/sessions/{id}/shares` | `steer`, person only | Takes the snapshot; `201` with the `ShareLink`. |
| `DELETE /v1/sessions/{id}/shares/{share_id}` | `steer` | Revokes at once; `404` for another chat's link. |
| `GET /v1/shares/{share_id}` | public | The `SharedChat` snapshot. |
| `GET /v1/shares/{share_id}/artifacts/{artifact}` | public | One copied file, by index. |

The two `GET /v1/shares/…` routes are in `PUBLIC_ROUTES`: the 43-character id (32 bytes
from the OS RNG, base64url) is the credential, like a signed preview URL. Only a caller whose
message author kind is `person` may publish (the local socket, or a paired person device
without the MCP or channel client header); an agent client or channel adapter gets `403
person_required` even with the `steer` scope, so an agent cannot publish a chat on its own.

`CreateShareRequest`: `artifacts` (paths from `GET /v1/sessions/{id}/artifacts`, at most 20,
no duplicates), `steps` (default `true`), `expires_in_secs` (`null` = until revoked, else
60 s to 365 days). Refusals: `invalid_expiry`, `invalid_share_artifact` (unknown, working or
duplicate path, or the transcript), `unsupported_share_artifact` (a file that is neither UTF-8
text nor a PNG, JPEG, GIF or WebP image), `sensitive_artifact`, `share_too_large` (4 MiB of
conversation, 10 MiB per file, 25 MiB of files), `too_many_shares` (50 live links per chat),
`private_session` (incognito chats), `vault_locked`. Sizes are charged while the snapshot is
built: the conversation stops at the first message past its budget, and the chosen files are
checked against the per-file and total limits from their indexed sizes before any is read
(and again by the bytes actually read).

## What a snapshot holds

The snapshot is built from the chat's durable event log when it is taken; later messages are
not part of it.

- Kept: messages delivered to the agent (`message.injected`, with the latest queued or edited
  text), non-empty final replies, and, when `steps` is on, one entry per tool call with only
  the tool name and whether it failed.
- Never kept: tool arguments and results, thinking, tickets and their answers, queued or
  dropped messages, cards, attachments the person did not pick, the transcript, and any
  run or model metadata.

Every string (title, messages, replies, file names, text files) passes the live redaction
filters (`Registry::redact`: vault items and session secrets become `[REDACTED]`), and then
`openside_memory::looks_secret` line by line: a line that still looks like an API key, token,
password assignment, card number, one-time code, cookie header or session token is replaced
whole with `[REDACTED]`. A set-up vault must be unlocked before publishing (`409
vault_locked`), so its filters are installed.

This screening is best effort, not a guarantee: exact vault and session values are always
masked, but the shape check only knows common credential patterns, one line at a time. A
value that is encoded (base64, compressed), obfuscated or split across lines or messages is
not recognised. What a snapshot holds is ultimately the person's choice when they publish it.

Only two kinds of file can be shared, because anything else can hide content the filters
cannot see (PDF text layers, archives, office documents): those are refused with
`unsupported_share_artifact`.

- Text (valid UTF-8 without NUL bytes) goes through the same filters; one that still looks like
  a credential afterwards is refused with `sensitive_artifact`. It is stored and served as
  `text/plain; charset=utf-8`, so shared HTML or SVG never renders.
- Raster images are decoded one at a time (at most 16384 px a side and 192 MiB of pixels) and re-encoded:
  JPEG as JPEG, the rest as PNG, scaled down to fit 4096 px. Re-encoding drops every metadata
  chunk (EXIF and GPS, XMP, ICC profiles, PNG text). The pixels themselves are shared as they
  are, so the dialog shows the chosen images with a note saying so before the link is made.

## Serving

Public responses carry `Cache-Control: no-store`, `Referrer-Policy: no-referrer`,
`X-Robots-Tag: noindex` and `X-Content-Type-Options: nosniff`, errors and refusals included.
Files are served with `Content-Security-Policy: sandbox; default-src 'none'`; images inline,
text as an attachment. A file is streamed in 64 KiB chunks rather than read into memory.

Reads are limited per caller and per link, so one holder of a link cannot use it up for
everyone else, and a caller's guessing cannot lock other callers out:

| Budget | Limit |
|---|---|
| Requests to one link, per caller / all callers | 120 / 600 per minute |
| Lookups of unknown, revoked, expired or malformed ids, per caller / all callers | 60 / 600 per minute |
| File bytes served by one link | 256 MiB per minute |
| Requests in flight, per caller on one link / per link / per daemon | 4 / 16 / 32 |

Every refusal is `429 share_rate_limited` with `Retry-After: 60`. A caller is the socket peer,
except that a loopback peer is this daemon's own tunnel when that tunnel is enabled, and the
tunnel's header names the visitor: Cloudflare's `cf-connecting-ip`, or for Tailscale the last
`x-forwarded-for` hop. Those headers are ignored from non-loopback peers. Per-visitor limits
through a tunnel rely on that header; a local process can set it too, and local processes are
trusted as the owner.

`ShareLink.url` is the full link through remote access: the first enabled Tailscale or
Cloudflare base URL, else `remote.public_url`. Relay access is skipped, because a visitor
without a paired device cannot use it. With no remote access the field is `null` and the web
app builds a link from `ShareLink.path` (`/share#<id>`) that opens only on this computer. The
id travels in the URL fragment, so the browser never sends it in a `Referer` header; the page
reads it and calls the public route itself.

## Storage

`<data_dir>/shares/<id>/share.json` holds the owning chat id, the link settings and the
snapshot; `<id>/<n>` are the file copies (directories 0700, files 0600). A share is written to
`.<id>.tmp` and renamed into place, so a reader never sees half of one.

Revoking fails closed. It first writes a tombstone, `.<id>.revoked`, and marks the id revoked in
memory; only then are the files deleted, with retries, and the tombstone goes last. If the
tombstone cannot be written, revoking answers an error and the link is unchanged. If deleting
fails, the link is already dead: every read checks the tombstone, and a later list or sweep
finishes the deletion. Revoking refuses new reads at once and stops downloads in progress at
the next chunk: every response is checked against the revocation set right before it is
handed over, and a download checks it before and after reading each 64 KiB chunk. A body or
chunk already handed to the connection may still arrive; no lock is held through delivery,
so a slow reader cannot stall revoking. Deleting the chat retires its links the same way;
reads also check that the chat still exists.

A sweeper runs at startup and every hour: it retires links that have expired or whose chat is
gone, and removes staging directories a crash left behind (creation stages under the same lock,
so any it sees are stale).
