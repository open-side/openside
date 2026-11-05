# openside web design

The web app adopts the design system of [aside-relay](https://github.com/mym0404/aside-relay) (MIT): its tokens, layout metrics, controls and interaction patterns, arranged in an Aside-like restrained shell (sidebar, home composer, plain conversation column).

## Philosophy

- Flat, solid surfaces. The page is `--canvas`; chrome is `--surface` with 1px hairlines. No gradients, no translucency, no backdrop blur anywhere (web, PWA or extension).
- The assistant's words are plain text on the canvas, not a bubble. Only the person's messages sit in a bubble. Content leads; chrome stays quiet.
- Quiet colour. Hierarchy comes from weight, size and spacing; the palette stays neutral and one accent marks links, primary actions and the live state. Send and Stop are monochrome.
- The phone layout is the reference. Desktop widens the canvas and pins the sidebar but changes nothing else.
- Every command lives where the object lives: a queued message has its own menu, a message has its long-press menu, a ticket carries its own buttons.
- Motion is short and only confirms what happened. Under reduced motion nothing moves; overlays only fade.

## Tokens

`src/tokens.css` is the only token source; the extension copies it into its bundle (`extension/build.ts`). Primitives (`src/ui/ui.css`) and screens read only these variables, never raw colours. Dark values apply under `:root[data-theme="dark"]`, and under `prefers-color-scheme: dark` for `data-theme="system"` or no `data-theme` at all (the extension side panel never sets one).

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `--canvas` | `#ffffff` | `#000000` | page, thread, drawer, home |
| `--surface` | `#f7f7f7` | `#141414` | sidebar, composer, code, inset rows, dialog fields |
| `--surface-raised` | `#ffffff` | `#181818` | cards, task cards, tiles |
| `--overlay` | `#ffffff` | `#141414` | dialogs, sheets, menus, toasts, PiP body |
| `--bubble` | `#f1f1f1` | `#181818` | user bubble, chips |
| `--text` | `#171717` | `#f3f3f3` | body text |
| `--muted` | `#666666` | `#999999` | secondary text, section headings, icons |
| `--faint` | `#8e8e93` | `#6e6e73` | placeholders, timestamps |
| `--border` | `#e9e9e9` | `#343434` | hairlines, the tool-activity timeline |
| `--control-border` | `#dcdcdc` | `#414141` | inputs, outline buttons, sheet handle |
| `--hover` | `#ededed` | `#303030` | row and control hover |
| `--active` | `#e4e4e4` | `#3a3a3a` | selected row (`aria-current`), pressed menu row |
| `--primary` / `--primary-foreground` / `--primary-hover` | `#2f6fed` / white / mixed | same | links, primary buttons, focus ring |
| `--live` | `--primary` | `--primary` | the running dot |
| `--send-bg` / `--send-fg` | `#171717` / `#ffffff` | `#f3f3f3` / `#000000` | composer Send and Stop |
| `--success` / `--warn` / `--danger` | `#067647` / `#b54708` / `#b42318` | `#47cd89` / `#fdb022` / `#ff9b91` | status text and icons |
| `--danger-surface` | `#fef3f2` | `#2a1513` | error rows |
| `--scrim` | black 40% | black 60% | dialog and drawer backdrop |
| `--shadow-sm` | `0 1px 2px` black 6% | none | composer, hover card |
| `--shadow-overlay` | `0 12px 32px` black 12% + `0 2px 6px` black 6% | `0 12px 32px` black 60% + a 1px `#343434` ring | dialogs, menus, PiP |
| `--hl-*` | | | code highlight colours |

Palette presets (`[data-light-palette]`, `[data-dark-palette]`) supply canvas, text and accent; every other surface is derived with `color-mix` (raised and overlay surfaces are the canvas in light and the surface in dark, `--send-bg`/`--send-fg` are text/canvas).

Scale (both themes):

- **Radius:** `--radius-xs` 4, `--radius-sm` 8 (rows, tiles; `--radius-row`), `--radius-md` 12 (inputs, tool details), `--radius-lg` 16 (cards, code, PiP, menus; `--radius-card`), `--radius-bubble` 24, `--radius-composer` 28, `--radius-dialog` 28, `--radius-pill` 9999. Components never use raw px radii.
- **Space:** `--space-1` 4, `-2` 8, `-3` 12, `-4` 16, `-5` 20, `-6` 24, `-8` 32, `-12` 48.
- **Type:** `--text-xs` 12 (section heads, meta), `--text-sm` 13 (labels, steps, status), `--text-md` 14 (rows, card body), `--text-base` 16 (body, composer, inputs; never smaller on inputs, so iOS never zooms), `--text-lg` 19 (header title, weight 650), `--text-xl` 24 (home title), `--text-2xl` 32 (onboarding headline, 700, 1.15). Leading: `--leading-body` 1.6, `--leading-user` 1.55, `--leading-code` 1.4, `--leading-tight` 1.15. Weights: `--weight-medium` 500, `--weight-semibold` 600, `--weight-title` 650, `--weight-bold` 700.
- **Layout:** `--header-height` 56 (64 below 821px), `--sidebar-width` 272, `--content-width` 736, `--composer-width` 768, `--home-width` 720, `--row-height` 36 (44 under `pointer: coarse`), `--drawer-width` `min(85vw, 340px)`.
- **Motion:** `--fast` 140ms, `--normal` 220ms, `--slow` 320ms (sheets, drawer), `--ease` `cubic-bezier(0.2, 0, 0, 1)`, `--ease-out` `cubic-bezier(0.16, 1, 0.3, 1)`.

## Typography

- UI and content use Wanted Sans Variable; code uses D2Coding. Both fonts are bundled under `src/assets/fonts`, and nothing loads from the network.
- Body and assistant text are 16px at 1.6; the person's bubble is 16px at 1.55; code is 13px at 1.4.
- The header title is 19px at weight 650 on desktop and 16px at 600 on phones. Section headings (`.section-heading`) are 12px muted at weight 600.
- Brand: the mark (`BrandMark` in `src/ui`, drawn in `currentColor` so it follows `--text` in both themes) then the word "openside" at weight 650. The app icon `public/icons/icon.svg` is the same mark in `#f4f3ee` on a `#111318` rounded tile; `icon-maskable-*.png` use a full-bleed tile with the mark inside the maskable safe zone. The wordmark for docs is `docs/assets/logo.svg` (`logo-dark.svg` for dark backgrounds): the mark plus "openside" set in Lexend (OFL-1.1) at weight 450, converted to paths.

## Layout

```
.shell[data-sidebar=open|collapsed]
 ├─ aside.shell-sidebar (≥ 821px; a Drawer named "Team menu" below that)
 └─ .shell-content (--canvas)
      ├─ header.app-header (anchored, solid)
      ├─ .conversation-scroll > .conversation-column (736px)   | home: .home-column (720px)
      └─ .composer-anchor > .composer (768px)
```

| Metric | Value |
| --- | --- |
| Shell | ≥ 821px: a grid of `--sidebar-width` and the content; collapsed (`localStorage["openside.sidebar"] = "collapsed"`) hides the sidebar. Below 821px one column and the sidebar only in the drawer. No backgrounds, gradients or shadows on the shell |
| Sidebar | `--surface` with a 1px right hairline |
| Header | `--header-height`, `--canvas`, a bottom hairline on phones only: the drawer or sidebar button and the heading on the start side, plain icon buttons on the end side (no pill wrapper) |
| Content column | 736px |
| Composer | 768px, `--surface`, radius 28, `--shadow-sm`, docked at the bottom of the content. Send and Stop are 36px monochrome circles (`--send-bg`/`--send-fg`). Stop asks "Stop everything?" (Keep running / Stop everything) before stopping the agent and every running Task; ✕ on one Task's card asks "Stop this task?" (Keep running / Stop task) |
| Phone sheet | dialogs and select lists become bottom sheets below 821px |

- Sidebar, from top to bottom: brand (`BrandMark` and "openside") with Collapse sidebar (desktop) or Close menu (drawer); "New task", which goes to the home composer; Pinned, the synced pinned tabs as 40px favicon tiles on `--surface-raised` (up to six, opening in a new browser tab; hidden when there are none); Chats, the people list (the assistant first, then teammates); Tabs, the agent's own browser tabs, each opening its conversation with the browser sheet; then History, Workspace and Settings at the bottom.
- Person rows are one 36px line (`--row-height`) at radius 8: a 24px avatar, the name at 14/500, and on the right a `.live-dot` while the person works, the "Needs you" badge or an `.unread-dot`. The live activity is in the row's `title`. Unread is per device (`state/seen.ts`): the first roster a device sees counts as read, and the open thread never shows unread. The open row is `--active`. Every label truncates with an ellipsis and keeps its full text in `title`; nothing in the sidebar scrolls sideways (`crates/openside-e2e/tests/web_overflow.rs`).
- Home (`/`): a centred column (`--home-width`) with "What should we do?" (24/650), the home composer ("Ask AI a task, @ for context"), a model chip, and Recent tasks: the assistant's background Tasks as cards (`--surface-raised`, hairline, radius 16) with their status, the latest browser step and an authenticated thumbnail when one exists. Sending hands the task to the assistant's thread (`/agents/chief`).
- A person's thread (`/agents/:id`): the header holds the avatar, the name and a subtitle (Needs you, the live activity, or the role), and opens the profile. An empty assistant thread shows the avatar, "Hi, I'm <name>" and the starters above the composer, whose placeholder names the person. A retired teammate's thread is read only, with a Restore note in place of the composer.
- Profile sheet: a wide dialog (a bottom sheet on phones) with identity first (64px avatar preview, name, initials, colour swatches with a ring on the chosen one), then role, voice, responsibilities and boundaries, then Model, Permissions and reach, Skills and Routines and memory sections divided by hairlines. Save sends only what changed; Retire is a danger button at the end and opens the hand-over or pause choice.
- History (`/history`): a 40px search field on `--surface` (radius 12), 44px rows, Earlier/Archived as chips, the row menu; retired teammates use person rows. Workspace → Team lists everyone as resource rows with their avatar.
- Ordinary conversation menus offer Delete conversation after a confirmation describing the irreversible removal and file deletion; success refreshes History and returns an open deleted thread there. Archive remains reversible. Agent home threads, including the chief's, offer neither Delete nor Clear. `docs/specs/app-help.md` is the static app capability contract supplied to the front; focused tests compare its route and menu names with the web UI. Conversation titles are preserved and no session model labels are added.
- The static capability map is ≤1.5KB and lives only in the front/chief instructions, never worker profile prompts. Broken-output reports are acknowledged without speculative rendering diagnoses; the front offers a real document attachment or native result card. Discovered sign-ins are not called verified connections, and authentication status never implies a grant to every agent.
- When the daemon cannot be reached, the shell shows one `.connection-banner` ("Can’t reach openside. Reconnecting…") instead of per-screen errors.

## Conversation

- Messages: assistant replies are plain text on the canvas (no background, border or padding), 16/1.6, the full column width. The person's messages are right-aligned `--bubble` bubbles (radius 24, padding 10×16) at most 80% wide. 24px between turns. No avatars.
- Message menu: long-press or right-click a message to copy it or quote it into the composer; a queued message also offers Edit.
- Failures: a `.turn-error` row (a `--danger` icon, the text in `--text`, a "Technical details" disclosure and an outline Retry pill).
- Scrolling: the transcript is the only scroller on the conversation route (the page, shell and header never scroll) and uses `overscroll-behavior: contain`. Inner scrollers opt into `.thin-scroll`: no scrollbar track ever; a thin pill thumb stays invisible until the reader scrolls (`data-scrolling`, set for 0.9s by `ui/scrollReveal.ts` after a wheel, touch, key or pointer input, never by the transcript pinning itself) or hovers, and touch devices keep their native overlay bar. In the transcript a reserved gutter keeps the column centred on the composer. The transcript follows a streaming turn only while the reader is at the bottom: the first upward move stops it (`pages/scrollFollow.ts`), coming back within 80px resumes it, and content that grows without an event (images, cards) keeps the end in view while following. Otherwise the jump-to-latest button shows and nothing moves under the reader. A new conversation always opens on its latest turn; scroll position is not restored on reload.
- Delivery: the person's newest message carries a small note under it, as a messenger does: "Sending…" until the daemon takes the POST, "Sent" once accepted (an idle session starts on it at once; a message sent during a run stays in the Queued list instead), "Read" once the agent has it (`message.injected`); "Read" stays under the newest message after the answer. When the agent starts on it, three dots bounce one after another (a slow fade under reduced motion, with `aria-label` "Agent is working") until the first text or tool row arrives. A POST that fails shows "Not sent · <reason>" with Retry (same `message_id`, so the daemon deduplicates) and Discard; a run that died after reading the message shows "Not finished", with Retry only when no error row follows. The state is derived from the event log (`message.queued` / `message.injected` / `turn.started`) plus this device's own in-flight sends (`state/outgoing.ts`), so a reload shows the same thing.
- Time: a centred muted line ("Today 14:05", "Yesterday 9:12", a weekday this week, a date before that; `components/timeStamp.ts`) goes before the first message and before any message that starts a new day or follows an hour's pause. No divider rules.
- The assistant's thread (front agent, `Timeline front`): the assistant only talks; its work runs in background Tasks. Each `send_message` arrives whole as one reply (nothing streams), and the three typing dots show while a turn is open (`turn.started` until `turn.ended`). No tool activity and no Task reports (`task-report-*` messages) appear in it. A document shows only when the assistant attaches it to a message.
- Attached documents (`components/MessageDocument.tsx`, `assistant.final.attachments`): Markdown documents sit at the top of the message that carries them. `inline` ones render in place on `--surface`, clipped at about twelve lines with a fade and "Show more"/"Show less"; `panel` ones are a card with the title and three faded lines that opens beside the thread on desktop (a full-height right panel) or as a full sheet on a phone, with Copy and Close.
- Visual blocks (```` ```html visual ````/```` ```svg visual ````, `components/VisualFrame.tsx`): drawn as inert images (`components/visualImage.ts`), never frames: SVG as is, HTML laid out as XHTML in an SVG `<foreignObject>`, scripts and external loads stripped. They are transparent, take the page's text colour and the system font as defaults, fit the column (no inner scrollbars) and are as tall as what they paint (HTML is measured by drawing it once; content sized to the viewport gets a 16:10 page). A visual that cannot be drawn says so plainly, and Source shows its code either way.
- Tool activity (other threads): a summary row (a chevron and a phrase from `toolCopy`) folds the run of calls. Open, its steps hang on a 1px `--border` line with 22px icon nodes on the canvas; each row is at least 32px, the title at 14px and the state on the right at 13px muted (a spinner while running, `--danger` on failure). Details open on `--surface` at radius 12 with 13px code. Task bookkeeping calls (`create_task`, `steer_task`, `cancel_task`, `get_tasks`, `get_models`, `send_message`) never show.
- Browser: the in-thread browser card is a `--surface-raised` card (radius 16) with a 16:10 screen and a footer line (favicon or host, the current action in 13px muted, Minimize and Close). The picture-in-picture window (`--overlay`, `--shadow-overlay`, radius 16; 280px wide on desktop, 200px on phones) stays available after the run, remembers its position, minimises to a 48px pill and is summoned from the header's "Show browser" button. On phones the browser opens as a read-only bottom sheet.
- Tasks (`components/TaskUnit.tsx`): each Task is a `--surface-raised` card directly below the message that started it, showing its status, the current step and ✕ to stop it.

## Controls

- Icons come from Lucide at stroke 1.8 (`ICON_STROKE`). Icon buttons are 36px circles (44px under a finger); ghost by default, outlined on `--control-border` where needed. `tone="primary"` is the monochrome send circle.
- Buttons (`Button`) come in primary (the accent pill), secondary (an outline pill on `--control-border`), ghost and danger (`--danger` text). They are 40px tall, 32px when compact (with a 44px hit area under a finger). Pressing scales them to 0.97 (icon buttons to 0.9).
- Chips (`Chip`, `.ui-chip`): 32px pills on `--bubble` at 13px for the model picker, filters and context tags. With `onClick` a chip is a button (`aria-pressed` for toggles); without one it is a plain label.
- Floating surfaces (dialogs, menus, toasts, select lists, the skill-mention list, docks above the composer) all take one class, `.surface-overlay`: solid `--overlay`, a 1px `--border`, `--shadow-overlay`, radius 16. New popovers add the class; they never copy a background.
- Menus (`PopoverMenu`) have a muted title, 40px items (44px under a finger) at radius 10 (menu radius 16 minus its 6px padding), a quiet leading icon, and danger items in `--danger`. A highlighted row is `--hover`, a pressed row `--active`, and keyboard focus adds a 2px accent ring; separators are `--border` hairlines.
- Select (`ui/Select.tsx`) replaces every native `<select>`: a trigger button plus a `.surface-overlay` listbox whose rows reuse the menu item styles. It supports icons or swatches per option, groups, disabled options, a check on the selected row, arrow/Home/End/type-ahead/Enter/Escape, and becomes a bottom sheet with 48px rows below 821px. `variant="field"` fills its container like an input; `variant="inline"` is a compact trigger for rows. Give it an `id` and point a `<label htmlFor>` at it. The extension side panel has the same control for its conversation picker (`extension/src/picker.ts`), and login cards get an editable suggestion list (`ui/suggest.ts`) instead of `<datalist>`; never use a native `<select>` or `<datalist>`, whose popups CSS cannot style.
  - Queued message: Edit message / Send now / Cancel message.
  - Conversation row: Pin / Rename / Archive.
- Dialogs (`Dialog`) are centred on desktop: 440px, or 880px with `size="wide"`, radius 28 on `--overlay` over the `--scrim`. Below 821px they become bottom sheets (at most 92dvh, top radius 28, a `.sheet-handle`, safe-area bottom padding); `back` adds a leading chevron. Fields inside a dialog sit on `--surface`.
- The drawer (`Drawer`, the phone team menu) slides in from the left at `--drawer-width`, full height on `--canvas` with a right hairline.
- Toasts stack at the top centre, at most three, on `--overlay` with a green or red status icon.
- `.section-heading`, `.live-dot` (an 8px `--live` dot pulsing every 1.6s, static under reduced motion) and `.sheet-handle` (a 36×4 `--control-border` pill) are shared classes in `ui/ui.css`.
- Inputs are 44px tall at radius 12 on `--canvas` with a `--control-border` outline and 16px text.
- Segmented controls (`Segmented`) set the selected segment on a raised surface.
- Switches (`Switch`) are `role="switch"` checkboxes for on/off settings that apply immediately: a 36×22 track in a 44px hit box, the accent only when on.

## Conversation cards

Every question, approval, code, login, choice, payment, form, takeover, hire, handoff, activity row and presented result (products, places, tables, orders) is one **card**: a daemon-built `Card` (`Ticket.card`, `card.presented`, `tool.ended.card`, team events) of typed blocks and typed actions. One plain-DOM renderer, `src/cards/` (`render.ts` mounts it, `blocks.ts` draws blocks, `fields.ts` inputs, `actions.ts` dispatches actions, `copy.ts` owns the standard English labels, `cards.css` the look), is mounted by `components/ConversationCard.tsx` in the web app and imported as is by the extension side panel. There is no second form renderer, legacy ticket card, login form or code sheet; the timeline shows a card where it used to show a ticket row and the tool rows the card now represents.

- **Surface.** `--surface-raised` with a 1px `--border` hairline, radius 16 (`--radius-card`), padding 16; title 15/650, field labels 13/500 `--muted`, body 14/1.5. 424px wide (512px for result tables and lists), full width below 480px. The primary action is the accent pill (`--primary`), secondary actions are outline pills on `--control-border`, destructive ones `--danger` text. Result cards (tables, products, places) are flat with hairlines. Targets are 44px; inputs are 16px on phones so the page never zooms.
- **Collapsed bubble.** A pending ask rests as a 44px row on `--bubble`: the site icon, a one-line safe summary (never a field value) and a chevron. It expands in place on click or Enter/Space, with a short size-and-fade morph (instant under reduced motion), moves focus to the first field and collapses on Escape or the close button. Collapsing is not rejecting; typed secrets are emptied. Fields are only built once the card is open. A settled card is a receipt pill with a status mark (done, failed, expired) and a polite live announcement; it holds no field and no action. Presented results (lists, tables, orders) are always open; long ones preview three rows behind "Show all N".
- **Icon and origin.** The site mark is the favicon the daemon normalized to a `data:image/png` URL, or the authenticated icon route for exactly the card's own origin; any other source falls back to the initial of the host. The verified-origin line (scheme, punycode host, non-default port; mono, wrapped, never ellipsised) is the daemon's, shown on login, code and payment cards; a page's own claimed name is never presented as identity. Images (products, places, tables, orders) load only from the session's `cards/media/*` artifacts through the authenticated artifact route and are raster only.
- **Fields.** Labels and `autocomplete` tokens for standard fields come from the client (`username`, `current-password`, `one-time-code`, `cc-number`, `cc-exp`, `cc-csc`), so a model never names them. Secret fields carry `data-feedback-redact`, never a `value` attribute and never an initial value. Passwords get an eye toggle; card number, expiry and CVC format as typed (expiry and CVC share one row); a field whose destination differs from the card's origin says so beside the field; text fields may offer saved suggestions. Enter in a field presses the primary action once.
- **Secrets are never echoed.** Values are read once, every field is emptied, and only then is the request awaited; a refusal, a settle elsewhere, Escape, collapse and dispose all leave the fields empty. The one restore is a 403 on saving login details (the person may lack approval rights): the details return, saving is refused in words, and nothing is sent again until the person presses the action. Chat text, receipts and the text fallback (`cardText`) show `[secure input]`, not values.
- **Code, authenticator.** A code card is one labelled one-time-code input (separators the site prints are dropped; emailed letters kept). When the daemon allows registering, a collapsed "Save this site's authenticator" section takes a masked setup key or `otpauth://` link: with "Save to vault" on it sends `{totp}`, off it computes one code locally and sends only that. A 403 keeps the form, says saving needs approval rights, and fills just this code from the key already pasted. It is hidden on a device without the approve scope.
- **Login, choice.** Login fields mirror the page; other sign-in options are labelled 48px provider buttons with locally drawn marks (`glyphs.ts`); saved accounts and page options are rows. A choice with an action runs it on click; plain options are radios with one primary action.
- **Result cards.** Lists show thumbnail, name, facts, price and rating; money and ratings are integers formatted client-side. Tables have real column headers, a focusable scroll region, right-aligned numbers and an optional per-row site mark. A total is pinned below the clipped body. Links open with `noopener,noreferrer` and only http(s). Notices are inline or a bubble beside the card and carry their tone as an attribute.
- **People.** Hire and handoff cards pair avatars (`avatar avatar--32 avatar--<tone>`; the daemon's colour is mapped to an allowlisted tone, never written into a style).
- **Where they appear.** In the conversation, in place; a pending ask is moved to the end of the thread so the newest thing on screen is the thing that needs the person. Off-thread attention requests (a code, a login, a choice, a phone approval or an authenticator offer) dock under the header (`components/CardAttention.tsx`): one `ConversationCard` plus a link to its conversation, with the dock's height reserved in the scroller. A tapped push (`?ticket=`) opens the full-screen `components/CardSheet.tsx`, which hosts the same card with `layout="sheet"` and closes when it settles. The extension panel lists pending requests of its tab's conversations and any attention request, open by default, and shows cards that arrive as `card.presented`; actions the panel cannot perform (remote screen, in-app navigation) are not offered.
- **Actions the host owns.** `ticket_resolve`, `ticket_reject`, `ticket_done` and `link` are handled by the card. `follow_up`, `open_thread`, `open_ticket`, `open_connection`, `team_command`, `takeover` and an edited `hire` go to the host (thread view, team actions, takeover view); a target nobody handles shows a plain error instead of doing nothing.

## Workspace

Workspace and Settings open as wide dialogs (`.ui-dialog--wide` on `.surface-overlay`, radius 28): a 200px section nav on the left and the section body on the right on desktop, grouped list rows that drill into the same routes on phones. Their sections (`src/pages`, styled in `pages.css`) share one list pattern built from `pages/blocks.tsx`:

- **Grouped lists.** Rows of a list sit in one `.settings-group` (`--surface-raised`, 1px `--border`, radius 16, 44px rows) with hairline dividers, not as separate cards. Small tints (tags, code) mix `--text` over transparent so they work on any surface. A section header carries a muted summary (count, mode) on the left and at most one compact add button on the right.
- **Rows.** `ResourceCard`: title, one muted detail line, optional leading icon in a neutral tile. Row actions go in a trailing "…" `RowMenu` ("Options for <name>"), the same menu as conversation rows; destructive items are red and always confirm. On/off state is a trailing `Switch`, never a pair of Enable/Disable buttons. Expandable history uses `Disclosure`.
- **Badges carry state only:** neutral (read-only, kind), success (connected, unlocked, completed), attention (paused, rate limited, unavailable), danger (failed, denied, rejected). Never the accent colour.
- **Human text.** Schedules, run states, dates and policies are worded for people (`pages/format.ts`: "Weekdays at 08:00 · Asia/Seoul", "in 2 hours"); raw cron expressions and enum values only appear where a person types them.
- **Loading, errors and scope.** `ResourceView` shows a spinner, a load error with "Try again", or, on 403, `ScopeNotice` naming `openside pair --scope admin`; add buttons are hidden when the device lacks the scope.
- **Configuration** renders each schema key as a row: key and help on the left, control on the right (full width for text, lists and JSON). The save bar appears only while there are unsaved changes.
- **Operator screens** (`/agents`, `/sync`, `/policy`, `/diagnostics`, plus the Search model card on Memory) follow the same list pattern: sections with a muted lead, rows in one group, one primary button per section. Anything that needs the daemon to restart shows `RestartNotice` (Restart daemon, confirm, reconnect when `boot_id` changes). Writes that need local authority show `LocalOnlyNotice` when refused. Progress (the model download) is a native `<progress>`, never an inline style. Secrets (sync passphrase and password) are read from the form at submit time and cleared from the inputs.
- **Connections** (`/connections`, next to Vault; `pages/ConnectionsPage.tsx`, `connections.css`) lists the sites the agent browser is signed in to, one `ResourceCard` per account. The leading tile is the site's favicon (a bounded `data:` URL, never fetched from the web; the site's initial otherwise). The title row is the site plus a state badge (Active, Sign in again, Expired, Not checked), the muted line is the masked account or "Account not identified yet", then a meta line (last use by whom, how it was signed in), an avatar stack with a coloured dot per agent (allowed, ask first, blocked) that opens the access dialog, and one visible button only when the sign-in needs attention ("Sign in again", or "Check now" for imported sites). "Sign in again" finishes silently when the Vault can, otherwise it leaves for the conversation's `?ticket=` sheet. The "…" menu holds Check now, Sign in again, Agent access, Activity, Add another account, and the red Sign out / Forget, each behind a confirmation; a card in a separate browser profile offers one "Remove account" instead, because removing it deletes the profile. Sites the cookie sweep found but nobody verified sit under a collapsed "Possibly signed in" disclosure. The Activity list below filters by connection, agent and action. The teammate profile sheet reuses `AgentConnections`: one access select per site, saved at once.
- **Phones:** every control in these sections is at least 44px tall at ≤ 820px, and nothing scrolls sideways at 390px.
- **Sign-in and pairing** (`TokenEntry`, `/pair`) are one centered card on `--canvas` (`--surface-raised`, hairline, radius 24) with a 44px input and the accent primary pill, the same card language as the in-chat login cards.
- **Connect your phone** (`remote/ConnectPhone.tsx`; the first section of Remote access and the guide's phone step) is one glance: opening it has already issued a code through the best connection that is on (the daemon's preferred one), so the QR code (always black on white, `--radius-lg` tile) is the first thing on screen, then one status line ("Waiting for your phone…", "Connected as <name>", "This code expired."), the pairing link with Copy, and "Expires in m:ss" with a New code button. On a wide card the code sits left of its status; on phones it stacks and fits 390×844 without scrolling. "Through <connection>" with Change connection (only when more than one is on) and the collapsed "Phone name and permissions" are secondary controls under it, never a step before the code. While no connection can carry a code the same tile shows its state instead of a QR that would lead to a 404 ("The tunnel isn't reachable yet. Retrying…", the daemon's reason with Try again, or one button that turns on the default connection: Tailscale where installed, a saved relay, otherwise a Cloudflare quick tunnel).

## Models & accounts

`/models` opens as a wide dialog over the current screen, from Settings or the model picker's "Models & accounts" button (`models/ModelsPanel.tsx`; the first-run guide's Models step reuses it compact). Top to bottom: a warning callout while no model is ready, a Models block with two roles chosen once for the whole workspace and never per conversation or message: **Fast model** ("Answers quickly and hands bigger work to the reasoning model"; the first option is "Same as reasoning model", i.e. `fast_model` null) and **Reasoning model** ("Does the thinking for tasks and teammates"; the wire field stays `default_model`), each a select whose first group, "Recommended", holds the catalog models flagged `recommended_for` that role (labelled with their provider) followed by the remaining models grouped by provider, unavailable ones disabled and labelled; every option carries its tier (Fast, Reasoning, Frontier) as a small pill after the name and the curated strengths as a muted second line, and an id configured outside the current catalog says "Not in current catalog"; ordered fallback chips sit under the reasoning model. Every save sends both roles plus the fallbacks, so changing one never clears another. The first-run Models step stays compact and has no role pickers: the daemon pre-selects the recommended pair and Settings changes it later, then one section per provider from `GET /v1/settings/models` (Anthropic, OpenAI, Google). A provider holds any number of accounts, listed as `ResourceCard` rows: API keys (name, masked `••••` + last four; the value is typed in a password field in a dialog and never rendered after saving), Claude Code / Codex CLI subscription accounts (each with its own sign-in home; adding one ends in a dialog that shows the exact sign-in command and polls until it lands) and accounts configured elsewhere (broker, env), with status badge, one meter per usage window showing what is left of it ("18% left · resets in 2 hours"; the bar shrinks as it is used, and turns amber at 30% and red at 10% left; windows without a reading say "usage unknown"), an on/off switch and a Manage dialog (name, keep-in-reserve %, replace key, remove with confirmation). Under the accounts, a Rotation list is the ordered set of accounts that may serve the provider: reorder with the arrow buttons, remove, or add an account from the select; a new account joins on its own only when it is the provider's first, otherwise the add dialog offers it. Then the provider's models (four, "Show all" for the rest). A limited device keeps the same page with a note, no controls and no key metadata. The conversation menu's "Model account…" dialog (`models/SessionAccountDialog.tsx`) prefers one rotation account for the next messages. Credential sources close the page. Remove and key-replace copy says plainly that later requests stop using the account and that nothing is revoked with the provider.

## Channels

`/channels` opens as a wide dialog from Settings (`channels/ChannelsPanel.tsx`, `channels.css`). One `SectionList` per service (Telegram, Slack, Discord), each a grouped list of `ResourceCard` rows with its own "Add account"; an empty service says so in one line. A row is the service icon tile, the account name, a state badge (Connected, Off, Sign in again, Unavailable, Vault locked, Restart needed), the account id, then a line with the teammate that answers (24px avatar and name, from the team roster; "Will answer as … once it connects" while the account is not live). Slack and Discord rows carry the signed callback address (mono, wrapped) with a Copy button; when it points at this computer a note offers Remote access instead of pretending the platform can reach it. Accounts that still read config.toml say so and offer "Move to vault". Trailing: a `Switch`, Edit (Reconnect when the credentials were refused) and the "…" menu (Test connection, Edit account, Remove). Test results are one sentence from the client's `MESSAGES`, never upstream text. Remove confirms and names only that account. Devices without the admin scope see rows and a note but no controls; a pasted token's scopes are unknown, so the daemon decides and its refusal is shown. `RestartNotice` appears when an account needs the daemon to restart.

The account dialog (`AccountDialog.tsx`) holds name, id (new accounts only, derived from the name until edited), the teammate select with avatars, the allow-lists, the Discord application key, the Telegram poll timeout, credentials, the callback address and the on/off switch. Credentials are uncontrolled password inputs read with `FormData` at submit, emptied before the request, and never held in React state; an existing account never shows them and replaces them only through an explicit "Replace credentials". `options` replaces the whole object on the daemon, so the dialog sends the existing options with its edits, which keeps the file-only `api_base` and `listen`.

## First-run guide

`/welcome` (`src/onboarding`, styled in `onboarding.css`) is a split card centred on `--canvas` (at 1024px and wider: at most 1040×640, radius 24, a 1px `--border`, `--shadow-overlay`); the shell hides the conversation list while it is open (`.shell[data-focus]`). The left column (440px, padding 40) holds the brand, the step headline (32/700/1.15), one short muted line, the step controls, the primary full-width `Continue` pill and the secondary text buttons (`Skip this step`, `Skip setup`), with progress as 4px segments. The right column is a decorative miniature of the openside shell built from tokens (`aria-hidden`): sidebar skeleton rows, the home composer and two skeleton task cards, with no invented names, numbers or images. Below 1024px the card is one column without the preview.

- **Preview before write.** `Import` stays disabled until a dry run of the same profile and categories has been shown; any change to either drops the preview. The result and preview use the same count list.
- **Colour carries state only:** success ticks in the recap and the paired-phone line, nothing else is tinted.

## Motion

- Sheets, drawers, backdrops, menus and toasts animate through CSS transitions keyed on Base UI's `data-starting-style` and `data-ending-style`: dialogs fade and scale over `--normal`, sheets and the drawer slide over `--slow` with `--ease-out`. The jump-to-latest button scales in.
- A phone sheet closes when its handle is dragged more than 96px down or flicked faster than 700px/s, as in aside-relay. A shorter drag springs back. Only the handle (and the sheet header) starts a drag, so scrolling the content never closes the sheet.
- `prefers-reduced-motion: reduce`:
  - `--normal` and `--slow` drop to `--fast`; overlays keep only an opacity fade, no transforms;
  - press scale, the live-dot pulse and the drag gesture turn off;
  - scrolling to the latest message jumps instead of scrolling smoothly.

## Appearance

The Settings dialog (`components/AppearanceSettings.tsx`, styled in `pages/pages.css`) has the following rows. Every choice is stored in this browser only:

- **Theme mode:** System, Light or Dark.
- **Light theme / Dark theme:** `openside` (the tokens above) or one of the aside-relay palette presets in `src/theme-presets.ts`. A preset supplies canvas, text and accent; every other surface is derived from those with `color-mix`.
- **Primary color:** a colour picker plus preset swatches. A custom accent overrides the palette accent until you press "Theme default".

`public/theme-boot.js` applies the stored choices before first paint. CSP forbids inline scripts, so this file is static. It also keeps the `theme-color` meta tag in step with the canvas.

## Deviations from aside-relay

- **Motion:** CSS transitions instead of the `motion` library. Base UI already exposes enter and exit states, and the bundle stays smaller.
- **Fonts:** no web-font picker and no text-size setting, because the egress policy forbids runtime font downloads. Only the two bundled families are offered.
- **Default accent:** blue (`#2f6fed`) instead of aside-relay's purple. Swatches and presets cover the rest. Send and Stop stay monochrome (`--send-bg`); the accent is for links, primary actions and the live dot.
- **Text selection:** no global `user-select: none`. Agent output, code and tickets must stay copyable.
- **Markdown images:** shown as an alt-text pill, never as an `<img>`. All network traffic goes through `src/api` with the device token, and CSP limits `img-src`. An absolute http(s) image becomes a link the reader can open; relative and daemon URLs stay plain text. Attachments and files render through the authenticated artifact path instead.
- **Menus:** sign out lives in Settings. The sidebar bottom links to History, Workspace and Settings, because openside has daemon management where aside-relay has its browser panel.
- **Home and "New task":** `/` is a home composer that hands the task to the assistant's continuous thread, so the sidebar says "New task", never "New chat".
- **No fake browser chrome:** openside runs inside the person's own browser and has no navigate endpoint, so there are no traffic lights or omnibox; the browser sheet is read only (taking over goes through a takeover ticket).
- **Pinned and Tabs:** Pinned shows synced pinned tabs; Tabs shows only agent and external tabs that belong to a conversation, never the person's own tabs.
- **No durations or timestamps on tasks,** and no date groups in the sidebar: the session and Task APIs carry no timestamps.
- **Message menu:** Copy and Quote (plus Edit for queued messages) only; sent messages cannot be edited or branched.
- **No effort or speed picker:** requests have no such field; the model sheet chooses the model only.
- **Shortcuts:** Mod+Shift+E opens the home composer, Mod+E focuses the composer, Mod+S toggles the sidebar (the drawer on phones); active only inside the app page.
- **Onboarding preview:** a DOM miniature built from tokens, not an illustration asset.

## Visual QA

`tools/qa/web-screens.sh [out-dir]` builds the app and runs `crates/openside-e2e/tests/web_screens.rs`. That test starts a hermetic daemon with seeded conversations, then captures every screen with headless Chromium at 390×844 and 1440×900, in light and dark (default `/tmp/openside-qa/redesign/core`). Seeded states include a short and a long thread, streaming with a subagent, a failed run, every ticket kind (options and free-text questions, secret, inline code request, takeover, device approval), long URL and Korean titles, archived and search results, and the daemon going offline. Set `OPENSIDE_SCREENS_ONLY=<screen,…>` to capture a subset, and compare the results with aside-relay's `docs/screenshots`.

The visual contracts also have automated acceptance tests, not just screenshots:

- `crates/openside-e2e/tests/web_surfaces.rs::web_and_extension_share_flat_surfaces_and_transient_status_pills` reads computed styles in the daemon-served web/PWA shell and the loaded MV3 extension panel at 390×844 and 1440×900, light and dark. It checks that the page is solid `--canvas` with no background image, that no element has a backdrop filter, that assistant text is transparent-backed in `--text`, that the person's bubble is `--bubble`, that all five pending ticket cards are `--surface-raised` and that Send is `--send-bg`, plus the transient connecting status pill appearing and disappearing. Captures go to `/tmp/openside-qa/redesign2/surfaces` by default (`OPENSIDE_SURFACES_DIR` overrides it).
- `crates/openside-e2e/tests/web_overflow.rs::every_route_and_surface_fits_all_supported_widths` checks 320, 360, 390, 768, 1024, 1280 and 1440px. It covers the conversation with login, choice, OTP, phone approval and confirm cards; expanded Tasks, tool activity and browser card; new chat; the collapsed and expanded code-request dock; Settings and every Workspace section; Pair entry; all six `/welcome` steps; and the phone/tablet conversation drawer and search. It checks document width and every rendered element's viewport bounds. Descendants of intentional inner scrollers are exempt, never their outer boxes; closed disclosures are not rendered. Set `OPENSIDE_OVERFLOW_DIR` to capture expanded activity at the two reference viewports in both themes.

Build `web/dist` first, then run `cargo test -j 4 -p openside-e2e --test web_surfaces --test web_overflow -- --test-threads=2` (through the local heavy-run wrapper where required). These tests require Chromium and the real sidecar Python environment; missing prerequisites fail rather than reporting an unexercised pass. Fixtures and pairing tokens are disposable; there are no live model calls.

## Attribution

- aside-relay: design system, UI patterns and theme preset palette values. MIT; see `third_party/aside-relay.LICENSE`.
- aside-remote: upstream of aside-relay. MIT; see `third_party/aside-remote.LICENSE`.
- Wanted Sans and D2Coding fonts: OFL-1.1.

`tools/check-licenses.py` lists all of these in its `VENDORED` table and writes them into the repository `NOTICE`.
