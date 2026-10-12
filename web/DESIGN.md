# openside web design

The web app adopts the design system of [aside-relay](https://github.com/mym0404/aside-relay) (MIT): its tokens, layout metrics, controls and interaction patterns.

## Philosophy

- One conversation column floating on a soft gradient backdrop (`--backdrop`: mist blue → lavender → peach in light, deep slate → periwinkle → rose in dark). Content leads; chrome is frosted glass that stays out of the way.
- Quiet colour. Hierarchy comes from weight, size, spacing and soft surfaces over the gradient; the palette stays neutral and one primary colour marks the action and state that matter: send, the active choice, "Needs you".
- The phone layout is the reference. Desktop widens the canvas and pins the sidebar but changes nothing else.
- Every command lives where the object lives: a queued message has its own menu, a conversation row has its own menu, and a ticket carries its own buttons.
- Motion is short and only confirms what happened. Under reduced motion, overlays fade instead of moving.

## Tokens

Tokens are defined in `src/styles.css`. Primitives (`src/ui/ui.css`) and screens read only these variables.

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `--canvas` | `#ffffff` | `#000000` | page, dialogs, menus |
| `--surface` | `#f7f7f7` | `#141414` | sidebar, cards, code blocks |
| `--bubble` | `#f1f1f1` | `#181818` | user messages |
| `--text` | `#171717` | `#f3f3f3` | body text |
| `--muted` | `#666666` | `#999999` | secondary text, icons |
| `--border` | `#e9e9e9` | `#343434` | dividers, outlines |
| `--hover` | `#ededed` | `#303030` | hover, selected row |
| `--control-border` | `#dcdcdc` | `#414141` | inputs, sheet handle |
| `--primary` | `#2f6fed` | `#2f6fed` | send, selection, attention |
| `--danger` | `#b42318` | `#ff9b91` | destructive items |
| `--glass` / `--glass-border` | white 72% / black 8% | navy `rgb(34 40 66)` 62% / white 12% | frosted chrome: header pills, sidebar panel, dialogs, event-chip lists |
| `--glass-strong-*` | navy 86%, white text | same | assistant bubbles, cards, panels, the composer |
| `--bubble-user-bg` / `-fg` | white 62% / ink | white 14% / white | user bubbles |
| `--status-pill-bg` / `-border` | white 18% / ink 14% | white 6% / white 16% | status line and event chips |

Strong glass stays dark in both themes. Inside it (`.bubble.assistant`, `.ticket`, `.panel`, `.composer`, `.composer-readonly`) `--text`, `--muted`, `--border`, `--surface`, `--canvas`, `--hover`, `--hl-*` and `--primary` are re-pointed at light-on-navy values, so Markdown, code, tables and buttons need no per-component rules; `--primary` there is the white pill and links use `--link`.

Motion uses `--fast: 140ms`, `--normal: 220ms` and `--ease: cubic-bezier(0.2, 0, 0, 1)`. Syntax colours are the `--hl-*` tokens.

## Typography

- UI and content use Wanted Sans Variable; code uses D2Coding. Both fonts are bundled under `src/assets/fonts`, and nothing loads from the network.
- Body text is 16px with a line height of 1.65. Code blocks are 0.8125rem.
- The header title is 1.0625rem at weight 650. Section headings are 600, and sidebar group labels are 0.75rem muted at weight 500.
- Brand: the mark (`BrandMark` in `src/ui`, drawn in `currentColor` so it follows `--text` in both themes) then the word "openside" at 1.3125rem, weight 650, letter spacing -0.5px. The app icon `public/icons/icon.svg` is the same mark in `#f4f3ee` on a `#111318` rounded tile; `icon-maskable-*.png` use a full-bleed tile with the mark inside the maskable safe zone. The wordmark for docs is `docs/assets/logo.svg` (`logo-dark.svg` for dark backgrounds): the mark plus "openside" set in Lexend (OFL-1.1) at weight 450, converted to paths.

## Layout

| Metric | Value |
| --- | --- |
| Header | 64px, no bar: a round menu button (drawer only), the title as a frosted pill (centred on phones), the actions pill; the transcript fades out underneath |
| Sidebar | 288px frosted panel inset 10px from the edges, pinned at ≥ 821px; a drawer below that |
| Content column | 736px |
| Composer | 768px strong-glass pill (radius 28) floating on the gradient with no dock background; a text row over a bar of + (Add files), compact tool pills, then Stop/Send. Send is the white circle; Stop is a translucent circle and asks "Stop everything?" (Keep running / Stop everything) before stopping the agent and every running Task. Cancelling one Task, from its card or the Tasks panel, asks "Cancel this task?" and leaves the agent and the other Tasks running |
| Phone sheet | dialogs become bottom sheets at ≤ 700px |

- Sidebar, from top to bottom: brand, New chat, then Search with a Show archived toggle beside it, and the Pinned and Conversations groups (Results while searching, Archived when toggled). Workspace and Settings sit at the bottom. A waiting conversation shows a "Needs you" badge.
- Rows are 40px tall (44px under a coarse pointer) with a radius of 10. Every label truncates with an ellipsis and keeps its full text in `title`; nothing in the sidebar scrolls sideways (`crates/openside-e2e/tests/web_overflow.rs`).
- New chat has no header title. On screens wider than 700px the greeting and composer sit together mid-screen; on phones the composer stays at the bottom.
- When the daemon cannot be reached, the shell shows one `.connection-banner` ("Can’t reach openside. Reconnecting…") instead of per-screen errors.
- Messages: assistant replies are strong-glass bubbles (radius 22, padding 14×18) that hug short text and cap at the column; user messages are lighter translucent bubbles on the right, at most 80%. No avatars.
- Tool calls: a finished run of calls folds into an event chip (`✓ Browsed example.com`, `Used 3 actions`, failures counted) that opens onto a glass list, one line per call (name, the file/URL/command in mono, a state only while running or after failing); each line opens the arguments and result. While a run has nothing new on screen, the last row is the status line: a muted outlined pill with shimmering text naming what the agent is doing ("Opening nid.naver.com", "Clicking on nid.naver.com", "Reading a page", "Thinking"; `components/toolStatus.ts`). It opens onto the same list and disappears when text streams or a card asks the person something. Visuals sit on an inset white sheet inside their card so dark mode stays calm.
- Live browser card (`components/BrowserPreview.tsx`): once a conversation has used the browser, a white card (radius 24, white in both themes because it is the agent's screen) sits at the top of the column, showing the agent tab's read-only screencast fitted at the top of a 4:3 box (capped at 40vh / 380px on wide screens). It sticks under the header while the agent works, while its done chip shows and as the slim pill; idle or waiting on the person it scrolls away with the transcript, so it never covers the card that asks. Frosted light pills sit over its bottom edge: the status pill (three circling dots and the same phrase as the status line while running; the tab title with a live dot when idle), "Action needed" with an amber triangle while a request waits (tapping it scrolls to the request), Minimize, and one control whose label says what it does: a labelled "Cancel task" (or "Stop" for the front agent) pill while running, which asks "Cancel this task?" first, and a Close × only when nothing is running, which hides the card until the agent uses the browser again. A `browser_pointer` click shows a white ripple at its point (`browser_act` targets refs and has no coordinates). After a run of at least a second, the status reads "✓ <first sentence of the reply>" and the card folds after 4s into a slim centred pill: a thumbnail, the status, Restore and ×. Tapping the picture or thumbnail opens the browser sheet. No tab open hides the card.
- Background Tasks on the live browser card: when the front agent hands browsing to Tasks, each Task's latest `browser.state` narrates the card (its phrase and target while acting, "Opening"/"Reading <host>" otherwise, "Action needed" while it waits on the person, "✓ <first sentence of its report>" when it completes, its page's error text or "Task cancelled" when it stops). With two or more, a strip of frosted tabs over the top edge picks the Task shown; the card follows whichever acted last until one is picked. The screencast follows the session's last-used tab, so a Task on another tab shows what its page says instead: heading, host, the form's field labels, result count, error. "Cancel task" on a running Task confirms "Cancel this task?" and cancels only that Task.
- Radii: 10 rows and small controls, 14 inputs, 22 bubbles, 24 (`--radius-glass`) cards, panels, the sidebar and dialogs, 28 the composer, 999 pills. Spinners follow the text colour; the accent is kept for focus and selection outside glass.

## Controls

- Icons come from Lucide at stroke 1.8 (`ICON_STROKE`). Icon buttons are 44px circles; ghost by default, outlined or glass where they float.
- Buttons come in primary, secondary, ghost and danger, with a 999px radius. Pressing scales them to 0.97 (icon buttons to 0.9).
- Menus (`PopoverMenu`) are canvas cards with a radius of 22, 44px items, a leading icon and danger items in red.
  - Queued message: Edit message / Send now / Cancel message.
  - Conversation row: Pin / Rename / Archive.
- Dialogs (`Dialog`) are centered cards on wider screens. On phones they become bottom sheets with a grab handle; `back` adds a leading chevron, and `size="wide"` gives a wider card.
- Toasts sit at the bottom center. They slide in, then fade out on dismissal.
- Segmented controls (`Segmented`) set the selected segment on a raised surface.
- Switches (`Switch`) are `role="switch"` checkboxes for on/off settings that apply immediately: a 36×22 track in a 44px hit box, the accent only when on.

## Code, login and choice requests

A verification-code (`request_otp`), login (`request_login`) or login-step choice (`request_choice`) request never moves the person: no route change, no takeover, no modal that hides the conversation.

- **One card language everywhere.** `src/otp/otpForm.ts` (code) and `src/otp/loginForm.ts` (login, choice, phone approval) build the cards as plain DOM so the web app (`components/AuthForm.tsx`) and the extension side panel mount the same thing; `src/otp/authCard.ts` draws the shared chrome and `src/otp/auth.css` styles all three. Each is a strong glass card (`--glass-strong-*`, `--radius-glass`) in both themes: header row with the site icon (the favicon the daemon sent as a `data:image/…` URL, else a provider mark, else the initial of the verified host), the action title ("Log in", "Choose an option"), the origin the daemon verified (scheme, punycode host, non-default port; mono, wrapped, never ellipsised) and a round close button (= reject). A name the page gives itself appears only as a labelled claim ("Page calls itself “X”") under that origin, never in place of it; the card's accessible name is title plus origin. Body text at 15px; pill inputs on `--glass-input-bg` with the label above; the white primary pill (`--pill-primary-*`) on the right; a hairline, then "Handled by openside" with the mark. Typed values are read, every input emptied, and only then sent; they never enter React state.
- **Code card.** One labelled `autocomplete=one-time-code` input (a six-cell backdrop on the authenticator and SMS channels; still one real input), help text naming the channel, Submit.
- **Login card.** Title "Log in" over the verified origin, one line saying the details go into the page (and the vault, if chosen), never into the chat. Fields mirror the page in page order (`autocomplete=username` / `current-password` so the browser's password manager can fill them), masked fields get an eye toggle, page checkboxes ("keep me signed in") are checkboxes. Saved accounts, when the ticket lists several, are rows above the fields. The page's other sign-in options are 48px pale tiles with locally drawn marks (`src/otp/glyphs.ts`: Google, Apple, Kakao, Naver, Facebook, GitHub, Microsoft, passkey, QR; anything else a key, with the page's label as its name and tooltip). "Save to vault" is on by default; it is hidden when the ticket cannot save or the device lacks `approve`, and a 403 puts the details back, hides it and lets the person log in without saving.
- **Choice card.** The site's question, the options as radio rows (label and description), Continue. After a login or choice resolves, the conversation keeps one quiet event chip ("Naver login saved to the vault", "Chose “Text me a code”").
- **Action needed.** While a code, login or choice ticket is pending, its row in the conversation starts with a white `.action-chip` pill ("Action needed", amber dot); the live browser card shows the same words with an amber triangle in its status pill.
- **Saving an authenticator.** When the ticket allows it, a collapsed "Save this site's authenticator" section takes a masked setup key or `otpauth://` link. "Don't ask again for this site" (on by default) sends `{totp}`; off, the code is computed in the page and only the code is sent. Saving needs the `approve` scope: a device paired without it (scopes kept from `/pair`) never sees the section. When the scopes are unknown (pasted token, extension) and the daemon answers 403, the form stays, says saving needs approval rights, hides the checkbox and fills just this code from the key already pasted.
- **Where they appear.** Inline in the conversation that asked, focused unless the person is typing elsewhere. On any other screen, or over another conversation, a compact banner docks under the header (`components/OtpDock.tsx`, portaled beside the dialogs; codes first, then logins and choices, then approvals). It is one row (what is needed, the site and the conversation's title, an arrow that goes to that conversation) with one button that opens the card in place, and its height is reserved in the conversation scroller so it never covers the timeline, composer or a card. Its accessible name includes the site. Only a tapped push (`/sessions/:id?ticket=`) opens the full-screen sheet (`components/OtpSheet.tsx`), which returns to the conversation when done.
- **Approve card.** A ticket with `otp_register` asks "Save the authenticator for <site> (<issuer · account>) to your vault?" with Approve and Decline; the key is never shown.

## Workspace

Workspace and Settings open as frosted glass sheets (`.ui-dialog`: `--glass` with `--blur-glass`, `--radius-glass`). Their sections (`src/pages`, styled in `pages.css`) share one list pattern built from `pages/blocks.tsx`:

- **Grouped lists.** Rows of a list sit in one glass inset (`--glass-inset-bg` / `--glass-inset-border`, radius 18) with hairline dividers, not as separate cards. Small tints (tags, icon tiles, code) mix `--text` over transparent so they work on any backdrop. A section header carries a muted summary (count, mode) on the left and at most one compact add button on the right.
- **Rows.** `ResourceCard`: title, one muted detail line, optional leading icon in a neutral tile. Row actions go in a trailing "…" `RowMenu` ("Options for <name>"), the same menu as conversation rows; destructive items are red and always confirm. On/off state is a trailing `Switch`, never a pair of Enable/Disable buttons. Expandable history uses `Disclosure`.
- **Badges carry state only:** neutral (read-only, kind), success (connected, unlocked, completed), attention (paused, rate limited, unavailable), danger (failed, denied, rejected). Never the accent colour.
- **Human text.** Schedules, run states, dates and policies are worded for people (`pages/format.ts`: "Weekdays at 08:00 · Asia/Seoul", "in 2 hours"); raw cron expressions and enum values only appear where a person types them.
- **Loading, errors and scope.** `ResourceView` shows a spinner, a load error with "Try again", or, on 403, `ScopeNotice` naming `openside pair --scope admin`; add buttons are hidden when the device lacks the scope.
- **Configuration** renders each schema key as a row: key and help on the left, control on the right (full width for text, lists and JSON). The save bar appears only while there are unsaved changes.
- **Phones:** every control in these sections is at least 44px tall at ≤ 700px, and nothing scrolls sideways at 390px.
- **Sign-in and pairing** (`TokenEntry`, `/pair`) are a strong glass card (`--glass-strong-*`) on the `--backdrop` gradient, with a pill input on `--glass-input-bg` and the white primary pill (`--pill-primary-*`), the same card language as the in-chat login cards.

## First-run guide

`/welcome` (`src/onboarding`, styled in `onboarding.css`) is one centered strong-glass card (`.panel`, so the in-card token re-pointing applies and primary actions are the white pill) on the `--backdrop` gradient; the shell hides the conversation list while it is open (`.shell[data-focus]`). Width is 560px on desktop and the card fills the phone with 16px margins.

- **One card, six steps.** A 4px segmented progress bar (`aria-label` "Step 2 of 6: Agents"), the step's `h1` (focused on every step change, so screen readers announce it), one muted lead paragraph, the step body, then a footer: `Back` (ghost) on the left, one forward button on the right. The forward button is primary only once the step achieved something; otherwise it is the secondary "Skip this step", so skipping never looks like the recommended path. "Skip setup" is a ghost button in the header on every step but the last.
- **Rows reuse `ResourceCard` / `resource-list`.** The card re-points `--glass-inset-*` at their dark values, so agents, profiles and the vault read like Workspace rows. Profile choice is a radio list (`.onb-choice`), categories are checkboxes; badges say why something cannot be picked.
- **Preview before write.** `Import` stays disabled until a dry run of the same profile and categories has been shown; any change to either drops the preview. The result and preview use the same count list.
- **Colour carries state only:** success ticks in the recap and the paired-phone line, nothing else is tinted.

## Motion

- Sheets, drawers, backdrops, menus and toasts animate through CSS transitions keyed on Base UI's `data-starting-style` and `data-ending-style`. The jump-to-latest button scales in.
- A phone sheet closes when its handle is dragged more than 96px down or flicked faster than 700px/s, as in aside-relay. A shorter drag springs back. Only the handle starts a drag, so scrolling the content never closes the sheet.
- `prefers-reduced-motion: reduce`:
  - overlays keep a 120ms opacity fade without transforms;
  - press scale and the drag gesture turn off;
  - other transitions drop to 1ms;
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
- **Date groups:** the sidebar groups only Pinned and Conversations, with no Today or Yesterday groups. The session API (`Session` in `docs/specs/openapi.json`) has no timestamps.
- **Default accent:** blue (`#2f6fed`) instead of aside-relay's purple. Swatches and presets cover the rest.
- **Text selection:** no global `user-select: none`. Agent output, code and tickets must stay copyable.
- **Markdown images:** shown as an alt-text pill, never as an `<img>`. All network traffic goes through `src/api` with the device token, and CSP limits `img-src`. An absolute http(s) image becomes a link the reader can open; relative and daemon URLs stay plain text. Attachments and files render through the authenticated artifact path instead.
- **Menus:** sign out lives in Settings. The sidebar bottom links to Workspace (skills, routines, memory, accounts, vault, devices) and Settings, because openside has daemon management where aside-relay has its browser panel.

## Visual QA

`tools/qa/web-screens.sh [out-dir]` builds the app and runs `crates/openside-e2e/tests/web_screens.rs`. That test starts a hermetic daemon with seeded conversations, then captures every screen with headless Chromium at 390×844 and 1440×900, in light and dark (default `/tmp/openside-qa/redesign/core`). Seeded states include a short and a long thread, streaming with a subagent, a failed run, every ticket kind (options and free-text questions, secret, inline code request, takeover, device approval), long URL and Korean titles, archived and search results, and the daemon going offline. Set `OPENSIDE_SCREENS_ONLY=<screen,…>` to capture a subset, and compare the results with aside-relay's `docs/screenshots`.

The visual contracts also have automated acceptance tests, not just screenshots:

- `crates/openside-e2e/tests/web_glass.rs::web_and_extension_share_glass_styles_and_transient_status_pills` reads computed styles in the daemon-served web/PWA shell and the loaded MV3 extension panel at 390×844 and 1440×900, light and dark. It checks gradient backdrops, blurred header chrome and composer, strong-glass assistant and all five ticket surfaces, white primary pills, and status pills appearing and disappearing. Captures go to `/tmp/openside-qa/polish/LastRows/after` by default (`OPENSIDE_GLASS_DIR` overrides it).
- `crates/openside-e2e/tests/web_overflow.rs::every_route_and_surface_fits_all_supported_widths` checks 320, 360, 390, 768, 1024, 1280 and 1440px. It covers the conversation with login, choice, OTP, phone approval and confirm cards; expanded Tasks, tool activity and browser card; new chat; the collapsed and expanded code-request dock; Settings and every Workspace section; Pair entry; all six `/welcome` steps; and the phone/tablet conversation drawer and search. It checks document width and every rendered element's viewport bounds. Descendants of intentional inner scrollers are exempt, never their outer boxes; closed disclosures are not rendered. Set `OPENSIDE_OVERFLOW_DIR` to capture expanded activity at the two reference viewports in both themes.

Build `web/dist` first, then run `cargo test -j 4 -p openside-e2e --test web_glass --test web_overflow -- --test-threads=2` (through the local heavy-run wrapper where required). These tests require Chromium and the real sidecar Python environment; missing prerequisites fail rather than reporting an unexercised pass. Fixtures and pairing tokens are disposable; there are no live model calls.

## Attribution

- aside-relay: design system, UI patterns and theme preset palette values. MIT; see `third_party/aside-relay.LICENSE`.
- aside-remote: upstream of aside-relay. MIT; see `third_party/aside-remote.LICENSE`.
- Wanted Sans and D2Coding fonts: OFL-1.1.

`tools/check-licenses.py` lists all of these in its `VENDORED` table and writes them into the repository `NOTICE`.
