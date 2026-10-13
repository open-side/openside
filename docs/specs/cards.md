# Conversation card transport

Cards are presentation, not permission. The canonical Rust types are exported by `openside-proto` from `cards.rs`; clients must use the generated schema rather than defining a second wire model.

## Sources and replay

- `Ticket.card` is a daemon projection of the existing ticket authority. `Ticket.form` holds verified field metadata only.
- `ticket.resolved`, `ticket.rejected`, and `ticket.expired` carry an optional replacement card in the same durable event. Old records without a card deserialize unchanged and are enriched on API replay without rewriting logs.
- Approved deferred operations remain pending until their original `tool.ended` carries the operation outcome and replacement card. Approval itself is not evidence that a payment, send, or other operation succeeded.
- `card.presented` contains a read-only card and its original `tool_call_id`. Its identity is `present:<tool_call_id>`; repeating the same call returns its stored `card_id` without another event or media fetch.
- The single Rust ticket projector lives in `opensided/src/cards.rs`. Terminal asks contain no editable fields or ticket-resolution actions. Historical secure asks without a destination explain that a new secure step is required.

### Person-only metadata

Read scope does not grant an agent access to imported address suggestions or saved-account choices. Proto fields opt into person-only handling with `#[schemars(extend("x-person-only" = <safe replacement>))]`: arrays use `[]`, optional values use `null`. The daemon's single schema-driven outbound projection follows nested schema references, including hydrated historical cards.

Agent-client JSON responses (ticket list/get, transition responses, and event polling) are projected after the handler; SSE replay and live delivery apply the same projection per event. The relay uses the same authenticated router. Person-device tokens and the local Unix socket retain the full view. Projection never edits stored records. Card input suggestions and initial values, operational login suggestions/accounts, and human-facing card choice options are person-only. A new private field needs the proto marker rather than a second endpoint-specific denylist.


## Presentation admission

`present` is a daemon catalog tool exposed in Full, BrowserAgent, and Ultrabrowse profiles, including the front agent's read-only tool list. Its compact model schema is derived from canonical `PresentRequest`, narrows variants and daemon-owned fields, and removes documentation annotations, response-only extensions, and unreachable definitions. The daemon still parses and validates the full canonical type. Input must use an empty id, done status, no interaction, no ticket id, and no favicon references. Only product/place lists, tables, order/payment summaries, form summaries, and connection intents are admitted. Actions may only draft a follow-up or open an HTTP(S) link.

The daemon rejects unknown properties, forged authority, interactive/team blocks, duplicate identities, malformed typed values and table widths, excessive sizes, and secret material. Raw tool arguments are capped at 64 KiB before parsing or diagnostic recording. The diagnostic argument projection contains only an allowlisted kind and numeric counts. Secret detection uses the existing memory secret scanner and known-value redactors; errors identify a rule rather than echoing the offending value.

Requests are bounded to 16 blocks, 8 actions, 30 list items, 12 table columns, 100 table rows, 600 cells, one primary action, one trailing notice, eight image sources, and 24 KiB of string data. Links reject credentials and secret query material; fragments are removed. All strings are plain text, not executable HTML or Markdown templates.

## Media and favicons

Images are materialized by the daemon under Fetch and configured egress policy, with public-address pinning and bounded raster decoding. Persisted references are session-relative artifacts. Failed images retain safe alternative text rather than falling back to remote browser image URLs. Media cannot make private artifacts or working screenshots public.

The unified favicon service reuses owned-tab extraction and validates normalized PNGs, with a bounded memory cache. `GET /v1/favicons/{origin}` requires Read and serves only an existing icon referenced by an accessible non-private card. It never performs a network fetch. Unknown entries return `favicon_unavailable`; clients show an initial tile. Private/isolated icons use validated session-local data references, never a shared global origin entry.

## Page forms and payments

`request_form` accepts only a tab id, an optional reason, and `purpose: form|payment`. It discovers actual page controls, derives standard metadata, and pins top-document and frame destinations. No caller-defined selector, field list, origin, validation script, or secret override is accepted. Unsupported or unpinnable provider controls require takeover rather than fabricated fields.

The existing ticket resolve route accepts `{value:{values:{field_id:string|bool}}}`. All transient strings receive redactors before validation or browser work. The resolver checks field identities, types, required values, lengths, semantic payment/OTP constraints, and live document/frame origins, then persists value-free fill intent before any writes. A partial or interrupted fill is not blindly replayed. Results contain status, field count, and next-step metadata, not values.

**FillOnly:** form resolution fills fields and never clicks Submit, Pay, or Place order. A later consequential submit remains a separately captured Confirm. Every payment field is secret, including name, expiry, and postal code. No PAN/CVC wallet storage is introduced.

## Command-line fallback

The CLI renders canonical Card blocks as bounded plain text, including exact origins, lifecycle status, captured totals, ratings, source links, and image alternative text with session-relative artifact paths. It never downloads card images. Tables wrap rather than losing cell contents at narrow widths. Control characters are removed before text reaches a terminal.

Pending ticket cards are rendered once, without a second tool/request summary. Approval receipts remain pending while the underlying operation continues; a fill-only card never claims that a payment was sent. JSON output forwards the existing canonical redacted events, not an additional CLI-owned Card envelope.

`openside ticket resolve ID` prompts from authoritative page-form metadata; `-` reads one line per field from stdin. Secret, OTP, every login text field (including identifiers), and every payment field use hidden input or a pipe. Private values in argv are refused after fetching the ticket metadata, including raw login JSON. Checkbox answers are sent as booleans, and nonsecret initial values can be retained with a blank answer. Login, choice, saved-account, alternative-sign-in, and authenticator-enrollment flows retain their existing encodings.

Hire and handoff cards include `openside team` navigation/action equivalents and an Open web link when the selected HTTP endpoint establishes a known web origin. A Unix socket or relay alone does not establish a web URL, so no address is guessed. Command hints preserve the selected `--socket`, `--url`, or `--remote` endpoint without including its token. Undo hire uses `openside team undo ID`; it is distinct from normal retirement.

## Conversation behavior and acceptance

Pending asks use a 48-pixel collapsed bubble. Opening it mounts the shared typed fields
and actions; a terminal update removes editable controls and replaces the same stable
Card. Helper-origin asks keep the helper's identity and task attribution in both states.
Read-only results remain visible: products, places, tables and orders preview three rows,
with Show all expanding the remaining rows. Captured totals stay pinned outside clipping.
This is a preview expansion, not the pending-ask bubble interaction.

Suggested hires require an authoritative Hire ticket; explicit person-requested hires do
not open one. Explicit-hire receipts expose typed Undo and Retire rather than a
model-directed message. Approved-proposal receipts expose Retire, not Undo, because the
person already approved the Hire Card. Handoffs project into both participant threads.
From the chief's view, the teammate's sidebar Needs you badge and collapsed CardAttention
dock link to the owning thread without mounting another secret form. Replayed events replace
the original Card identity, including historical records enriched by the Rust projector.

`crates/openside-e2e/tests/cards.rs` exercises the real daemon, scripted Python provider,
CLI and headless web app. Page-dependent cases use loopback fixtures, synthetic accounts,
OTP keys and payment values. No real provider credentials, payments or merchant traffic
are permitted. Existing login, OTP, human-step and web targets retain their end-to-end
assertions using the shared Card DOM.

Run focused acceptance through the workstation's thermal runner:

```sh
/tmp/openside-heavy-run cargo test -p openside-e2e --test cards -j 2 -- --test-threads=1
```

The test harness scans persisted data, event/transcript/tool arguments, audit/log output
and sidecar snapshots for canaries. Browser assertions additionally inspect settled
state and image resource URLs. The intended transient input/request and private local
fixture page are excluded from the canary prohibition.

## Regeneration

After the integration wave, regenerate `docs/specs/openapi.json`, `web/src/api/schema.ts`, and `extension/src/schema.ts` from the Rust schemas. Profile catalog snapshots are generated through the existing `OPENSIDE_UPDATE_PROFILE_SNAPSHOTS=1` test mechanism. Generated TypeScript must not be edited by hand.
