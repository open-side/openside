//! macOS backend.
//!
//! * Eligibility: Screen Recording (`CGPreflightScreenCaptureAccess`) for frames and
//!   Accessibility (`AXIsProcessTrusted`) for input. Neither is requested from here; the person
//!   grants them in System Settings › Privacy & Security.
//! * Windows: `CGWindowListCopyWindowInfo`, on-screen layer-0 windows, front to back; the id is
//!   the `CGWindowID`.
//! * Frames: `screencapture -l <id>` (ScreenCaptureKit), which captures that window alone.
//! * Focus: the owning app's Accessibility element is made frontmost, then the frontmost
//!   layer-0 window must be the connected one.
//! * Input: `CGEvent` mouse, scroll and keyboard events posted to the HID event tap.

// CoreGraphics, CoreFoundation and Accessibility are C APIs without safe std wrappers. Every
// CF object created here is released exactly once; borrowed ("Get") values are never released.
#![allow(unsafe_code)]

use std::ffi::{c_void, CStr};
use std::os::raw::{c_char, c_long};
use std::process::Stdio;
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::process::Command;

use crate::{
    Backend, Button, Eligibility, Error, Feature, Frame, Input, Key, KeyCombo, Modifier, NamedKey,
    Rect, Window,
};

type CFTypeRef = *const c_void;
type CFArrayRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFStringRef = *const c_void;
type CFNumberRef = *const c_void;
type CFIndex = c_long;
type CGEventRef = *mut c_void;
type CGEventSourceRef = *mut c_void;
type AXUIElementRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGRect {
    origin: [f64; 2],
    size: [f64; 2],
}

const K_CG_WINDOW_LIST_ON_SCREEN_ONLY: u32 = 1 << 0;
const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP: u32 = 1 << 4;
const K_CF_NUMBER_SINT64: CFIndex = 4;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CG_HID_EVENT_TAP: u32 = 0;
const K_CG_SCROLL_EVENT_UNIT_LINE: u32 = 1;
const K_CG_MOUSE_EVENT_CLICK_STATE: u32 = 1;
const FLAG_SHIFT: u64 = 0x0002_0000;
const FLAG_CONTROL: u64 = 0x0004_0000;
const FLAG_ALTERNATE: u64 = 0x0008_0000;
const FLAG_COMMAND: u64 = 0x0010_0000;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: CFTypeRef);
    fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: CFArrayRef, index: CFIndex) -> *const c_void;
    fn CFDictionaryGetValue(dictionary: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFNumberGetValue(number: CFNumberRef, kind: CFIndex, value: *mut c_void) -> bool;
    fn CFStringGetCString(
        string: CFStringRef,
        buffer: *mut c_char,
        size: CFIndex,
        encoding: u32,
    ) -> bool;
    static kCFBooleanTrue: CFTypeRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
    fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect) -> bool;
    fn CGEventCreateMouseEvent(
        source: CGEventSourceRef,
        kind: u32,
        point: CGPoint,
        button: u32,
    ) -> CGEventRef;
    fn CGEventCreateScrollWheelEvent2(
        source: CGEventSourceRef,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> CGEventRef;
    fn CGEventCreateKeyboardEvent(source: CGEventSourceRef, key: u16, down: bool) -> CGEventRef;
    fn CGEventKeyboardSetUnicodeString(event: CGEventRef, length: CFIndex, text: *const u16);
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CGEventPostToPid(pid: i32, event: CGEventRef);
    static kCGWindowNumber: CFStringRef;
    static kCGWindowOwnerName: CFStringRef;
    static kCGWindowOwnerPID: CFStringRef;
    static kCGWindowName: CFStringRef;
    static kCGWindowBounds: CFStringRef;
    static kCGWindowLayer: CFStringRef;
}

// libproc lives in libSystem, which every binary links.
extern "C" {
    fn proc_pidpath(pid: i32, buffer: *mut c_void, size: u32) -> i32;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> i32;
    static kAXFrontmostAttribute: CFStringRef;
}

pub(crate) struct Mac;

fn number(dict: CFDictionaryRef, key: CFStringRef) -> Option<i64> {
    let mut value: i64 = 0;
    unsafe {
        let raw = CFDictionaryGetValue(dict, key);
        (!raw.is_null()
            && CFNumberGetValue(raw, K_CF_NUMBER_SINT64, (&mut value as *mut i64).cast()))
        .then_some(value)
    }
}

fn string(dict: CFDictionaryRef, key: CFStringRef) -> Option<String> {
    let mut buffer = [0 as c_char; 1024];
    unsafe {
        let raw = CFDictionaryGetValue(dict, key);
        if raw.is_null()
            || !CFStringGetCString(
                raw,
                buffer.as_mut_ptr(),
                buffer.len() as CFIndex,
                K_CF_STRING_ENCODING_UTF8,
            )
        {
            return None;
        }
        Some(
            CStr::from_ptr(buffer.as_ptr())
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// On-screen layer-0 windows, front to back.
fn list() -> Vec<Window> {
    let mut windows = Vec::new();
    unsafe {
        let array = CGWindowListCopyWindowInfo(
            K_CG_WINDOW_LIST_ON_SCREEN_ONLY | K_CG_WINDOW_LIST_EXCLUDE_DESKTOP,
            0,
        );
        if array.is_null() {
            return windows;
        }
        for index in 0..CFArrayGetCount(array) {
            let dict = CFArrayGetValueAtIndex(array, index);
            if number(dict, kCGWindowLayer) != Some(0) {
                continue;
            }
            let bounds_dict = CFDictionaryGetValue(dict, kCGWindowBounds);
            let mut rect = CGRect::default();
            if bounds_dict.is_null()
                || !CGRectMakeWithDictionaryRepresentation(bounds_dict, &mut rect)
            {
                continue;
            }
            let (Some(id), Some(pid)) = (
                number(dict, kCGWindowNumber),
                number(dict, kCGWindowOwnerPID),
            ) else {
                continue;
            };
            let program = program(pid);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            windows.push(Window {
                id: id.to_string(),
                app: string(dict, kCGWindowOwnerName).unwrap_or_default(),
                title: string(dict, kCGWindowName).unwrap_or_default(),
                pid: u32::try_from(pid).unwrap_or_default(),
                bounds: Rect {
                    x: rect.origin[0].round() as i32,
                    y: rect.origin[1].round() as i32,
                    width: rect.size[0].max(0.0).round() as u32,
                    height: rect.size[1].max(0.0).round() as u32,
                },
                visible: true,
                // Window numbers come from one counter for the login session and are not
                // reused; the executable tells an exec in the same process apart.
                instance: format!("{id}.{:016x}", crate::digest(program.as_deref())),
                program,
            });
        }
        CFRelease(array);
    }
    windows
}

fn post(event: CGEventRef) -> Result<(), Error> {
    if event.is_null() {
        return Err(Error::Failed("The input event could not be created".into()));
    }
    unsafe {
        CGEventPost(K_CG_HID_EVENT_TAP, event);
        CFRelease(event.cast_const());
    }
    Ok(())
}

/// Keyboard events go to the connected app's process rather than to whatever is focused, so a
/// window that takes focus between the frontmost check and the event never receives them.
fn post_to(pid: i32, event: CGEventRef) -> Result<(), Error> {
    if event.is_null() {
        return Err(Error::Failed("The input event could not be created".into()));
    }
    unsafe {
        CGEventPostToPid(pid, event);
        CFRelease(event.cast_const());
    }
    Ok(())
}

fn mouse(kind: u32, point: CGPoint, button: u32, clicks: i64) -> Result<(), Error> {
    unsafe {
        let event = CGEventCreateMouseEvent(std::ptr::null_mut(), kind, point, button);
        if !event.is_null() {
            CGEventSetIntegerValueField(event, K_CG_MOUSE_EVENT_CLICK_STATE, clicks);
        }
        post(event)
    }
}

fn keycode(key: Key) -> Option<u16> {
    Some(match key {
        Key::Named(named) => match named {
            NamedKey::Return => 36,
            NamedKey::Tab => 48,
            NamedKey::Space => 49,
            NamedKey::Backspace => 51,
            NamedKey::Escape => 53,
            NamedKey::Delete => 117,
            NamedKey::Home => 115,
            NamedKey::PageUp => 116,
            NamedKey::End => 119,
            NamedKey::PageDown => 121,
            NamedKey::Left => 123,
            NamedKey::Right => 124,
            NamedKey::Down => 125,
            NamedKey::Up => 126,
            NamedKey::Insert => 114,
            NamedKey::F(n) => [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111]
                .get(usize::from(n.checked_sub(1)?))
                .copied()?,
        },
        Key::Char(c) => match c {
            'a' => 0,
            's' => 1,
            'd' => 2,
            'f' => 3,
            'h' => 4,
            'g' => 5,
            'z' => 6,
            'x' => 7,
            'c' => 8,
            'v' => 9,
            'b' => 11,
            'q' => 12,
            'w' => 13,
            'e' => 14,
            'r' => 15,
            'y' => 16,
            't' => 17,
            '1' => 18,
            '2' => 19,
            '3' => 20,
            '4' => 21,
            '6' => 22,
            '5' => 23,
            '=' => 24,
            '9' => 25,
            '7' => 26,
            '-' => 27,
            '8' => 28,
            '0' => 29,
            ']' => 30,
            'o' => 31,
            'u' => 32,
            '[' => 33,
            'i' => 34,
            'p' => 35,
            'l' => 37,
            'j' => 38,
            '\'' => 39,
            'k' => 40,
            ';' => 41,
            '\\' => 42,
            ',' => 43,
            '/' => 44,
            'n' => 45,
            'm' => 46,
            '.' => 47,
            '`' => 50,
            _ => return None,
        },
    })
}

fn key(pid: i32, combo: &KeyCombo) -> Result<(), Error> {
    let code = keycode(combo.key)
        .ok_or_else(|| Error::Invalid("That key cannot be pressed on macOS".into()))?;
    let flags = combo.modifiers.iter().fold(0, |flags, modifier| {
        flags
            | match modifier {
                Modifier::Shift => FLAG_SHIFT,
                Modifier::Ctrl => FLAG_CONTROL,
                Modifier::Alt => FLAG_ALTERNATE,
                Modifier::Super => FLAG_COMMAND,
            }
    });
    for down in [true, false] {
        unsafe {
            let event = CGEventCreateKeyboardEvent(std::ptr::null_mut(), code, down);
            if !event.is_null() {
                CGEventSetFlags(event, flags);
            }
            post_to(pid, event)?;
        }
    }
    Ok(())
}

/// Types in chunks, re-checking before each that `window` is still frontmost and yielding after
/// each so that Stop, which drops this future, takes effect between chunks.
async fn type_text(window: &str, pid: i32, text: &str) -> Result<(), Error> {
    let units: Vec<u16> = text.encode_utf16().collect();
    // CGEventKeyboardSetUnicodeString accepts at most 20 UTF-16 units per event.
    for chunk in units.chunks(20) {
        if front_window().as_deref() != Some(window) {
            return Err(Error::NotFocused);
        }
        for down in [true, false] {
            unsafe {
                let event = CGEventCreateKeyboardEvent(std::ptr::null_mut(), 0, down);
                if !event.is_null() {
                    CGEventKeyboardSetUnicodeString(event, chunk.len() as CFIndex, chunk.as_ptr());
                }
                post_to(pid, event)?;
            }
        }
        tokio::task::yield_now().await;
    }
    Ok(())
}

/// The executable of `pid` as the kernel reports it.
fn program(pid: i64) -> Option<String> {
    // PROC_PIDPATHINFO_MAXSIZE
    let mut buffer = vec![0_u8; 4096];
    let length = unsafe {
        proc_pidpath(
            i32::try_from(pid).ok()?,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
        )
    };
    buffer.truncate(usize::try_from(length).ok().filter(|length| *length > 0)?);
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

fn front_window() -> Option<String> {
    list().into_iter().next().map(|window| window.id)
}

impl Backend for Mac {
    fn eligibility(&self) -> BoxFuture<'_, Eligibility> {
        Box::pin(async {
            let (screen, accessibility) =
                unsafe { (CGPreflightScreenCaptureAccess(), AXIsProcessTrusted()) };
            let capture = if screen {
                Feature::via("screencapturekit")
            } else {
                Feature::missing(
                    "Allow openside under System Settings › Privacy & Security › Screen Recording",
                )
            };
            let input = |via: &str| {
                if accessibility {
                    Feature::via(via)
                } else {
                    Feature::missing(
                        "Allow openside under System Settings › Privacy & Security › Accessibility",
                    )
                }
            };
            Eligibility {
                platform: "macos",
                backend: Some("macos"),
                capture,
                pointer: input("cgevent"),
                keyboard: input("cgevent"),
            }
        })
    }

    fn windows(&self) -> BoxFuture<'_, Result<Vec<Window>, Error>> {
        Box::pin(async { Ok(list()) })
    }

    fn capture<'a>(&'a self, window: &'a Window) -> BoxFuture<'a, Result<Frame, Error>> {
        Box::pin(async move {
            if !window.id.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Error::UnknownWindow);
            }
            // A fresh 0700 directory per capture: creating it fails if anything already sits
            // at that name, so a planted file or symlink cannot receive the frame.
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "openside-frame-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_nanos()),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&directory)
                    .map_err(|_| Error::Failed("No private place to capture the window".into()))?;
            }
            let path = directory.join("frame.png");
            let status = Command::new("/usr/sbin/screencapture")
                .args(["-x", "-o", "-t", "png", "-l", &window.id])
                .arg(&path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status();
            let status = tokio::time::timeout(Duration::from_secs(10), status).await;
            let png = tokio::fs::read(&path).await;
            let _ = tokio::fs::remove_dir_all(&directory).await;
            let status = status
                .map_err(|_| Error::Failed("The window capture did not finish in time".into()))?
                .map_err(|_| Error::Failed("screencapture could not be started".into()))?;
            let png = match (status.success(), png) {
                (true, Ok(png)) => png,
                _ => return Err(Error::Failed("The window could not be captured".into())),
            };
            if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
                return Err(Error::Failed("The window frame was not a PNG".into()));
            }
            let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
            let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
            Ok(Frame { png, width, height })
        })
    }

    fn focus<'a>(&'a self, window: &'a Window) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let pid = i32::try_from(window.pid).map_err(|_| Error::UnknownWindow)?;
            let status = unsafe {
                let app = AXUIElementCreateApplication(pid);
                if app.is_null() {
                    return Err(Error::NotFocused);
                }
                let status =
                    AXUIElementSetAttributeValue(app, kAXFrontmostAttribute, kCFBooleanTrue);
                CFRelease(app);
                status
            };
            // kAXErrorAPIDisabled: Accessibility was revoked after the eligibility probe.
            match status {
                0 => {}
                -25211 => {
                    return Err(Error::Unavailable(
                        "Allow openside in System Settings > Privacy & Security > Accessibility"
                            .into(),
                    ))
                }
                _ => return Err(Error::NotFocused),
            }
            // Activation is asynchronous; give the window server a moment to reorder.
            for _ in 0..10 {
                if front_window().as_deref() == Some(window.id.as_str()) {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(Error::NotFocused)
        })
    }

    fn input<'a>(
        &'a self,
        window: &'a Window,
        input: &'a Input,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            // The frontmost window owns every point inside its own bounds.
            if front_window().as_deref() != Some(window.id.as_str()) {
                return Err(Error::NotFocused);
            }
            match input {
                Input::Click {
                    x,
                    y,
                    button,
                    count,
                } => {
                    let point = CGPoint { x: *x, y: *y };
                    let (down, up, code) = match button {
                        Button::Left => (1, 2, 0),
                        Button::Right => (3, 4, 1),
                        Button::Middle => (25, 26, 2),
                    };
                    mouse(5, point, 0, 0)?;
                    for click in 1..=i64::from(*count) {
                        // A press and its release stay together; the window is checked before.
                        if front_window().as_deref() != Some(window.id.as_str()) {
                            return Err(Error::NotFocused);
                        }
                        mouse(down, point, code, click)?;
                        mouse(up, point, code, click)?;
                    }
                    Ok(())
                }
                Input::Move { x, y } => mouse(5, CGPoint { x: *x, y: *y }, 0, 0),
                Input::Scroll { x, y, dx, dy } => {
                    mouse(5, CGPoint { x: *x, y: *y }, 0, 0)?;
                    // Moving the pointer can raise another app; check again before scrolling.
                    if front_window().as_deref() != Some(window.id.as_str()) {
                        return Err(Error::NotFocused);
                    }
                    let event = unsafe {
                        CGEventCreateScrollWheelEvent2(
                            std::ptr::null_mut(),
                            K_CG_SCROLL_EVENT_UNIT_LINE,
                            2,
                            -*dy,
                            -*dx,
                            0,
                        )
                    };
                    post(event)
                }
                Input::Type { text } => {
                    let pid = i32::try_from(window.pid).map_err(|_| Error::UnknownWindow)?;
                    type_text(&window.id, pid, text).await
                }
                Input::Key { combo } => {
                    let pid = i32::try_from(window.pid).map_err(|_| Error::UnknownWindow)?;
                    key(pid, combo)
                }
            }
        })
    }
}
