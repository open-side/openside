# openside web design

The web app adopts the design system of [aside-relay](https://github.com/mym0404/aside-relay) (MIT): its tokens, layout metrics, controls and interaction patterns.

## Philosophy

- One conversation column on a quiet canvas. Content leads; chrome stays out of the way.
- Restrained monochrome. Hierarchy comes from weight, size, spacing and soft surfaces. One primary color marks the action and state that matter: send, the active choice, "Needs you".
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
| `--glass` / `--glass-border` | white 72% / black 8% | `#1c1c1c` 72% / white 10% | floating header and composer |

Motion uses `--fast: 140ms`, `--normal: 220ms` and `--ease: cubic-bezier(0.2, 0, 0, 1)`. Syntax colours are the `--hl-*` tokens.

## Typography

- UI and content use Wanted Sans Variable; code uses D2Coding. Both fonts are bundled under `src/assets/fonts`, and nothing loads from the network.
- Body text is 16px with a line height of 1.65. Code blocks are 0.8125rem.
- The header title is 1.0625rem at weight 650. Section headings are 600, and sidebar group labels are 0.75rem muted at weight 500.
- Brand: the mark (`BrandMark` in `src/ui`, drawn in `currentColor` so it follows `--text` in both themes) then the word "openside" at 1.3125rem, weight 650, letter spacing -0.5px. The app icon `public/icons/icon.svg` is the same mark in `#f4f3ee` on a `#111318` rounded tile; `icon-maskable-*.png` use a full-bleed tile with the mark inside the maskable safe zone. The wordmark for docs is `docs/assets/logo.svg` (`logo-dark.svg` for dark backgrounds): the mark plus "openside" set in Lexend (OFL-1.1) at weight 450, converted to paths.

## Layout

| Metric | Value |
| --- | --- |
| Header | 64px, glass pills over a fading canvas |
| Sidebar | 288px, pinned at ≥ 821px; a drawer below that |
| Content column | 736px |
| Composer | 768px, glass, floating above the safe area |
| Phone sheet | dialogs become bottom sheets at ≤ 700px |

- Sidebar, from top to bottom: brand, then New chat, Search, the Active/Archived toggle, and the Pinned and Conversations groups. Workspace and Settings sit at the bottom.
- Rows are 48px tall with a radius of 12.

## Controls

- Icons come from Lucide at stroke 1.8 (`ICON_STROKE`). Icon buttons are 44px circles; ghost by default, outlined or glass where they float.
- Buttons come in primary, secondary, ghost and danger, with a 999px radius. Pressing scales them to 0.97 (icon buttons to 0.9).
- Menus (`PopoverMenu`) are canvas cards with a radius of 22, 44px items, a leading icon and danger items in red.
  - Queued message: Edit message / Send now / Cancel message.
  - Conversation row: Pin / Rename / Archive.
- Dialogs (`Dialog`) are centered cards on wider screens. On phones they become bottom sheets with a grab handle; `back` adds a leading chevron, and `size="wide"` gives a wider card.
- Toasts sit at the bottom center. They slide in, then fade out on dismissal.
- Segmented controls (`Segmented`) set the selected segment on a raised surface.

## Code requests

A verification-code request (`request_otp`) never moves the person: no route change, no takeover, no modal that hides the conversation.

- **One form everywhere.** `src/otp/otpForm.ts` builds the form as plain DOM so the web app (`components/OtpForm.tsx`) and the extension side panel mount the same thing; `src/otp/otp.css` styles both. It shows a letter avatar with the site, a single labelled `autocomplete=one-time-code` input (a six-cell backdrop on the authenticator and SMS channels; still one real input), help text naming the channel, Submit and Reject. The value is cleared before the request and never kept in state.
- **Saving an authenticator.** When the ticket allows it, a collapsed "Save this site's authenticator" section takes a masked setup key or `otpauth://` link. "Don't ask again for this site" (on by default) sends `{totp}`; off, the code is computed in the page and only the code is sent. Saving needs the `approve` scope: a device paired without it (scopes kept from `/pair`) never sees the section. When the scopes are unknown (pasted token, extension) and the daemon answers 403, the form stays, says saving needs approval rights, hides the checkbox and fills just this code from the key already pasted.
- **Where it appears.** Inline in the conversation that asked, focused unless the person is typing elsewhere. On any other screen, including over Settings or Workspace, a floating card (`components/OtpDock.tsx`, portaled beside the dialogs). Only a tapped push (`/sessions/:id?ticket=`) opens the full-screen code sheet (`components/OtpSheet.tsx`), which returns to the conversation when done.
- **Approve card.** A ticket with `otp_register` asks "Save the authenticator for <site> (<issuer · account>) to your vault?" with Approve and Decline; the key is never shown.

## Motion

- Sheets, drawers, backdrops, menus and toasts animate through CSS transitions keyed on Base UI's `data-starting-style` and `data-ending-style`. The jump-to-latest button scales in.
- A phone sheet closes when its handle is dragged more than 96px down or flicked faster than 700px/s, as in aside-relay. A shorter drag springs back. Only the handle starts a drag, so scrolling the content never closes the sheet.
- `prefers-reduced-motion: reduce`:
  - overlays keep a 120ms opacity fade without transforms;
  - press scale and the drag gesture turn off;
  - other transitions drop to 1ms;
  - scrolling to the latest message jumps instead of scrolling smoothly.

## Appearance

The Settings dialog (`components/AppearanceSettings.tsx`) has the following rows. Every choice is stored in this browser only:

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

`tools/qa/web-screens.sh [out-dir]` builds the app and runs `crates/openside-e2e/tests/web_screens.rs`. That test starts a hermetic daemon with seeded conversations, then captures every screen with headless Chromium at 390×844 and 1440×900, in light and dark (default `/tmp/openside-qa/redesign/core`). Set `OPENSIDE_SCREENS_ONLY=<screen,…>` to capture a subset, and compare the results with aside-relay's `docs/screenshots`. The code request surfaces are not seeded there yet.

## Attribution

- aside-relay: design system, UI patterns and theme preset palette values. MIT; see `third_party/aside-relay.LICENSE`.
- aside-remote: upstream of aside-relay. MIT; see `third_party/aside-remote.LICENSE`.
- Wanted Sans and D2Coding fonts: OFL-1.1.

`tools/check-licenses.py` lists all of these in its `VENDORED` table and writes them into the repository `NOTICE`.
