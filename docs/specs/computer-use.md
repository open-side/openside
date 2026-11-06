# Native computer use

An agent can use one desktop app the person connects to its chat. This is separate from the
daemon's own browser (`browser-cua`): the agent sees frames of that one window only, and its
input lands only inside it. Code: `crates/openside-computer` (backends),
`crates/opensided/src/computer_use.rs` (service and routes), `crates/opensided/src/tools/computer.rs`
(agent tools).

## Turning it on

`preferences.computer_use` (config `[preferences] computer_use`, default `false`). Only the
computer running openside may change it: the local socket, or the computer's own launcher on
loopback with `X-Openside-Local-Secret` (`PATCH /v1/settings/preferences` is in
`api::LOCAL_AWARE`). Any other paired device gets `403 local_access_required`, audited as
`refused_remote`. Local access is not enough on its own: an `openside mcp` client
(`x-openside-client: mcp`) or an agent-client token gets `403 person_access_required`, audited as
`refused_agent`, here and for the same key in `PUT /v1/settings/config`, so an agent cannot grant
itself computer use. Turning it off ends every
connection at once. The tools are hidden from agents while it is off, or while this computer is
known not to be eligible. The web shows it under Settings › Agents › Computer use.

## Eligibility

`GET /v1/computer-use` (`read`) reports `enabled`, `eligible`, `platform`, `backend`, and for each
of `capture`, `pointer` and `keyboard` whether it is `available`, the mechanism (`via`) and the
reason it is not. `eligible` needs window frames and at least one kind of input.

| Backend | Windows | Frames | Pointer | Keyboard |
|---|---|---|---|---|
| Hyprland (Linux) | `hyprctl -j clients` | `grim -T <stableId>`: that window only, even when covered or on a hidden workspace | virtual pointer through `/dev/uinput` (needs write access) | `wtype` |
| macOS | CoreGraphics window list | `screencapture -l <window>` into a fresh 0700 directory | CoreGraphics events (needs Accessibility permission) | CoreGraphics key events |

On macOS, eligibility checks the daemon's own Screen Recording and Accessibility grants.
`screencapture` runs as the daemon's child, so macOS normally attributes it to the same grant;
if it is denied anyway, the capture fails with an error rather than returning an empty frame. A
focus request that macOS refuses because Accessibility was revoked fails with a request to allow
openside again.

Other desktops report `backend: null` with a reason.

## Tools

| Tool | Policy | Behaviour |
|---|---|---|
| `computer_use_apps` | `privacy_read` `computer-use/apps` | Visible windows: id, app and title. Blocked windows are left out entirely, as are hidden and minimized ones. |
| `computer_use_connect` | `sensitive` on `app:<app>/<window>:<pid>:<instance>` | Asks in Guard and Full, denied in Read-only. One app per chat; connecting replaces the previous one. Blocked apps are refused before anyone is asked. |
| `computer_use_screenshot` | none beyond the connection | A frame of the connected window, at most 1600 px on the long edge. Coordinates in later actions are pixels of this frame. |
| `computer_use_act` | `computer_use` on `app:<app>/<connection id>` | `click`, `double_click`, `right_click`, `move`, `scroll`, `type`, `key`. Guard asks before every action, Full allows, Read-only denies. Returns a fresh frame. Typed text is logged as its length only. |
| `computer_use_stop` | none | Ends this chat's connection. |

The question names the app and the action, for example "Allow openside to see and control calc?",
"Allow openside to click at 400, 300 in calc?" or "Allow openside to type 12 characters in calc?".
It never contains the window title, which the app chooses and could word like another request;
when the executable's file name differs from the app id, it is added in parentheses. App ids and
titles lose control characters and are cut to 80 characters before they are shown anywhere.

A window is blocked when it belongs to a browser, a password manager or sign-in prompt, or
openside itself, judged by the app id and by the executable path the operating system reports
for the window's process (`/proc/<pid>/exe` on Linux, `proc_pidpath` on macOS), so renaming a
window's class does not get a browser through. A window whose executable cannot be read is
blocked too.

## Safety rules

- Pointer actions need a frame first, and a point outside the latest frame is refused
  (`computer_outside_window`) before any approval is asked.
- Private chats can neither list, connect, see nor stream the desktop: the tools are hidden from
  their agents, every tool called anyway refuses in its approval step, before anything runs or is
  saved, and the frame stream answers `409 private_session`. The chat's card is not shown there.
- A hidden or minimized window counts as gone: it can be neither connected nor driven.
- Approvals bind the window's instance: on Hyprland its stable id (from a counter that is never
  reused) plus its process's start time (`/proc/<pid>/stat` field 22); on macOS its window number,
  which the window server never reuses within a login. Both add a fingerprint of the process's
  executable path. A later window that reuses the id or the pid, or a process that execs another
  program, is a different window: connect fails, and so does every action on a connection to it
  (each action also compares the executable path itself). Windows whose start time cannot be read
  are not offered at all.
- Before each input the backend brings the connected window forward and checks that it is the
  focused window and that no other window or overlay covers the point. Hyprland reports no
  stacking order, so any other floating, pinned or special-workspace window over the point counts
  as covering it, even one on a workspace that is not shown. Focus is checked again before every
  press of a click, every typed chunk and every key: on Hyprland through `hyprctl activewindow`,
  on macOS through the frontmost window. On macOS, typed text and keys are also posted to the
  connected app's process (`CGEventPostToPid`), so they cannot reach another app.
- Accepted residual risk: on Hyprland, and for pointer events on macOS, input is not bound to a
  window. Wayland offers no way to send input to one window only, so `wtype` and the uinput
  pointer go to whatever has focus when the event arrives. If another window takes focus in the
  few milliseconds between the last focus check and the event, that one chunk, key or click can
  land in it. Each later chunk sees the change and stops the action.
- Turning the setting off and connecting take the same lock, so no connection outlives the switch.
- Approvals for actions name the connection id, which changes on every connect, so an approval
  never carries over to a later connection.
- Every connect, action and stop is written to the audit log (`computer_use`).
- Stop ends the connection at once and cancels an action in flight between input events
  (`computer_use_stopped`); only the frame of the stopped connection is dropped, so a reconnect
  racing it keeps its own. Stop comes from the agent, `POST /v1/sessions/{id}/computer-use/stop`
  (`steer`), stopping or deleting the chat, archiving it, or turning the setting off.

## Routes

| Method | Path | Scope | Description |
|---|---|---|---|
| GET | `/v1/computer-use` | `read` | Eligibility and the setting |
| GET | `/v1/sessions/{id}/computer-use` | `read`, person only | The chat's connection (app, title, phase, cursor, frame size). Same rules as the frames: agent clients get `403 person_device_required`, private chats `409 private_session` |
| GET | `/v1/sessions/{id}/computer-use/frames` | `browse`, person only | SSE: `state` on connect and every change, `frame` whenever the agent takes a new look. Agent clients get `403 person_device_required`; the person's own paired devices may watch on purpose (a phone following along). `404` for an unknown chat, `409 private_session` for private chats, `429 too_many_viewers` past 4 open streams per chat |
| POST | `/v1/sessions/{id}/computer-use/stop` | `steer` | Ends the connection |

The chat shows a card while an app is connected: the app's name, the latest frame with the
agent's pointer, what it is doing, and Stop.
