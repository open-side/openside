# Session files and screenshot delivery

## Data path

1. `crates/opensided/src/tools/browser.rs` captures PNG bytes and returns model image parts.
   `agent/src/openside_agent/tools/__init__.py` wraps these in `ToolReturn` content for vision;
   the sidecar's bounded continuation is stored as opaque `agent_state` in
   `crates/openside-sessions/src/store.rs`, not as public message attachments.
2. Only explicit screenshot deliveries call `save_tool_output` in
   `crates/opensided/src/artifacts.rs`. It saves private bytes and appends `artifact.saved`
   through the session event store. Other tool-created files use the existing tracker.
3. `artifacts.rs` reconstructs the file index from events and applies `ArtifactPurpose`
   before serving `GET /v1/sessions/{id}/artifacts`. Direct tool API calls in
   `tools/api_call.rs` expose only persisted content-part references in `response.artifacts`;
   observations have no reference to expose.
4. `web/src/hooks/useSessionFiles.ts` fetches that listing. `web/src/api/artifacts.ts` also
   filters legacy/mixed responses, while `web/src/state/session.ts` filters saved-event
   replay. `FilesTray` uses that data for its count, list, and previews. `Timeline` keeps
   screenshot results in technical tool details rather than rendering them as images.
5. `extension/src/panel.ts` handles user/assistant messages and tickets only. It has no
   screenshot-result renderer or independent outputs counter.

## Classification

Session files carry `purpose: "output" | "working"` separately from their `source`
(`attachment`, `tool`, `visual`, or `transcript`). The artifact listing endpoint returns only
output-classified files. Attachments and transcript exports keep their existing semantics:
an upload is still an attachment, and the Files UI excludes the transcript export.
Generated/modified workspace files, downloads, and deliberate visual blocks remain visible.

`browser_screenshot` accepts `output: boolean` (default `false`):

- Omitted or `false`: a working observation. The vision model receives the image and
  `{saved:false,purpose:"working",width,height}`. No PNG is copied into the session artifact
  directory, no `artifact.saved` event is appended, and the content part has no artifact
  reference. It does not contribute to Files/output counts or inline conversation images.
- `true`: an explicit delivery, used when the person asks for a screenshot or the agent
  deliberately shares one. The original PNG is stored privately at
  `artifacts/<session>/screenshots/<tool_call_id>.png`, with an `artifact.saved` event and an
  image part referencing that relative path. The result is
  `{saved:true,purpose:"output",width,height}`. It is listed, previewable, and downloadable
  like any other generated file.

The screenshot tool retains its existing tab ownership, capability, cancellation, capture
area, secret-fill, and setup-key checks. `output=true` does not bypass capture denials.
The image handed to the model retains the existing 1 MiB downscaling limit.

Direct CLI/MCP calls use the same parameter. `openside browser screenshot SESSION tab_id=T`
is an observation; add `output=true` to deliver it. `--out FILE` explicitly saves a local
copy and therefore opts into `output=true`. Artifact download paths are percent-encoded as
one URL segment, including the `screenshots/` separator.

## Legacy sessions and presentation

Older daemons saved observation screenshots as root-level `<tool_call_id>.png`. Classification
uses tool provenance (`browser_screenshot`) or the root-level `toolu_*.png` filename convention
to hide these records. The new `screenshots/` delivery directory, ordinary `files/` paths,
and `attachments/` paths are not mistaken for legacy observations, even when a name begins
with `toolu_`. There is no destructive migration: existing PNGs and public tool events remain
on disk and continue to be available through the authenticated raw route until session deletion.

The daemon filters the listing. The web API helper applies the same inference when talking
to an older daemon; both Files data and `artifact.saved` replay exclude working images.
Web tool rows remain in the conversation, with their original JSON result inside collapsed
technical details. The extension renders user/assistant messages and tickets, not tool results
or a separate artifacts/output list. Neither surface inlines observation images; technical
thumbnails did not previously exist, so none are added.

## Retention

Working captures live only in the model conversation and the existing private sidecar
continuation checkpoints. They have no separate durable artifact copy. The existing shared
1 MiB snapshot media budget keeps the newest media across parent/child histories and queued
steers, replacing older media with an omission note. Checkpoints retain the existing private
`events.db` lifecycle: deletion tombstones a session and removes its artifact directory but
does not purge the append-only event history or opaque checkpoint row. No new cache,
background pruning task, or retention policy is introduced. Deliberate deliveries and
untouched legacy PNG files follow normal session artifact deletion rules.

## Card media and site icons

Card image sources are materialized in the daemon, never fetched by the web or extension.
The media client checks configured egress and Fetch policy on every hop, pins a checked public
socket address, rejects private/metadata/transition addresses, and follows at most two
revalidated redirects without HTTPS downgrades. It sends no cookies, authorization, referrer
or proxy traffic. Encoded HTTP bodies are refused; identity streams are capped at 2 MiB,
with an 8 MiB request budget and a 10-second fetch-and-decode deadline per source.
Raster signatures must match the declared MIME. Decoding is bounded to 4096 pixels per side
and four million pixels; only a static decoded frame is re-encoded to PNG. Metadata and
active formats do not survive. The resulting `cards/media/<sha256>.png` is an ordinary
private session artifact served through the Read-scoped artifact API and removed with the
session. Reusing an artifact requires its session-local index entry, excludes working
screenshots, and repeats raster normalization. Failed remote images retain their safe
alternative text instead of falling back to a remote URL.

Site icons reuse `Browser::favicon` and `Browser::validated_favicon` for login, page Choice,
phone approval, browser state and Cards. Owned browser tabs may provide same-origin icons
from private sites, but daemon HTTP never receives a private-address exception. Browser
extraction omits credentials and redirects, streams at most 64 KiB, and returns a decoded,
validated PNG no larger than 32 pixels per side / 8 KiB. If unavailable, the only daemon
fallback is the exact HTTPS origin's `/favicon.ico`, under the same egress and Fetch policy.

The favicon cache is memory-only, session-partitioned, LRU, and bounded to 256 entries /
2 MiB (each decoded icon is also at most 8 KiB). Successful entries live for 24 hours;
negative results live for five minutes. Fetch admission prevents duplicate in-flight work.
Incognito and isolated-browser-space cards carry validated data refs, never global cache
entries. `GET /v1/favicons/{origin}` requires Read and never fetches: it serves only a live
cache entry referenced by a persisted non-private Card in an accessible, non-deleted session.
The percent-encoded origin is a single path segment. Responses are PNG with `nosniff` and
`Cache-Control: private,max-age=300`; misses return `404 favicon_unavailable`.

The browser must not turn a missing artifact or icon into a remote image request. Result
cards keep three-row previews with an explicit Show all control; captured totals stay
outside the clipped region. This presentation rule does not change artifact ownership,
authentication, download scope or media retention. Card e2e checks inspect rendered image
URLs and browser resource entries so every network image request belongs to the daemon;
blob/data references are local decoded media, not external fallbacks.

Artifact downloads encode the complete session-relative artifact path as one URL segment
(including `/` as `%2F`). CLI and MCP use the same route as the web client; splitting the
path into URL segments does not match the daemon's artifact endpoint.


## Acceptance evidence

- `openside-proto`: `screenshot_purpose_preserves_deliveries_and_hides_legacy_observations`.
- `opensided`: `artifact_listing_excludes_working_screenshots_without_deleting_legacy_files`;
  existing browser and canvas screenshot tests cover observation metadata and explicit delivery.
- Card media fixtures: `card_media::tests` exercises pinned DNS, rebinding on redirect,
  private/mapped/transition addresses, redirect limits, MIME/signature and size/pixel bounds.
- Favicon fixtures: `favicons::tests` and `tests::favicons` exercise decoded PNG validation,
  LRU/TTL bounds, concurrent positive/negative fetch coalescing, private cache isolation and Read-scoped
  cache-only route behavior. The browser's `tests/favicon.rs` smoke checks credential
  omission, isolated-world extraction and tab ownership using a temporary headless browser.
- Native Unix artifact fixtures retain write/edit/listing/transcript assertions everywhere.
  Their composite shell step must succeed on Linux and assert the credential-safe sandbox
  refusal elsewhere. Credential exclusions exercise the real collector directly against
  fixture filesystem changes, including protected storage and a binary canary; they do not
  require or weaken Linux-only shell isolation.
- Web: `api/artifacts.test.ts`, `components/Timeline.artifacts.test.tsx`, and the Files tray tests.
- Scripted real-daemon/sidecar smoke:
  `openside-e2e/tests/artifacts.rs::a_browser_screenshot_reaches_the_model_as_an_image` captures
  three observations, verifies zero outputs and no artifact copies, then requests one explicit
  screenshot and verifies exactly one listed downloadable output and model image delivery.
