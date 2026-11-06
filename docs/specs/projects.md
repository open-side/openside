# Projects

A project is a named folder with instructions that chats belong to. A chat in a project works in the project's folder and its agent receives the project's instructions. Code: daemon `crates/opensided/src/projects.rs` (store, routes, chat membership), `crates/opensided/src/folder_picker.rs` (native folder picker and the folder check), types `crates/openside-proto/src/projects.rs`.

## Project

| Field | Meaning |
|---|---|
| `id` | Stable opaque id. |
| `name` | Display name: trimmed, inner whitespace collapsed, 1–80 characters (`invalid_project_name`). |
| `root` | Canonical absolute folder; fixed at creation. |
| `scratch` | The daemon made the folder (Start from scratch). |
| `instructions` | Text every chat in the project receives; empty for none; at most 16 KiB (`instructions_too_large`). |
| `needs_confirmation` | The folder has no recorded identity; chats cannot use it until a local person confirms it (see [Confirming a folder](#confirming-a-folder)). |
| `created_at`, `updated_at` | Creation, and the last rename or instructions change. |

Projects are stored in `<data_dir>/projects.json` (0600, atomic replacement). A missing file means no projects; an unreadable one is reported on stderr and the daemon starts without projects. Lists are sorted by name (case-insensitive), then id.

Each record also keeps `root_identity`, the folder's identity when the project was made or its folder last confirmed (see [The folder stays the one approved](#the-folder-stays-the-one-approved)); it is internal and never part of the API's `Project`. `needs_confirmation` is derived from it whenever the daemon reads the file.

On start, every record is checked as the API would have checked it: a name that is non-blank and at most 80 characters after normalising, instructions within 16 KiB, an absolute `root` that still passes the folder check below, or, for a scratch project, exactly `<data_dir>/projects/<id>`. A record that fails is left out with a line on stderr. The file is not rewritten at start, so a folder that is only unavailable for now (an unmounted drive) comes back on a later start, unless a project change rewrites the file first.

## Folders

- **Existing folder** (`root` given): the path must be absolute, exist, be a folder and not contain control characters. It is canonicalised (symlinks resolved) and refused (`400 invalid_project_root`) when it is the file system root (on Windows also a drive or share root), a home folder or a folder above it, a folder at or above openside's data folder, or a folder inside it. A folder covering the home folder would hand agents the person's whole account; one overlapping the data folder would let them edit the daemon's own state. The home folder is `HOME` on Unix; on Windows both `USERPROFILE` and `HOME` count when set, since programs there disagree on which is home.
- It is also refused when it lies in an operating-system tree (`/etc`, `/proc`, `/sys`, `/dev`, `/boot`, `/usr`, `/bin`, `/sbin`, `/lib*`, `/var`, `/run`, `/System`, `/Library`, `/private`, `%SystemRoot%`), unless it is inside the person's home, the temporary folder or `/run/media` (removable drives); inside another person's home (under `/home`, `/Users`, `/var/home` or `/root`, outside the person's own home); or is a credential or openside folder of the person's home or a folder above one (`.ssh`, `.gnupg`, `.aws`, `.kube`, `.docker`, `.password-store`, `.local/share/keyrings`, `.config/openside`, `.local/share/openside`, `.local/state/openside`, `Library/Keychains`, `Library/Application Support/openside`). A credential folder that does not exist yet is placed where it would be made, so `~/.config` is refused before `~/.config/openside` exists, and a symlinked one is judged by its target. A home that is the file system root (some service accounts) opens no operating-system tree.
- Folders are compared by identity, not by name: on Unix a folder is its device and inode number (`st_dev`, `st_ino`), on Windows its volume serial number and file index (`GetFileInformationByHandle`). "Above" or "inside" walks the canonical path's ancestors and compares each one's identity, so another spelling of the same folder (`/Users/Me` and `/users/me` on a case-insensitive volume, a bind mount) counts as the same folder.
- **Start from scratch** (no `root`): the daemon makes an empty private (0700) folder `<data_dir>/projects/<id>`; it skips the check above because the daemon made it.
- Two projects never share a folder (`409 project_exists`), compared by identity as above; a project whose folder no longer exists blocks nothing.
- Deleting a project never deletes its folder or the files in it.
- `POST /v1/folders/pick` opens the native folder picker of the computer running openside and returns the chosen folder after the same check (null when the person closed it); one picker at a time (`409 folder_picker_busy`), `503 folder_picker_unavailable` when none can start.
  Pickers, first available wins: Linux tries xdg-desktop-portal's FileChooser over the session bus, then `zenity`, then `kdialog`; macOS uses `osascript` (`choose folder`); Windows a PowerShell `FolderBrowserDialog`. Programs start without a shell. A picker that starts and then fails ends the request with `503 folder_picker_unavailable`; no second dialog follows.

### The folder stays the one approved

The project's folder is checked once when the project is made and its identity recorded (as above); on Unix and Windows a folder whose identity cannot be read is refused (`400 invalid_project_root`), and a record without one fails every later check until it is confirmed (below). After that, the daemon checks that `root` still canonicalises to itself, is a folder, and has the recorded identity:

- before every tool call in a chat of the project (`host.rs`); a mismatch refuses the call with `project_root_changed` ("The project's folder was moved, replaced or not yet confirmed; confirm it in Settings › Projects or recreate the project") and the tool does not run;
- when a chat starts in the project, moves into it, or is branched from one of its chats: `409 project_root_changed`.

So a folder renamed away with a new one made at the same path, or replaced by a symlink to somewhere else, approves nothing. Leaving the project (`PUT /v1/sessions/{id}/folder` `{}`) still works; the fix is to make a new project for the folder.

This detects a swap before each call starts. It is not race-proof against something that swaps the folder while a call is running: a local process already runs with the person's own rights, and the calls of one chat can run at the same time (parallel tool calls, subagents), so a call that swaps the folder can overlap another call that has already passed its check. Every call that starts after the swap is refused. Within a call the Guard still applies: file tools canonicalise their target when called (`tools/files.rs`), the Guard compares it with the chat's workspace (`openside_policy::path_within`), and file reads and writes refuse to follow a symlink swapped in after that check (`nofollow`).

A folder a chat attaches on its own (`workspace` without a project) records no identity; it is checked when attached and then guarded per call as above, without the identity check.

### Confirming a folder

A record without `root_identity` (one written before identities were kept, or edited by hand) fails closed: the API reports `needs_confirmation: true`, starting, moving or branching a chat into the project answers `409 project_root_changed`, and tool calls in its chats are refused. `PATCH /v1/projects/{id}` with `confirm_root: true` puts it right: it needs local access (`403 local_access_required` elsewhere), runs the folder check of a new project on `root` again (for a scratch project: `root` is still `<data_dir>/projects/<id>`), requires `root` to still be canonical, and records the folder's current identity (`400 invalid_project_root` when the folder no longer passes). It may come alone (an otherwise empty body is a valid change) and does not change `updated_at`. Any other `PATCH` from the computer running openside to a project that needs confirming records the identity the same way when the folder passes, and otherwise leaves the project unconfirmed. Settings › Projects shows such a project with a "Needs confirming" badge and, on the computer running openside, a "Confirm folder" button; elsewhere it explains the folder can only be confirmed there. The composer's project list disables the project until then.

## Chats in a project

A chat's place is a pair: the project it belongs to (`Session.project_id`) and the folder it works in (`Session.workspace`). The folder is the chat's Guard root and the file tools' working folder.

- `POST /v1/sessions` with `project_id` starts the chat in the project: `session.created` carries the project's folder as `workspace` and is followed, in the same SQLite transaction, by `session.folder_changed {project_id, workspace}`, so a chat never exists in the project's folder without belonging to the project. Unknown project → `404 project_not_found`; folder replaced → `409 project_root_changed`; together with `workspace` → `400 invalid_workspace`.
- `PUT /v1/sessions/{id}/folder` with `{project_id}` moves the chat into a project, with `{workspace}` attaches a folder of its own outside any project (same folder check as above), and with neither leaves the project and drops the folder. Both set → `400 invalid_value`; an agent's home chat → `409 agent_home`; the project's folder replaced → `409 project_root_changed`. It records one `session.folder_changed` event, none when nothing changes, decided against the chat's place at commit time.
- Deleting a project first gives each of its chats `session.folder_changed` with no project and no folder, all in one SQLite transaction, and only then rewrites `projects.json`. The chats stay. A crash or a failed write in between leaves at worst an empty project, never chats that name a deleted one.
- `POST /v1/sessions/{id}/fork` keeps the source's project: the branch starts with the same `session.created` + `session.folder_changed` pair, committed together (`409 project_root_changed` when the folder was replaced). When the caller is an agent-client token, or the source's project no longer exists, the branch keeps the folder as a plain `workspace` with no project, and the folder counts like any workspace a paired device names (`403 local_socket_required`).
- `GET /v1/sessions?project=<id>` lists one project's chats; `?project=none` the chats outside every project.

Starting a chat in a project, moving one in, and deleting a project all take the same lock, so a delete never misses a chat that was joining.

### A chat whose project is gone

A chat can name a project that no longer exists, for example after an unreadable `projects.json` made the daemon start without projects. Every reader tolerates it: the agent gets no project prompt part, `?project=<id>` still lists the chat under that id and `?project=none` does not, `PUT /v1/sessions/{id}/folder` moves or detaches it like any chat, and a branch carries the folder without the project (see fork above).

`session.folder_changed` is metadata: it does not move a chat up the list.

## Instructions

When a chat's project has non-empty instructions, its agent gets one more system prompt part after the profile's parts:

```
Project "<name>" (folder <root>). The person's instructions for every chat in this project:
<instructions>
```

The instructions are cut at 16 KiB (on a character boundary) when the part is built, even though records are already checked on load and on every change.

The open runtime remembers the workspace and instructions it started with. Moving the chat or editing the project's instructions reopens it before the next message while the chat is idle; a running turn finishes as it started.

## Who may do what

| Action | Route | Scope | Locality |
|---|---|---|---|
| List / read projects | `GET /v1/projects`, `GET /v1/projects/{id}` | read | any device |
| Create a project | `POST /v1/projects` | admin | the computer running openside only (`403 local_access_required`) |
| Rename, change instructions, delete | `PATCH`, `DELETE /v1/projects/{id}` | admin | any person device or the local socket; agent-client tokens get `403 person_device_required` |
| Confirm a project's folder | `PATCH /v1/projects/{id}` `{confirm_root: true}` | admin | the computer running openside only (`403 local_access_required`) |
| Pick a folder | `POST /v1/folders/pick` | admin | the computer running openside only |
| Start a chat in a project, move a chat into one | `POST /v1/sessions`, `PUT /v1/sessions/{id}/folder` `{project_id}` | steer | any person device or the local socket; agent-client tokens get `403 person_device_required` |
| Leave a project | `PUT /v1/sessions/{id}/folder` `{}` | steer | any device |
| Attach a folder of its own | `PUT /v1/sessions/{id}/folder` `{workspace}` | steer | the computer running openside only (`403 local_access_required`) |
| Start a chat with a `workspace` | `POST /v1/sessions` | steer | the local socket or a device with local access; other paired devices get `403 local_socket_required` |

A project's folder is approved for people on the computer running openside when the project is created, so a person starting or branching a chat in it is never a widening for a paired device. An agent-client token is software, not a person: it cannot use that approval, and it cannot change the instructions every chat in the project receives as a system prompt part (`api::Caller::require_person`). The local socket is the person's own computer and always counts as a person here. "Local access" is the local socket, or an admin person device on this computer (`api::LOCAL_REQUIRED` / `LOCAL_AWARE`). Routines keep their own rule: a routine that names a workspace needs the local socket.

## Decisions

- **A swap is detected before each tool call, not prevented during one.** The project's folder is re-checked by canonical path and identity before every tool call (see [The folder stays the one approved](#the-folder-stays-the-one-approved)). Making it race-proof against a local process that swaps the folder mid-call would need every tool to work through a held directory handle; such a process already runs with the person's own rights, so it is out of scope.
- **Operating-system trees, other people's homes and credential folders are refused as project folders.** A project's folder becomes the Guard root of every chat in it, so a project over `~/.ssh` or `/etc` would put keys, tokens or system configuration in reach of every agent turn. The person's own home and the temporary folder stay usable even where they sit inside an operating-system tree (`/var/home`, macOS's `/private/var/folders`).
- **A folder without a recorded identity fails closed.** Trusting it by path alone would let a folder replaced at the same path pass unnoticed; one local confirmation records the identity and restores the project.
- **The folder picker has no deadline.** A person is at the dialog; closing it ends the request. While it is open every other pick answers `409 folder_picker_busy` rather than stacking a second modal window.
