//! Linux backend for the Hyprland compositor.
//!
//! * Windows come from `hyprctl -j clients`; a window's id is its Hyprland address.
//! * Frames come from `grim -T <stable id>` (ext-image-copy-capture of one toplevel), so a frame
//!   holds that window's own pixels even when another window overlaps it.
//! * Focus and the absolute cursor move go through Hyprland: its Lua dispatchers (`hyprctl
//!   repl`, Hyprland 0.55+) or the classic `hyprctl dispatch` on older versions.
//! * Buttons and wheels go through a uinput virtual pointer; text and keys through `wtype`.

use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde_json::Value;
use tokio::process::Command;

use crate::uinput::{self, Pointer};
use crate::{
    Backend, Eligibility, Error, Feature, Frame, Input, Key, KeyCombo, Modifier, NamedKey, Rect,
    Window,
};

const TIMEOUT: Duration = Duration::from_secs(10);
/// Typed text is sent in pieces so a stop takes effect between them.
const TYPE_CHUNK: usize = 48;

#[derive(Default)]
pub(crate) struct Hyprland {
    pointer: Mutex<Option<Pointer>>,
    /// Bumped by [`Backend::release`]; a pointer taken out under an older epoch is dropped.
    epoch: AtomicU64,
}

#[derive(Clone)]
struct Instance {
    signature: String,
    wayland: Option<String>,
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let path = dir.join(program);
            Path::new(&path).is_file()
                && std::fs::metadata(&path)
                    .map(|meta| {
                        std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o111 != 0
                    })
                    .unwrap_or(false)
        })
    })
}

async fn output(
    program: &str,
    args: &[&str],
    instance: Option<&Instance>,
) -> Result<Vec<u8>, Error> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(instance) = instance {
        command.env("HYPRLAND_INSTANCE_SIGNATURE", &instance.signature);
        if let Some(wayland) = &instance.wayland {
            command.env("WAYLAND_DISPLAY", wayland);
        }
    }
    let run = async {
        let output = command
            .output()
            .await
            .map_err(|_| Error::Unavailable(format!("{program} could not be started")))?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(Error::Failed(format!("{program} failed")))
        }
    };
    tokio::time::timeout(TIMEOUT, run)
        .await
        .map_err(|_| Error::Failed(format!("{program} did not answer in time")))?
}

/// The Hyprland session this daemon can reach: its own environment, or the only running one.
async fn instance() -> Result<Instance, String> {
    if let Some(signature) = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .ok()
        .filter(|value| !value.is_empty())
    {
        return Ok(Instance {
            signature,
            wayland: std::env::var("WAYLAND_DISPLAY").ok(),
        });
    }
    if !which("hyprctl") {
        return Err(if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            "Computer use on Linux needs the Hyprland compositor".into()
        } else {
            "Computer use on Linux needs a Hyprland desktop session".into()
        });
    }
    let listed = output("hyprctl", &["instances", "-j"], None)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let instances = listed
        .as_ref()
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    match instances.as_slice() {
        [only] => Ok(Instance {
            signature: only["instance"].as_str().unwrap_or_default().to_owned(),
            wayland: only["wl_socket"].as_str().map(str::to_owned),
        }),
        [] => Err("No Hyprland session is running for this user".into()),
        _ => {
            Err("Several Hyprland sessions are running; start openside from the one to use".into())
        }
    }
}

async fn hyprctl_json(instance: &Instance, what: &str) -> Result<Value, Error> {
    let bytes = output("hyprctl", &["-j", what], Some(instance)).await?;
    serde_json::from_slice(&bytes)
        .map_err(|_| Error::Failed("hyprctl answered unexpectedly".into()))
}

/// `hyprctl repl` output, or `None` when this Hyprland has no Lua API.
async fn lua(instance: &Instance, script: &str) -> Result<Option<String>, Error> {
    let bytes = output("hyprctl", &["repl", script], Some(instance)).await?;
    let text = String::from_utf8_lossy(&bytes).trim().to_owned();
    Ok((!text.contains("unknown request")).then_some(text))
}

fn valid_address(id: &str) -> bool {
    id.strip_prefix("0x").is_some_and(|hex| {
        !hex.is_empty() && hex.len() <= 16 && hex.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

fn pair(value: &Value) -> Option<(i64, i64)> {
    Some((value.get(0)?.as_i64()?, value.get(1)?.as_i64()?))
}

fn window(client: &Value) -> Option<Window> {
    let (x, y) = pair(&client["at"])?;
    let (width, height) = pair(&client["size"])?;
    let app = client["class"]
        .as_str()
        .filter(|class| !class.is_empty())
        .or_else(|| client["initialClass"].as_str())
        .unwrap_or_default();
    let pid = client["pid"].as_i64()?;
    let program = program(pid);
    Some(Window {
        id: client["address"].as_str()?.to_owned(),
        app: app.to_owned(),
        title: client["title"].as_str().unwrap_or_default().to_owned(),
        pid: u32::try_from(pid).ok()?,
        // Hyprland numbers windows from one counter and never reuses a stable id; the process
        // start time tells a reused pid apart, and the executable an exec in the same process.
        // A window without a stable id or start time is not offered at all.
        instance: format!(
            "{}.{}.{:016x}",
            client["stableId"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| client["stableId"].as_u64().map(|id| id.to_string()))?,
            started(pid)?,
            crate::digest(program.as_deref())
        ),
        program,
        bounds: Rect {
            x: i32::try_from(x).ok()?,
            y: i32::try_from(y).ok()?,
            width: u32::try_from(width).ok()?,
            height: u32::try_from(height).ok()?,
        },
        visible: client["mapped"].as_bool() == Some(true)
            && client["hidden"].as_bool() != Some(true),
    })
}

fn program(pid: i64) -> Option<String> {
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    Some(exe.to_string_lossy().into_owned())
}

/// Field 22 of `/proc/<pid>/stat`: when the process started, in clock ticks since boot.
fn started(pid: i64) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name (field 2) may hold spaces and parentheses; count after its last `)`.
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(19)?.parse().ok()
}

fn contains(x: f64, y: f64, left: f64, top: f64, width: f64, height: f64) -> bool {
    x >= left && y >= top && x < left + width && y < top + height
}

/// Whether something that may be stacked above `target` covers the desktop point: another
/// window that is pinned, floating, or on a shown special workspace over a regular one; or a
/// top/overlay layer surface such as a notification.
fn covered(clients: &Value, layers: &Value, target: &str, x: f64, y: f64) -> bool {
    let Some(target_client) = clients
        .as_array()
        .and_then(|all| all.iter().find(|client| client["address"] == target))
    else {
        return true;
    };
    let special = |client: &Value| client["workspace"]["id"].as_i64().is_some_and(|id| id < 0);
    let windows = clients.as_array().into_iter().flatten().any(|client| {
        if client["address"] == target
            || client["mapped"].as_bool() != Some(true)
            || client["hidden"].as_bool() == Some(true)
            || client["visible"].as_bool() == Some(false)
        {
            return false;
        }
        let Some(((left, top), (width, height))) = pair(&client["at"]).zip(pair(&client["size"]))
        else {
            return false;
        };
        #[allow(clippy::cast_precision_loss)]
        let inside = contains(x, y, left as f64, top as f64, width as f64, height as f64);
        // `clients` carries no stacking order among floating windows, and a floating window on
        // a workspace that is not shown has the same coordinates as one that is, so any other
        // floating window over the point counts as covering it.
        let above = client["pinned"].as_bool() == Some(true)
            || client["floating"].as_bool() == Some(true)
            || (special(client) && !special(target_client));
        inside && above
    });
    let surfaces = layers
        .as_object()
        .into_iter()
        .flat_map(|monitors| monitors.values())
        .filter_map(|monitor| monitor["levels"].as_object())
        .flat_map(|levels| {
            ["2", "3"]
                .into_iter()
                .filter_map(|level| levels.get(level).and_then(Value::as_array))
                .flatten()
        })
        .any(|surface| {
            let number = |key: &str| surface[key].as_f64().unwrap_or(0.0);
            contains(x, y, number("x"), number("y"), number("w"), number("h"))
        });
    windows || surfaces
}

fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    Some((
        u32::from_be_bytes(png[16..20].try_into().ok()?),
        u32::from_be_bytes(png[20..24].try_into().ok()?),
    ))
}

fn keysym(key: NamedKey) -> String {
    match key {
        NamedKey::Return => "Return".into(),
        NamedKey::Tab => "Tab".into(),
        NamedKey::Escape => "Escape".into(),
        NamedKey::Backspace => "BackSpace".into(),
        NamedKey::Delete => "Delete".into(),
        NamedKey::Space => "space".into(),
        NamedKey::Up => "Up".into(),
        NamedKey::Down => "Down".into(),
        NamedKey::Left => "Left".into(),
        NamedKey::Right => "Right".into(),
        NamedKey::Home => "Home".into(),
        NamedKey::End => "End".into(),
        NamedKey::PageUp => "Prior".into(),
        NamedKey::PageDown => "Next".into(),
        NamedKey::Insert => "Insert".into(),
        NamedKey::F(n) => format!("F{n}"),
    }
}

/// `wtype` arguments for a combination: press modifiers, the key, release modifiers.
fn wtype_combo(combo: &KeyCombo) -> Vec<String> {
    let name = |modifier: &Modifier| match modifier {
        Modifier::Ctrl => "ctrl",
        Modifier::Shift => "shift",
        Modifier::Alt => "alt",
        Modifier::Super => "logo",
    };
    let mut args = Vec::new();
    for modifier in &combo.modifiers {
        args.extend(["-M".to_owned(), name(modifier).to_owned()]);
    }
    match combo.key {
        Key::Named(key) => args.extend(["-k".to_owned(), keysym(key)]),
        // A leading dash would read as an option, and `--` would type the releases below.
        Key::Char('-') => args.extend(["-k".to_owned(), "minus".to_owned()]),
        Key::Char(c) => args.push(c.to_string()),
    }
    for modifier in combo.modifiers.iter().rev() {
        args.extend(["-m".to_owned(), name(modifier).to_owned()]);
    }
    args
}

impl Hyprland {
    async fn active(&self, instance: &Instance) -> Result<String, Error> {
        Ok(hyprctl_json(instance, "activewindow").await?["address"]
            .as_str()
            .unwrap_or_default()
            .to_owned())
    }

    async fn ensure_active(&self, instance: &Instance, window: &Window) -> Result<(), Error> {
        if self.active(instance).await? == window.id {
            Ok(())
        } else {
            Err(Error::NotFocused)
        }
    }

    async fn move_cursor(&self, instance: &Instance, x: f64, y: f64) -> Result<(), Error> {
        let (x, y) = (x.round(), y.round());
        let script = format!(
            "local r = hl.dispatch(hl.dsp.cursor.move({{ x = {x}, y = {y} }})); return (r and r.ok) and 'ok' or 'fail'"
        );
        match lua(instance, &script).await? {
            Some(answer) if answer == "ok" => Ok(()),
            Some(_) => Err(Error::Failed("The cursor could not be moved".into())),
            None => output(
                "hyprctl",
                &["dispatch", "movecursor", &format!("{x}"), &format!("{y}")],
                Some(instance),
            )
            .await
            .map(drop),
        }
    }

    async fn pointer(&self) -> Result<(Pointer, u64), Error> {
        let epoch = self.epoch.load(Ordering::SeqCst);
        let existing = self
            .pointer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        match existing {
            Some(pointer) => Ok((pointer, epoch)),
            None => Ok((Pointer::create().await?, epoch)),
        }
    }

    fn keep(&self, pointer: Pointer, epoch: u64) {
        if self.epoch.load(Ordering::SeqCst) == epoch {
            *self
                .pointer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pointer);
        }
    }

    async fn send(&self, instance: &Instance, window: &Window, input: &Input) -> Result<(), Error> {
        match input {
            Input::Click { x, y, .. } | Input::Move { x, y } | Input::Scroll { x, y, .. } => {
                let clients = hyprctl_json(instance, "clients").await?;
                let layers = hyprctl_json(instance, "layers")
                    .await
                    .unwrap_or(Value::Null);
                if covered(&clients, &layers, &window.id, *x, *y) {
                    return Err(Error::Covered);
                }
                self.move_cursor(instance, *x, *y).await?;
                let position = hyprctl_json(instance, "cursorpos").await?;
                let (cx, cy) = (
                    position["x"].as_f64().unwrap_or(f64::NAN),
                    position["y"].as_f64().unwrap_or(f64::NAN),
                );
                let bounds = window.bounds;
                if !contains(
                    cx,
                    cy,
                    f64::from(bounds.x),
                    f64::from(bounds.y),
                    f64::from(bounds.width),
                    f64::from(bounds.height),
                ) {
                    return Err(Error::OutsideWindow);
                }
                // Focus can follow the mouse onto whatever is really under the cursor.
                self.ensure_active(instance, window).await?;
                match input {
                    Input::Click { button, count, .. } => {
                        let (mut pointer, epoch) = self.pointer().await?;
                        // Each press of a double click is checked on its own; one check costs a
                        // few milliseconds, well inside the double-click interval.
                        let mut result = Ok(());
                        for _ in 0..*count {
                            result = match self.ensure_active(instance, window).await {
                                Ok(()) => pointer.click(*button, 1).await,
                                Err(error) => Err(error),
                            };
                            if result.is_err() {
                                break;
                            }
                        }
                        self.keep(pointer, epoch);
                        result
                    }
                    Input::Scroll { dx, dy, .. } => {
                        let (mut pointer, epoch) = self.pointer().await?;
                        self.ensure_active(instance, window).await?;
                        let result = pointer.scroll(*dx, *dy).await;
                        self.keep(pointer, epoch);
                        result
                    }
                    _ => Ok(()),
                }
            }
            Input::Type { text } => {
                for (index, line) in text.split('\n').enumerate() {
                    if index > 0 {
                        self.ensure_active(instance, window).await?;
                        output("wtype", &["-k", "Return"], Some(instance)).await?;
                    }
                    let chars: Vec<char> = line.chars().collect();
                    for chunk in chars.chunks(TYPE_CHUNK) {
                        let piece: String = chunk.iter().collect();
                        self.ensure_active(instance, window).await?;
                        output("wtype", &["--", &piece], Some(instance)).await?;
                    }
                }
                Ok(())
            }
            Input::Key { combo } => {
                self.ensure_active(instance, window).await?;
                let args = wtype_combo(combo);
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                output("wtype", &args, Some(instance)).await.map(drop)
            }
        }
    }
}

impl Backend for Hyprland {
    fn eligibility(&self) -> BoxFuture<'_, Eligibility> {
        Box::pin(async {
            let instance = match instance().await {
                Ok(instance) => instance,
                Err(reason) => return Eligibility::unsupported("linux", &reason),
            };
            if hyprctl_json(&instance, "version").await.is_err() {
                return Eligibility::unsupported(
                    "linux",
                    "openside cannot reach the Hyprland session",
                );
            }
            let capture = if !which("grim") {
                Feature::missing("Install grim to let openside see app windows")
            } else {
                match output("grim", &["-h"], None).await {
                    Ok(help) if String::from_utf8_lossy(&help).contains("-T") => {
                        Feature::via("grim")
                    }
                    _ => Feature::missing("grim 1.5 or newer is needed to capture a single window"),
                }
            };
            let pointer = match uinput::available() {
                Ok(()) => Feature::via("uinput"),
                Err(reason) => Feature::missing(reason),
            };
            let keyboard = if which("wtype") {
                Feature::via("wtype")
            } else {
                Feature::missing("Install wtype to let openside type and press keys")
            };
            Eligibility {
                platform: "linux",
                backend: Some("hyprland"),
                capture,
                pointer,
                keyboard,
            }
        })
    }

    fn windows(&self) -> BoxFuture<'_, Result<Vec<Window>, Error>> {
        Box::pin(async {
            let instance = instance().await.map_err(Error::Unavailable)?;
            let clients = hyprctl_json(&instance, "clients").await?;
            let mut listed: Vec<(i64, Window)> = clients
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|client| {
                    let order = client["focusHistoryID"].as_i64().unwrap_or(i64::MAX);
                    window(client).map(|window| (order, window))
                })
                .collect();
            listed.sort_by_key(|(order, _)| *order);
            Ok(listed.into_iter().map(|(_, window)| window).collect())
        })
    }

    fn capture<'a>(&'a self, window: &'a Window) -> BoxFuture<'a, Result<Frame, Error>> {
        Box::pin(async move {
            let instance = instance().await.map_err(Error::Unavailable)?;
            let clients = hyprctl_json(&instance, "clients").await?;
            let stable = clients
                .as_array()
                .into_iter()
                .flatten()
                .find(|client| client["address"] == window.id.as_str())
                .and_then(|client| client["stableId"].as_str().map(str::to_owned))
                .ok_or(Error::WindowGone)?;
            if !stable.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Error::WindowGone);
            }
            let png = output("grim", &["-T", &stable, "-t", "png", "-"], Some(&instance)).await?;
            let (width, height) = png_size(&png)
                .ok_or_else(|| Error::Failed("The window frame was not a PNG".into()))?;
            Ok(Frame { png, width, height })
        })
    }

    fn focus<'a>(&'a self, window: &'a Window) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if !valid_address(&window.id) {
                return Err(Error::UnknownWindow);
            }
            let instance = instance().await.map_err(Error::Unavailable)?;
            let id = &window.id;
            let script = format!(
                "local s = 'address:{id}' if not hl.get_window(s) then return 'gone' end \
                 local r = hl.dispatch(hl.dsp.focus({{ window = s }})) \
                 if not (r and r.ok) then return 'fail' end \
                 local a = hl.get_active_window() return (a and a.address == '{id}') and 'ok' or 'other'"
            );
            match lua(&instance, &script).await?.as_deref() {
                Some("ok") => return Ok(()),
                Some("gone") => return Err(Error::WindowGone),
                Some(_) => return Err(Error::NotFocused),
                None => {}
            }
            output(
                "hyprctl",
                &["dispatch", "focuswindow", &format!("address:{id}")],
                Some(&instance),
            )
            .await?;
            self.ensure_active(&instance, window).await
        })
    }

    fn input<'a>(
        &'a self,
        window: &'a Window,
        input: &'a Input,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if !valid_address(&window.id) {
                return Err(Error::UnknownWindow);
            }
            let instance = instance().await.map_err(Error::Unavailable)?;
            self.send(&instance, window, input).await
        })
    }

    fn release(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        drop(
            self.pointer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn floating_pinned_special_and_overlay_surfaces_cover_a_tiled_target() {
        let target = json!({"address":"0xa","mapped":true,"hidden":false,"floating":false,"at":[0,0],"size":[800,600],"workspace":{"id":1}});
        let floating = json!({"address":"0xb","mapped":true,"hidden":false,"floating":true,"at":[100,100],"size":[200,200],"workspace":{"id":1}});
        let clients = json!([target, floating]);
        assert!(covered(&clients, &Value::Null, "0xa", 150.0, 150.0));
        assert!(!covered(&clients, &Value::Null, "0xa", 500.0, 500.0));
        let layers = json!({"DP-1":{"levels":{"3":[{"x":600,"y":0,"w":200,"h":100,"namespace":"notifications"}]}}});
        assert!(covered(&clients, &layers, "0xa", 700.0, 50.0));
        assert!(covered(&clients, &Value::Null, "0xmissing", 1.0, 1.0));
    }

    #[test]
    fn any_other_floating_window_over_the_point_covers_a_floating_target() {
        let target = json!({"address":"0xa","mapped":true,"hidden":false,"floating":true,"at":[0,0],"size":[800,600],"workspace":{"id":1}});
        let same = json!({"address":"0xb","mapped":true,"hidden":false,"floating":true,"at":[100,100],"size":[200,200],"workspace":{"id":1}});
        let elsewhere = json!({"address":"0xc","mapped":true,"hidden":false,"floating":true,"at":[400,400],"size":[200,200],"workspace":{"id":2}});
        let tiled = json!({"address":"0xd","mapped":true,"hidden":false,"floating":false,"at":[600,0],"size":[200,200],"workspace":{"id":1}});
        let clients = json!([target, same, elsewhere, tiled]);
        assert!(covered(&clients, &Value::Null, "0xa", 150.0, 150.0));
        assert!(covered(&clients, &Value::Null, "0xa", 500.0, 500.0));
        assert!(!covered(&clients, &Value::Null, "0xa", 700.0, 50.0));
        assert!(!covered(&clients, &Value::Null, "0xa", 50.0, 550.0));
    }

    #[test]
    fn process_identity_comes_from_proc() {
        let pid = i64::from(std::process::id());
        assert!(started(pid).is_some_and(|ticks| ticks > 0));
        assert!(program(pid).is_some_and(|path| path.starts_with('/')));
        assert_eq!(started(i64::from(u32::MAX)), None);
    }

    #[test]
    fn key_combinations_map_to_wtype_and_addresses_are_validated() {
        let combo = KeyCombo::parse("ctrl+shift+Tab").ok();
        assert_eq!(
            combo.map(|combo| wtype_combo(&combo)),
            Some(
                ["-M", "ctrl", "-M", "shift", "-k", "Tab", "-m", "shift", "-m", "ctrl"]
                    .map(String::from)
                    .to_vec()
            )
        );
        assert!(valid_address("0x60af368c3910"));
        assert!(!valid_address("0x1' end os.execute('x"));
        assert!(!valid_address("60af"));
    }
}
