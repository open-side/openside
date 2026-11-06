# Session files and screenshot delivery

## Data path

1. `crates/opensided/src/tools/browser.rs` captures PNG bytes and returns model image parts.
   `agent/src/runtime/tools.ts` returns them as `ImageContent` next to the text result for vision;
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

## Private Task output

After registered-secret redaction, Task tool results larger than 32 KiB of serialized JSON
are stored under the session's private `.tool-output` directory. Shell Tasks retain full
stdout/stderr within a 64 MiB capture ceiling instead of discarding everything after 32 KiB.
The worker receives a UTF-8-safe preview of at most 1024 bytes, an opaque `output:<id>`
reference, serialized byte count, and range-read guidance. Screenshot binary parts remain
on the existing image/artifact path; only their textual metadata is eligible.

`read(path="output:<id>", offset=0, limit=4096)` retrieves UTF-8 text using zero-based byte
offsets. Limits are 1–4096 bytes; `next_offset` skips no bytes and should be used for the
next page. Invalid offsets, non-character boundaries, zero/oversize limits, unknown refs,
and cross-session reads fail explicitly. The authenticated session and live Task identity,
not a model-supplied session or filesystem path, authorize retrieval. Tasks receive the
existing daemon `read` schema even when their requested subset omits it, but that implicit
grant covers output refs only. The Ultrabrowse profile likewise exposes only output-ref
reads, not arbitrary file access. No sidecar-local tool or runtime retrieval tool is added.

Files are created 0600 in a 0700 directory using descriptor-relative no-follow I/O.
Total stored output is limited to 64 MiB per session; quota exhaustion and storage failures
return explicit errors rather than claiming a truncated preview is complete. Existing refs
are never evicted to admit new results. Refs survive daemon restart and history compaction,
and are removed with the session artifact directory on deletion (startup retries failed
cleanup). They are excluded from public artifact paths, listings, previews, attachments,
and transcripts. Spill previews and range bodies go only to the worker; persisted Task
progress contains status/size, never the body. The chief receives only the worker's compact
report, not its tool history.

Per-tool security limits still apply before spill: connection responses retain their
64 KiB redacted-response cap, browser snapshot capture/paging semantics are unchanged, and
REPL operation/heap bounds remain enforced. Spill preserves the result available at the
host boundary; it does not bypass those source limits. Redaction covers registered secret
material, not arbitrary secrets embedded in readable content.

Acceptance lives in `opensided::tests::tool_output` (limits, UTF-8 ranges, secret redaction,
session isolation, quota, private permissions, restart, compaction and deletion) and
`crates/openside-e2e/tests/tool_output.rs`. The latter drives a real isolated daemon and
Bun sidecar with a scripted Task: shell output → private ref → ranged read → checked
range → compact chief report, and asserts the durable chief thread contains no body.

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
only from cached CDP resources. Extraction performs no page evaluation or outbound request,
accepts at most 64 KiB of raster source bytes, and returns a decoded, validated PNG no larger
than 32 pixels per side / 8 KiB. Missing cached icons stay absent; clients show the host
initial or a globe rather than fetching an icon.

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
  cache-only route behavior. The browser's `tests/favicon.rs` smoke checks cache-only
  extraction, absence of page evaluation and tab ownership using a temporary headless browser.
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

## Assistant document attachments

`send_message{text,attachments?:[{artifact,display:"inline"|"panel",title?}]}` accepts
at most two Markdown Output artifacts from the current session, including its Tasks.
Each document must be UTF-8, `text/markdown` or a `.md` path, and at most 256 KiB.
Admission reuses `present` secret screening (secret-shaped text, payment-card numbers,
Vault values and cookie redactors) for both body and title. Titles must be nonempty,
at most 160 characters, and contain no control characters.
Denials identify the zero-based `attachment[index]` and include a next-step hint.

Task-side `write_document{title,markdown}` writes
`documents/tasks/<task-id>/<tool-call-id>.md`, emits `artifact.saved`, and returns
`{artifact,title,bytes,content_type}`. It needs no workspace file permissions.
`get_tasks().tasks[].artifacts` preserves these paths independently of the Task's
free-text summary.

An admitted attachment is copied to immutable
`documents/delivered/<sha256>.md` before publication, so later changes to its source
cannot change the bytes the person reads. `assistant.final.attachments` contains
`{path,title,display,bytes,content_type}` only; old events default to an empty list.
The existing authenticated `GET /v1/sessions/{id}/artifacts/{path}` route serves
the body using the chief session ID and the entire percent-encoded artifact path.
No body is embedded in the message event. The attachment is indexed as Output and
is deleted with the session.

## Browser downloads

A file that an agent tab downloads (a click, a redirect, a script) is imported into the
owning session's artifacts. Implementation: `crates/opensided/src/downloads.rs`, the tool in
`tools/browser_downloads.rs`, the transfer ceiling in `openside-browser`
(`Browser::set_download_limit`, `Browser::read_download`).

**Attribution.** Every browser the daemon drives reports Chromium's download events to its
event pump: the shared profile (isolated spaces included), incognito browsers and account
browsers. The pump attributes a download to the session that owns the tab that started it;
a download from a person's own tab belongs to nobody and is not imported.

**Lifecycle.** All steps are published on the download's guid as `tool_call_id`, in this
order:

1. `tool.updated {browser_event:"download.started", url, tab_id}` (URL without credentials,
   query or fragment).
2. `tool.ended {browser_event:"download.finished"}`, `is_error` when Chromium canceled it.
3. On success, `artifact.saved {path, size, mime, tool:"browser_download", tool_call_id:guid}`
   then `tool.updated {browser_event:"download.saved", artifact, file_name, size, mime,
   quarantined}`.
4. Otherwise `tool.updated {browser_event:"download.failed", error}` with `error` one of
   `canceled`, `too_large`, `session_quota`, `policy`, `unavailable` (the session was
   deleted, the browser is gone, or the file could not be read or written).

The web client renders these as one download chip; a running agent also gets a steer notice
that names `browser_downloads`.

**Limits.** One file is kept up to 50 MiB. The browser cancels a transfer as soon as its
announced size or its received bytes pass that ceiling, so an oversized file is never written
out in full; it ends as `too_large`. The downloads one session holds total at most 200 MiB,
counted from the files under its `downloads/` folder under the session's lifecycle gate, so
two concurrent imports cannot both take the remaining space; the one that does not fit ends as
`session_quota`.

**Layout and type.** The file is written once, exclusively and with mode 0600, at
`downloads/<guid>/<name>`. The guid must be ASCII alphanumerics and `-`. `<name>` is the
page-suggested name made into one plain component by `openside_browser::safe_file_name`
(also the REPL `saveAs` default name): separators, characters Windows forbids, control and
bidirectional or zero-width format characters become `_`, leading dots and trailing dots or
spaces are dropped, a Windows device name gets a `_` prefix, and it is cut to 120 bytes
keeping the extension. The MIME type comes from the file's bytes only (the allowlisted
sniffer used for attachments; unrecognised bytes are `application/octet-stream`). The artifact
route serves `downloads/` with that sniffed type, so an `.html` or `.svg` download is never
served or previewed as active content.

**Quarantine.** `quarantined: true` marks a file whose bytes are unrecognised or whose
extension runs code or renders active content when opened (executables, installers, scripts,
HTML, SVG and similar). Such a file is still kept and listed, but clients offer it for
download only and never open or preview it. `GET /v1/sessions/{id}/artifacts` carries the
same flag on the file's entry (`quarantined: true`, omitted when false), re-derived from the
stored name and sniffed type by the helper the importer uses (`downloads::quarantined`), and
lists no `preview_url` for it. The web Files sheet shows such a row with its name, size and
the download-only note but nothing to open, and the file view (`ArtifactPreview`, also used
by the split pane) shows that note and Download in place of Source and the preview frame.

**Policy admission.** Mirroring the browser REPL's `saveAs` gate, the import runs only when
the session's effective policy denies `Browse` on neither the origin of the page whose tab
started the download nor the origin of the file's own URL, and does not deny `FsWrite` on
`<artifacts>/<session>/downloads`. Both origins are judged when both exist; a `blob:` or
`data:` file has no origin of its own, so the starting page alone decides for it. A download
with neither origin (a `data:` file started from `about:blank`) cannot be judged and fails
closed as `policy`. A read-only session therefore browses and downloads but keeps nothing
(`policy`). An `Ask` effect does not stop the import: the file lands in the session's private
artifacts, not in a workspace folder.

**Tool.** `browser_downloads {download_id?, wait_seconds?, read_text?}` (Read tier) lists the
session's recent downloads, newest last (the in-memory record keeps 64 per session), as
`{untrusted: true, authority: "data_only", downloads:[{download_id, url, tab_id, state, …}], pending}`
(URLs, file names and file text come from pages). `state` is `in_progress`
(with `received_bytes` and, once announced, `total_bytes`), `saving`, `saved` (with
`artifact`, `file_name`, `size`, `mime`, `quarantined`) or `failed` (with `error`).
`wait_seconds` (at most 60) waits until the named download, or every listed one, is saved or
failed. `read_text` needs `download_id` and returns up to 16 KiB of a saved `text/plain` file
whose source is at most 4 MiB as `text` with `truncated`, and marks that entry
`untrusted: true, authority: "data_only"` too.
The tool's policy actions are `Browse` on the origins of the downloads it reports and of the
pages that started them. It is part of the `browser-agent` and `ultrabrowse` profiles, and
direct callers use
`openside browser downloads SESSION [download_id=…] [wait_seconds=N] [read_text=true]` or the
read-only MCP tool of the same name.

**Retention and private chats.** The download record is in memory and is dropped when the
session is deleted. Imported files and their `artifact.saved` events live with the session's
artifacts and are removed with them on session deletion. Chromium's own file of a download
(`downloads/<guid>` of the shared, incognito or account profile, or
`space-downloads/<context>/<guid>` of an isolated space) is deleted as soon as the download
settles, whatever the outcome (`saved`, `too_large`, `session_quota`, `policy`, `unavailable`,
`canceled`, or a session deleted meanwhile), before the outcome is published
(`Browser::release_download`). The unlink is descriptor-relative under the profile, never
follows a link (a link swapped in at that name is removed, its target untouched) and only
takes a UUID-shaped name and its `.crdownload` partial. A browser REPL `download.saveAs`
that runs after that copies from the imported artifact instead; for a download the session
did not keep (any failure) it fails with "the download is no longer available". This is
simpler than holding the raw file while a REPL might still claim it, because persistent REPL
contexts keep download objects alive for up to ten idle minutes across calls. Files a crash
left behind are swept by the daemon when it launches or reconnects to a browser (and when an
account browser opens), and every ten minutes after: guid-named files in those folders
older than one hour that belong to no running download are deleted; saved copies in
`downloads/sessions/` and other names are left alone. An incognito profile is also deleted
whole when that browser closes.

**Acceptance evidence.**

- `opensided`: `tests::browser_downloads::agent_downloads_are_imported_listed_quarantined_and_bounded`
  drives real headless Chromium against a loopback fixture: a text attachment is saved as
  `downloads/<guid>/report.txt` (0600, `text/plain`, readable through `read_text`) with the
  lifecycle above in order; a `.sh` file is quarantined; with a 1000-byte transfer ceiling a
  slow 2 MiB stream ends as `too_large` with nothing stored; a read-only session's download
  ends as `policy`; the per-session total refuses a file one byte over and accepts one that
  fits. After the saved, `too_large` and `policy` outcomes Chromium's file is gone from the
  profile, and both `save_download` and a REPL `saveAs` run after the release copy the
  imported file. `private_downloads_leave_no_file_in_the_temporary_profile` shows the same
  for an incognito session. Both skip when no browser is installed.
- `opensided`: `tests::download_gaps` adds the edges: `blob:` and `data:` downloads made by a
  page whose origin is denied end as `policy` and are saved once it is allowed (through the
  daemon's own browser and event pump); a `data:` download from `about:blank` fails closed as
  `policy` under a policy allowing everything; the source origin and the downloads folder are
  denied separately; a raw file grown past the per-file ceiling after Chromium finished ends as
  `too_large`, and one swapped for a link to a file outside the profile ends as `unavailable`
  with the target untouched; a 120-byte device name is saved as a 120-byte `_CON.….pdf`;
  concurrent imports racing another artifact writer fill the per-session ceiling exactly
  (other artifact kinds do not count toward it); the authenticated raw route serves
  `downloads%2F<guid>%2F<name>` with the sniffed type, answers 401 without a token and 404 to
  traversal, and serves an `.html` download as an attachment that is never typed as HTML, with
  the same headers as an uploaded `.html`; the artifact list marks `.sh`, `.html` and
  unrecognised downloads `quarantined` without a `preview_url`, and leaves a `.txt` or `.pdf`
  download and a tool-written `files/install.sh` unmarked and previewable.
  `openside-browser` `tests/events.rs`
  `download_reads_are_bounded_and_never_follow_a_swapped_source` covers `read_download`
  reading at most `max + 1` bytes of a file grown after completion and refusing a swapped-in
  link.
- `openside-browser` `downloads::tests` cover name sanitising (`safe_file_name`), the guid
  shape, the unlink refusing links and non-guid names, and the sweep (old guid files removed;
  fresh, running, non-guid, saved and linked-folder entries kept). `opensided`
  `downloads::tests` cover the quarantine list and the per-session record bound;
  `tests::core_gaps` probes the tool's policy actions. Web: `FilesTray.test.tsx` checks that a
  quarantined entry opens nothing, renders no frame, image or source view, and still downloads.
