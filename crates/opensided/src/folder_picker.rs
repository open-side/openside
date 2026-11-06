//! `POST /v1/folders/pick`: the native folder picker of the computer running openside, and
//! the check every project folder passes (`docs/specs/projects.md`).

use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use axum::{extract::State, http::StatusCode, Json};
use openside_proto::PickedFolder;

use crate::{api::HttpError, engine::Shared};

/// Blocks until the person chooses a folder (`Some`) or closes the picker (`None`).
/// `ErrorKind::NotFound` means this computer has no picker openside can start.
pub(crate) type FolderPicker = fn() -> io::Result<Option<PathBuf>>;

// Route sweeps call every route on the socket; no test may open a real dialog. A test that
// checks the call installs its own picker.
#[cfg(test)]
static PICKER: parking_lot::RwLock<FolderPicker> = parking_lot::RwLock::new(|| Ok(None));
#[cfg(not(test))]
static PICKER: parking_lot::RwLock<FolderPicker> = parking_lot::RwLock::new(pick_folder);

/// One dialog at a time: a second would stack another modal window on the person's screen.
/// `pub(crate)` so a test can hold it and observe `folder_picker_busy`.
pub(crate) static PICKING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const TITLE: &str = "Choose a project folder";

#[cfg(test)]
pub(crate) fn set_picker(picker: FolderPicker) {
    *PICKER.write() = picker;
}

/// The person's home folders, the ones `validate_root` keeps projects from covering. The
/// first is where a picker starts.
pub(crate) fn home_dirs() -> Vec<PathBuf> {
    candidate_homes(
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
        cfg!(windows),
    )
}

/// Windows programs disagree on the home folder: `USERPROFILE` is the system's, while shells
/// ported from Unix set `HOME`, sometimes elsewhere. Either may hold the person's files, so
/// both count there. Unix has only `HOME`.
fn candidate_homes(
    home: Option<std::ffi::OsString>,
    profile: Option<std::ffi::OsString>,
    windows: bool,
) -> Vec<PathBuf> {
    let candidates = if windows {
        vec![profile, home]
    } else {
        vec![home]
    };
    let mut homes: Vec<PathBuf> = Vec::new();
    for home in candidates.into_iter().flatten() {
        let home = PathBuf::from(home);
        if !home.as_os_str().is_empty() && !homes.contains(&home) {
            homes.push(home);
        }
    }
    homes
}

/// The canonical folder `raw` names, if a project may work in it. A project's folder becomes
/// the Guard root of its chats, so a root that covers a home folder (or everything) would
/// hand agents the person's whole account, and one that overlaps openside's data folder would
/// let them edit the daemon's own state. Folders made for scratch projects skip this check.
pub(crate) fn validate_root(
    raw: &str,
    homes: &[PathBuf],
    data_dir: &Path,
) -> Result<PathBuf, HttpError> {
    let invalid = || HttpError(StatusCode::BAD_REQUEST, "invalid_project_root");
    if raw.is_empty() || raw.chars().any(char::is_control) || !Path::new(raw).is_absolute() {
        return Err(invalid());
    }
    let root = std::fs::canonicalize(raw).map_err(|_| invalid())?;
    // `parent()` is `None` for `/` and, once canonical on Windows, for drive and share roots
    // (`\\?\C:\`, `\\?\UNC\server\share\`). A symlink may resolve to a name the API cannot
    // carry as a plain string.
    if !root.is_dir()
        || root.parent().is_none()
        || root
            .to_str()
            .map_or(true, |s| s.chars().any(char::is_control))
    {
        return Err(invalid());
    }
    // Compare like with like: a symlinked home or data folder must not slip past the check.
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if homes.iter().any(|home| inside(&canonical(home), &root)) {
        return Err(invalid());
    }
    let data = canonical(data_dir);
    if inside(&data, &root) || inside(&root, &data) {
        return Err(invalid());
    }
    let homes: Vec<PathBuf> = homes.iter().map(|home| canonical(home)).collect();
    let existing = |paths: &[&str]| -> Vec<PathBuf> {
        paths
            .iter()
            .filter_map(|path| std::fs::canonicalize(path).ok())
            .collect()
    };
    let mut open = existing(REMOVABLE_MOUNTS);
    open.push(canonical(&std::env::temp_dir()));
    if off_limits(
        &root,
        &homes,
        &existing(HOME_PARENTS),
        &system_trees(),
        &open,
    ) {
        return Err(invalid());
    }
    Ok(root)
}

/// Credential and openside folders inside a home folder. A project over one of them (or
/// above it) would put keys, tokens or the daemon's own settings in reach of every chat.
const CREDENTIAL_DIRS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".kube",
    ".docker",
    ".password-store",
    ".local/share/keyrings",
    ".config/openside",
    ".local/share/openside",
    ".local/state/openside",
    "Library/Keychains",
    "Library/Application Support/openside",
];

/// Folders that hold every user's homes; only the person's own home inside them is usable.
const HOME_PARENTS: &[&str] = &["/home", "/Users", "/var/home", "/root"];

/// Where udisks mounts removable drives (Arch, Fedora), inside the `/run` system tree. A
/// project on a USB drive is ordinary work; each user's mounts are closed to the others.
const REMOVABLE_MOUNTS: &[&str] = &["/run/media"];

/// The operating system's own trees, canonicalised so `/etc` on macOS (`/private/etc`) and a
/// merged `/bin` (`/usr/bin`) match.
fn system_trees() -> Vec<PathBuf> {
    let mut trees: Vec<PathBuf> = Vec::new();
    if cfg!(unix) {
        for tree in [
            "/etc", "/proc", "/sys", "/dev", "/boot", "/usr", "/bin", "/sbin", "/lib", "/lib32",
            "/lib64", "/libx32", "/var", "/run", "/System", "/Library", "/private",
        ] {
            trees.push(PathBuf::from(tree));
        }
    }
    if let Some(windows) = std::env::var_os("SystemRoot") {
        trees.push(PathBuf::from(windows));
    }
    trees
        .into_iter()
        .filter_map(|tree| std::fs::canonicalize(tree).ok())
        .collect()
}

/// Whether `root` lies where no project may work: in an operating-system tree (`system`), in
/// another person's home (inside `home_parents`), or over a credential folder of the person's
/// own home. A folder inside the person's home or an `open` folder (the temporary folder,
/// removable drives) is never an operating-system tree, so a home under `/var/home` and
/// macOS's temporary folder under `/private/var` stay usable. All paths are canonical.
fn off_limits(
    root: &Path,
    homes: &[PathBuf],
    home_parents: &[PathBuf],
    system: &[PathBuf],
    open: &[PathBuf],
) -> bool {
    for home in homes {
        for dir in CREDENTIAL_DIRS {
            // Resolved, so a folder above the target of a symlinked `~/.ssh` is caught too.
            let credential = resolved(&home.join(dir));
            if inside(root, &credential) || inside(&credential, root) {
                return true;
            }
        }
    }
    // A service account's home may be `/`; being inside it would open every system tree.
    if homes
        .iter()
        .any(|home| home.parent().is_some() && inside(root, home))
    {
        return false;
    }
    home_parents.iter().any(|parent| inside(root, parent))
        || (!open.iter().any(|folder| inside(root, folder))
            && system.iter().any(|tree| inside(root, tree)))
}

/// `path` with its deepest existing ancestor canonicalised: a folder that does not exist yet
/// is placed where it would be made.
fn resolved(path: &Path) -> PathBuf {
    for ancestor in path.ancestors() {
        if let Ok(real) = std::fs::canonicalize(ancestor) {
            return match path.strip_prefix(ancestor) {
                Ok(rest) if !rest.as_os_str().is_empty() => real.join(rest),
                _ => real,
            };
        }
    }
    path.to_owned()
}

/// Which folder a path names, independent of its spelling. Comparing names misses aliases a
/// file system makes, such as `/Users/Me` and `/users/me` on a case-insensitive volume. Unix:
/// device and inode; Windows: volume serial number and file index.
#[derive(PartialEq, Eq)]
struct FolderId(u64, u64);

#[cfg(unix)]
fn folder_id(path: &Path) -> Option<FolderId> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    Some(FolderId(metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn folder_id(path: &Path) -> Option<FolderId> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    // A folder opens only with backup semantics; no access right is needed to read its
    // identity, and sharing everything never gets in another program's way.
    let folder = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .ok()?;
    // SAFETY: a plain C struct of integers, for which all zeroes is a valid value.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle stays open for the call and `info` is a valid out pointer.
    if unsafe { GetFileInformationByHandle(folder.as_raw_handle() as _, &mut info) } == 0 {
        return None;
    }
    Some(FolderId(
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}

/// Whether `path` is `folder` or inside it: some ancestor of `path` is the same folder.
fn inside(path: &Path, folder: &Path) -> bool {
    let Some(folder) = folder_id(folder) else {
        return false;
    };
    path.ancestors()
        .any(|ancestor| folder_id(ancestor).is_some_and(|id| id == folder))
}

/// Whether two project folders are the same folder on this computer; a folder that no longer
/// exists matches nothing. Blocking: it reads file metadata.
pub(crate) fn same_folder(a: &Path, b: &Path) -> bool {
    match (folder_id(a), folder_id(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// The identity recorded for a project's folder when the project is made ([`FolderId`]).
pub(crate) fn recorded_identity(path: &Path) -> Option<[u64; 2]> {
    folder_id(path).map(|FolderId(volume, index)| [volume, index])
}

/// Whether `root` still names the folder the project was made with: it canonicalises to
/// itself, is a folder, and has the recorded identity. A folder that was renamed away with a
/// new one made at its path, or swapped for a symlink, fails. On platforms with folder
/// identities a record without one fails too (fail closed). Blocking.
pub(crate) fn still_same(root: &str, recorded: Option<[u64; 2]>) -> bool {
    let Ok(canonical) = std::fs::canonicalize(root) else {
        return false;
    };
    canonical.as_os_str() == std::ffi::OsStr::new(root)
        && canonical.is_dir()
        && match recorded {
            Some(recorded) => recorded_identity(&canonical) == Some(recorded),
            None => !cfg!(any(unix, windows)),
        }
}

/// Open the picker and wait for the person. Local access only (`api::LOCAL_REQUIRED`).
pub(crate) async fn pick(State(s): State<Shared>) -> Result<Json<PickedFolder>, HttpError> {
    let Ok(guard) = PICKING.try_lock() else {
        return Err(HttpError(StatusCode::CONFLICT, "folder_picker_busy"));
    };
    let picker = *PICKER.read();
    // The guard moves into the worker: if the caller goes away while the dialog is still on
    // screen, a new request must keep getting `folder_picker_busy` until it closes.
    let picked = tokio::task::spawn_blocking(move || {
        let picked = picker();
        drop(guard);
        picked
    })
    .await
    .map_err(|_| HttpError::internal())?;
    let path = match picked {
        Ok(Some(path)) => path,
        Ok(None) => return Ok(Json(PickedFolder { path: None })),
        // A picker that cannot start or fails after starting leaves the person nothing to
        // retry here; either way they can type the path instead.
        Err(_) => {
            return Err(HttpError(
                StatusCode::SERVICE_UNAVAILABLE,
                "folder_picker_unavailable",
            ))
        }
    };
    let raw = path
        .to_str()
        .ok_or(HttpError(StatusCode::BAD_REQUEST, "invalid_project_root"))?;
    let root = validate_root(raw, &home_dirs(), &s.config.data_dir)?;
    Ok(Json(PickedFolder {
        path: root.to_str().map(str::to_owned),
    }))
}

/// Try this platform's pickers in order; the first one that exists answers.
#[cfg_attr(test, allow(dead_code))]
fn pick_folder() -> io::Result<Option<PathBuf>> {
    let pickers: &[fn() -> io::Result<Option<PathBuf>>] = &[
        #[cfg(target_os = "linux")]
        portal::pick,
        #[cfg(all(unix, not(target_os = "macos")))]
        zenity,
        #[cfg(all(unix, not(target_os = "macos")))]
        kdialog,
        #[cfg(target_os = "macos")]
        osascript,
        #[cfg(windows)]
        powershell,
    ];
    for picker in pickers {
        match picker() {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            answer => return answer,
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no folder picker on this computer",
    ))
}

/// Run a dialog program (argv, never shell text) and read the chosen path from its stdout.
/// A program that is not installed fails with `NotFound`, so the next picker is tried.
#[cfg_attr(test, allow(dead_code))]
fn run_dialog(mut command: Command, cancelled: fn(&Output) -> bool) -> io::Result<Option<PathBuf>> {
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if output.status.success() {
        return path_from_stdout(output.stdout).map(Some);
    }
    if cancelled(&output) {
        return Ok(None);
    }
    Err(io::Error::other(format!(
        "folder picker exited with {}",
        output.status
    )))
}

/// Exactly one trailing line break is the program's; anything else belongs to the name.
#[cfg_attr(test, allow(dead_code))]
fn path_from_stdout(mut bytes: Vec<u8>) -> io::Result<PathBuf> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    if bytes.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "folder picker printed no path",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes).map(PathBuf::from).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "folder picker printed a path that is not UTF-8",
            )
        })
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
#[cfg_attr(test, allow(dead_code))]
fn zenity() -> io::Result<Option<PathBuf>> {
    let mut command = Command::new("zenity");
    command.args([
        "--file-selection",
        "--directory",
        &format!("--title={TITLE}"),
    ]);
    run_dialog(command, |output| output.status.code() == Some(1))
}

#[cfg(all(unix, not(target_os = "macos")))]
#[cfg_attr(test, allow(dead_code))]
fn kdialog() -> io::Result<Option<PathBuf>> {
    let mut command = Command::new("kdialog");
    command.arg("--getexistingdirectory");
    if let Some(home) = home_dirs().into_iter().next() {
        command.arg(home);
    }
    command.args(["--title", TITLE]);
    run_dialog(command, |output| output.status.code() == Some(1))
}

#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
fn osascript() -> io::Result<Option<PathBuf>> {
    let mut command = Command::new("osascript");
    command.args([
        "-e",
        &format!("POSIX path of (choose folder with prompt \"{TITLE}\")"),
    ]);
    // AppleScript reports "User canceled" as error -128 with exit status 1.
    run_dialog(command, |output| {
        output.status.code() == Some(1) && String::from_utf8_lossy(&output.stderr).contains("-128")
    })
}

#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
fn powershell() -> io::Result<Option<PathBuf>> {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW: the dialog is the only window the person should see.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // Exit 2 marks a cancelled dialog; PowerShell itself uses 1 for script errors. UTF-8
    // output keeps folder names outside the console code page intact.
    let script = format!(
        "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; \
         Add-Type -AssemblyName System.Windows.Forms; \
         $dialog = New-Object System.Windows.Forms.FolderBrowserDialog; \
         $dialog.Description = '{TITLE}'; \
         if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) \
         {{ [Console]::Out.Write($dialog.SelectedPath); exit 0 }} else {{ exit 2 }}"
    );
    let mut command = Command::new("powershell");
    command
        .args(["-NoProfile", "-STA", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW);
    run_dialog(command, |output| output.status.code() == Some(2))
}

/// xdg-desktop-portal's FileChooser: the desktop's own dialog, and the only one a sandboxed
/// or Wayland-only session may offer.
#[cfg(target_os = "linux")]
#[cfg_attr(test, allow(dead_code))]
mod portal {
    use std::{collections::HashMap, io, path::PathBuf};

    use zbus::{
        blocking::{Connection, Proxy},
        zvariant::{OwnedObjectPath, OwnedValue, Value},
    };

    const DESTINATION: &str = "org.freedesktop.portal.Desktop";
    const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";

    fn unavailable(error: impl std::fmt::Display) -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, error.to_string())
    }

    fn failed(error: impl std::fmt::Display) -> io::Error {
        io::Error::other(error.to_string())
    }

    /// `NotFound` while nothing has been shown yet, so the caller may try another picker;
    /// any later failure is final, a second dialog must not follow the first.
    pub(super) fn pick() -> io::Result<Option<PathBuf>> {
        let connection = Connection::session().map_err(unavailable)?;
        let sender = connection
            .unique_name()
            .ok_or_else(|| unavailable("no unique bus name"))?
            .as_str()
            .trim_start_matches(':')
            .replace('.', "_");
        let token = format!("openside_{:016x}", rand::random::<u64>());
        // Subscribe before calling: the portal may answer before `OpenFile` returns.
        let predicted = format!("{DESKTOP_PATH}/request/{sender}/{token}");
        let request = Proxy::new(
            &connection,
            DESTINATION,
            predicted.clone(),
            "org.freedesktop.portal.Request",
        )
        .map_err(unavailable)?;
        let responses = request.receive_signal("Response").map_err(unavailable)?;
        let chooser = Proxy::new(
            &connection,
            DESTINATION,
            DESKTOP_PATH,
            "org.freedesktop.portal.FileChooser",
        )
        .map_err(unavailable)?;
        let options: HashMap<&str, Value<'_>> = HashMap::from([
            ("handle_token", Value::from(token.as_str())),
            ("directory", Value::from(true)),
            ("modal", Value::from(true)),
        ]);
        let handle: OwnedObjectPath = chooser
            .call("OpenFile", &("", super::TITLE, options))
            .map_err(unavailable)?;
        // Portals older than 0.9 pick their own request path; listen there instead.
        let responses = if handle.as_str() == predicted {
            responses
        } else {
            Proxy::new(
                &connection,
                DESTINATION,
                handle.as_str().to_owned(),
                "org.freedesktop.portal.Request",
            )
            .and_then(|request| request.receive_signal("Response"))
            .map_err(failed)?
        };
        let Some(message) = responses.into_iter().next() else {
            return Err(failed("portal closed the request without answering"));
        };
        let (code, results): (u32, HashMap<String, OwnedValue>) =
            message.body().deserialize().map_err(failed)?;
        match code {
            0 => {}
            1 => return Ok(None),
            _ => return Err(failed(format!("portal answered {code}"))),
        }
        let uri = match results.get("uris").map(|uris| &**uris) {
            Some(Value::Array(uris)) => match uris.iter().next() {
                Some(Value::Str(uri)) => uri.as_str().to_owned(),
                _ => return Err(failed("portal chose no folder")),
            },
            _ => return Err(failed("portal chose no folder")),
        };
        url::Url::parse(&uri)
            .ok()
            .and_then(|uri| uri.to_file_path().ok())
            .map(Some)
            .ok_or_else(|| failed("portal chose a folder that is not local"))
    }
}

// The fixtures build symlinks with the Unix API.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn rejected(raw: &str, home: Option<&Path>, data_dir: &Path) {
        let homes: Vec<PathBuf> = home.map(Path::to_path_buf).into_iter().collect();
        let error = validate_root(raw, &homes, data_dir).unwrap_err();
        assert_eq!(
            (error.0, error.1),
            (StatusCode::BAD_REQUEST, "invalid_project_root"),
            "{raw}"
        );
    }

    struct Layout {
        _temp: tempfile::TempDir,
        home: PathBuf,
        data: PathBuf,
        project: PathBuf,
    }

    fn layout() -> Layout {
        let temp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(temp.path()).unwrap();
        let home = base.join("home");
        let data = home.join(".local/share/openside");
        let project = home.join("code/app");
        for dir in [&data, &project] {
            std::fs::create_dir_all(dir).unwrap();
        }
        Layout {
            _temp: temp,
            home,
            data,
            project,
        }
    }

    fn text(path: &Path) -> &str {
        path.to_str().unwrap()
    }

    #[test]
    fn a_folder_inside_home_is_accepted_canonically() {
        let l = layout();
        let raw = format!("{}/../app/.", text(&l.project));
        assert_eq!(
            validate_root(&raw, std::slice::from_ref(&l.home), &l.data).unwrap(),
            l.project
        );
        // Without a known home only the data folder and the file system root are refused.
        assert_eq!(
            validate_root(text(&l.project), &[], &l.data).unwrap(),
            l.project
        );
    }

    #[test]
    fn root_home_and_its_ancestors_are_refused() {
        let l = layout();
        rejected("/", Some(&l.home), &l.data);
        rejected(text(&l.home), Some(&l.home), &l.data);
        rejected(text(l.home.parent().unwrap()), Some(&l.home), &l.data);
        // A home reached through a symlink is compared by its target.
        let link = l.home.parent().unwrap().join("home-link");
        std::os::unix::fs::symlink(&l.home, &link).unwrap();
        rejected(text(&l.home), Some(&link), &l.data);
    }

    #[test]
    fn the_data_folder_its_ancestors_and_its_contents_are_refused() {
        let l = layout();
        let inside = l.data.join("projects/p1");
        std::fs::create_dir_all(&inside).unwrap();
        for dir in [&l.data, &l.data.parent().unwrap().to_owned(), &inside] {
            rejected(text(dir), Some(&l.home), &l.data);
        }
        // `.local` holds the data folder below it; refusing it is refusing an ancestor.
        rejected(text(&l.home.join(".local")), Some(&l.home), &l.data);
    }

    #[test]
    fn files_missing_relative_and_control_character_paths_are_refused() {
        let l = layout();
        let file = l.project.join("notes.txt");
        std::fs::write(&file, "x").unwrap();
        rejected(text(&file), Some(&l.home), &l.data);
        rejected(text(&l.project.join("missing")), Some(&l.home), &l.data);
        rejected("code/app", Some(&l.home), &l.data);
        rejected("", Some(&l.home), &l.data);
        rejected(&format!("{}\n", text(&l.project)), Some(&l.home), &l.data);
        let odd = l.home.join("odd\u{7}name");
        std::fs::create_dir_all(&odd).unwrap();
        rejected(text(&odd), Some(&l.home), &l.data);
    }

    #[test]
    fn a_symlink_is_resolved_and_checked_by_its_target() {
        let l = layout();
        let link = l.home.join("shortcut");
        std::os::unix::fs::symlink(&l.project, &link).unwrap();
        assert_eq!(
            validate_root(text(&link), std::slice::from_ref(&l.home), &l.data).unwrap(),
            l.project
        );
        let sneaky = l.home.join("sneaky");
        std::os::unix::fs::symlink(&l.data, &sneaky).unwrap();
        rejected(text(&sneaky), Some(&l.home), &l.data);
    }

    #[test]
    fn dialog_output_keeps_the_name_and_drops_one_line_break() {
        assert_eq!(
            path_from_stdout(b"/a/b \n".to_vec()).unwrap(),
            PathBuf::from("/a/b ")
        );
        assert_eq!(
            path_from_stdout(b"/a/b\r\n".to_vec()).unwrap(),
            PathBuf::from("/a/b")
        );
        assert!(path_from_stdout(b"\n".to_vec()).is_err());
    }

    #[test]
    fn folders_are_compared_by_identity_not_by_name() {
        let l = layout();
        // Another name for the home folder, as a case-insensitive volume would give one.
        let alias = l.home.parent().unwrap().join("HOME-alias");
        std::os::unix::fs::symlink(&l.home, &alias).unwrap();
        assert!(inside(&l.project, &alias));
        assert!(inside(&l.home, &alias));
        assert!(!inside(&alias.parent().unwrap().join("elsewhere"), &alias));
        // Whole folders only: `code-old` is not inside `code`.
        let sibling = l.home.join("code-old");
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(!inside(&sibling, &l.home.join("code")));
        assert!(same_folder(&alias, &l.home));
        assert!(!same_folder(&l.project, &l.home));
        // A folder that no longer exists matches nothing, not even its own name.
        let gone = l.home.join("gone");
        assert!(!same_folder(&gone, &gone));
    }

    #[test]
    fn every_home_folder_counts() {
        let os = |s: &str| Some(std::ffi::OsString::from(s));
        assert_eq!(
            candidate_homes(os("/h"), os("/p"), true),
            [PathBuf::from("/p"), PathBuf::from("/h")]
        );
        assert_eq!(
            candidate_homes(os("/h"), os("/p"), false),
            [PathBuf::from("/h")]
        );
        assert_eq!(
            candidate_homes(None, os("/p"), false),
            Vec::<PathBuf>::new()
        );
        assert_eq!(
            candidate_homes(os("/p"), os("/p"), true),
            [PathBuf::from("/p")]
        );
        assert_eq!(
            candidate_homes(os(""), os("/p"), true),
            [PathBuf::from("/p")]
        );

        // A folder above either home is refused; one inside the first home is not.
        let l = layout();
        let other = l.home.parent().unwrap().join("other/profile");
        std::fs::create_dir_all(&other).unwrap();
        let homes = [l.home.clone(), other.clone()];
        let above_other = other.parent().unwrap();
        let error = validate_root(text(above_other), &homes, &l.data).unwrap_err();
        assert_eq!(error.1, "invalid_project_root");
        assert_eq!(
            validate_root(text(&l.project), &homes, &l.data).unwrap(),
            l.project
        );
    }

    struct Machine {
        _temp: tempfile::TempDir,
        base: PathBuf,
        os: PathBuf,
        parents: PathBuf,
        me: PathBuf,
        open: Vec<PathBuf>,
    }

    /// A stand-in computer: `os` plays the system trees, with the homes under `os/var/home`
    /// (as on Fedora Silverblue) and the temporary folder and removable drives inside `os`.
    fn machine() -> Machine {
        let temp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(temp.path()).unwrap();
        let os = base.join("os");
        let parents = os.join("var/home");
        let me = parents.join("me");
        let open = vec![os.join("tmp"), os.join("run/media")];
        for dir in [
            os.join("etc"),
            os.join("usr/share"),
            os.join("run/user/1000"),
            os.join("tmp/scratch"),
            os.join("run/media/me/usb"),
            parents.join("other/code"),
            me.join("code"),
            base.join("work"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        Machine {
            _temp: temp,
            base,
            os,
            parents,
            me,
            open,
        }
    }

    impl Machine {
        fn off(&self, root: &Path) -> bool {
            off_limits(
                root,
                std::slice::from_ref(&self.me),
                std::slice::from_ref(&self.parents),
                std::slice::from_ref(&self.os),
                &self.open,
            )
        }
    }

    #[test]
    fn system_trees_and_other_peoples_homes_are_off_limits() {
        let m = machine();
        for refused in [
            m.os.clone(),
            m.os.join("etc"),
            m.os.join("usr/share"),
            m.os.join("run/user/1000"),
            m.os.join("var"),
            m.parents.clone(),
            m.parents.join("other"),
            m.parents.join("other/code"),
        ] {
            assert!(m.off(&refused), "{refused:?}");
        }
        // The person's own home, the temporary folder and removable drives sit inside the
        // system trees here and stay usable; so does a folder outside every tree.
        for allowed in [
            m.me.join("code"),
            m.os.join("tmp/scratch"),
            m.os.join("run/media/me/usb"),
            m.base.join("work"),
        ] {
            assert!(!m.off(&allowed), "{allowed:?}");
        }
    }

    #[test]
    fn credential_folders_and_folders_above_them_are_off_limits() {
        let m = machine();
        for dir in [".ssh/keys", ".config/app", ".local/bin", "Library/Fonts"] {
            std::fs::create_dir_all(m.me.join(dir)).unwrap();
        }
        for refused in [".ssh", ".ssh/keys", ".config", ".local", "Library"] {
            assert!(m.off(&m.me.join(refused)), "{refused}");
        }
        // Neighbours of a credential folder are not above it.
        for allowed in [".config/app", ".local/bin", "Library/Fonts"] {
            assert!(!m.off(&m.me.join(allowed)), "{allowed}");
        }
        std::fs::create_dir_all(m.me.join(".config/openside/sub")).unwrap();
        assert!(m.off(&m.me.join(".config/openside/sub")));
    }

    #[test]
    fn a_symlinked_credential_folder_is_checked_by_its_target() {
        let m = machine();
        let vault = m.base.join("vault/ssh");
        std::fs::create_dir_all(&vault).unwrap();
        std::os::unix::fs::symlink(&vault, m.me.join(".ssh")).unwrap();
        assert!(m.off(&vault));
        assert!(m.off(&m.base.join("vault")));
        assert!(!m.off(&m.base.join("work")));
        // `~/.config` linked elsewhere, before openside made its folder in it.
        let dotfiles = m.base.join("dotfiles/config");
        std::fs::create_dir_all(&dotfiles).unwrap();
        std::os::unix::fs::symlink(&dotfiles, m.me.join(".config")).unwrap();
        assert!(m.off(&dotfiles));
        assert!(m.off(&m.base.join("dotfiles")));
    }

    #[test]
    fn a_home_at_the_file_system_root_opens_no_system_tree() {
        let m = machine();
        let root_home = [PathBuf::from("/")];
        let etc = m.os.join("etc");
        assert!(off_limits(
            &etc,
            &root_home,
            &[],
            std::slice::from_ref(&m.os),
            &[]
        ));
        assert!(!off_limits(
            &m.base.join("work"),
            &root_home,
            &[],
            std::slice::from_ref(&m.os),
            &[]
        ));
    }

    #[test]
    fn real_system_folders_and_home_credentials_are_refused() {
        let l = layout();
        for raw in ["/etc", "/usr/share"] {
            rejected(raw, Some(&l.home), &l.data);
        }
        #[cfg(target_os = "linux")]
        rejected("/proc", Some(&l.home), &l.data);
        for dir in [".ssh", ".config/openside"] {
            let credential = l.home.join(dir);
            std::fs::create_dir_all(credential.join("inner")).unwrap();
            rejected(text(&credential), Some(&l.home), &l.data);
            rejected(text(&credential.join("inner")), Some(&l.home), &l.data);
        }
        rejected(text(&l.home.join(".config")), Some(&l.home), &l.data);
        assert_eq!(
            validate_root(text(&l.project), std::slice::from_ref(&l.home), &l.data).unwrap(),
            l.project
        );
    }
}
