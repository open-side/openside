//! A virtual pointer through `/dev/uinput`, for button presses and wheel turns. Positioning is
//! the compositor's job (an absolute cursor move), so the device only reports buttons, wheels
//! and the relative axes libinput needs to classify it as a mouse.

// uinput is configured with ioctls, which have no safe wrapper in std; each call below passes
// a valid open descriptor and, for UI_DEV_SETUP, a fully initialised repr(C) struct.
#![allow(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::time::Duration;

use crate::{Button, Error};

const PATH: &str = "/dev/uinput";

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const SYN_REPORT: u16 = 0;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_HWHEEL: u16 = 0x06;
const REL_WHEEL: u16 = 0x08;
const BUS_VIRTUAL: u16 = 0x06;

#[repr(C)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [u8; 80],
    ff_effects_max: u32,
}

nix::ioctl_none!(ui_dev_create, b'U', 1);
nix::ioctl_none!(ui_dev_destroy, b'U', 2);
nix::ioctl_write_ptr!(ui_dev_setup, b'U', 3, UinputSetup);
nix::ioctl_write_int!(ui_set_evbit, b'U', 100);
nix::ioctl_write_int!(ui_set_keybit, b'U', 101);
nix::ioctl_write_int!(ui_set_relbit, b'U', 102);

/// Whether this process may create a uinput device.
pub(crate) fn available() -> Result<(), String> {
    if cfg!(not(target_pointer_width = "64")) {
        return Err("Pointer input needs a 64-bit system".into());
    }
    match OpenOptions::new().write(true).open(PATH) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err("The uinput kernel module is not loaded, so clicks cannot be sent".into())
        }
        Err(_) => Err(
            "openside may not write to /dev/uinput, so clicks cannot be sent; add your user to the input group or grant access with a udev rule"
                .into(),
        ),
    }
}

/// A created virtual pointer; destroyed on drop.
pub(crate) struct Pointer {
    file: File,
}

impl Pointer {
    /// Create the device and wait until the compositor can have picked it up.
    pub(crate) async fn create() -> Result<Self, Error> {
        let failed = |_| Error::Failed("The virtual pointer could not be created".into());
        let file = OpenOptions::new()
            .write(true)
            .custom_flags(nix::fcntl::OFlag::O_NONBLOCK.bits())
            .open(PATH)
            .map_err(failed)?;
        let fd = file.as_raw_fd();
        let mut name = [0u8; 80];
        let label = b"openside computer use pointer";
        name[..label.len()].copy_from_slice(label);
        let setup = UinputSetup {
            id: InputId {
                bustype: BUS_VIRTUAL,
                vendor: 0x1d6b,
                product: 0x0104,
                version: 1,
            },
            name,
            ff_effects_max: 0,
        };
        let configured = unsafe {
            ui_set_evbit(fd, EV_KEY.into())
                .and_then(|_| ui_set_evbit(fd, EV_REL.into()))
                .and_then(|_| ui_set_evbit(fd, EV_SYN.into()))
                .and_then(|_| ui_set_keybit(fd, BTN_LEFT.into()))
                .and_then(|_| ui_set_keybit(fd, BTN_RIGHT.into()))
                .and_then(|_| ui_set_keybit(fd, BTN_MIDDLE.into()))
                .and_then(|_| ui_set_relbit(fd, REL_X.into()))
                .and_then(|_| ui_set_relbit(fd, REL_Y.into()))
                .and_then(|_| ui_set_relbit(fd, REL_WHEEL.into()))
                .and_then(|_| ui_set_relbit(fd, REL_HWHEEL.into()))
                .and_then(|_| ui_dev_setup(fd, &setup))
                .and_then(|_| ui_dev_create(fd))
        };
        configured.map_err(|_| Error::Failed("The virtual pointer could not be created".into()))?;
        // libinput enumerates new devices asynchronously; events sent before that are lost.
        tokio::time::sleep(Duration::from_millis(250)).await;
        Ok(Self { file })
    }

    fn emit(&mut self, kind: u16, code: u16, value: i32) -> Result<(), Error> {
        // struct input_event on 64-bit Linux: timeval (two i64, ignored by uinput), u16, u16, i32.
        let mut event = [0u8; 24];
        event[16..18].copy_from_slice(&kind.to_ne_bytes());
        event[18..20].copy_from_slice(&code.to_ne_bytes());
        event[20..24].copy_from_slice(&value.to_ne_bytes());
        self.file
            .write_all(&event)
            .map_err(|_| Error::Failed("The virtual pointer stopped working".into()))
    }

    fn sync(&mut self) -> Result<(), Error> {
        self.emit(EV_SYN, SYN_REPORT, 0)
    }

    /// Press and release `button` `count` times at the current cursor position. Each press is
    /// released before the next await, so a cancelled action never leaves a button held.
    pub(crate) async fn click(&mut self, button: Button, count: u8) -> Result<(), Error> {
        let code = match button {
            Button::Left => BTN_LEFT,
            Button::Right => BTN_RIGHT,
            Button::Middle => BTN_MIDDLE,
        };
        for index in 0..count {
            if index > 0 {
                tokio::time::sleep(Duration::from_millis(60)).await;
            }
            let pressed = self.emit(EV_KEY, code, 1).and_then(|()| self.sync());
            let released = self.emit(EV_KEY, code, 0).and_then(|()| self.sync());
            pressed.and(released)?;
        }
        Ok(())
    }

    /// Turn the wheels; positive `dy` scrolls down, positive `dx` right.
    pub(crate) async fn scroll(&mut self, dx: i32, dy: i32) -> Result<(), Error> {
        for _ in 0..dy.unsigned_abs() {
            self.emit(EV_REL, REL_WHEEL, if dy > 0 { -1 } else { 1 })?;
            self.sync()?;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        for _ in 0..dx.unsigned_abs() {
            self.emit(EV_REL, REL_HWHEEL, if dx > 0 { 1 } else { -1 })?;
            self.sync()?;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }
}

impl Drop for Pointer {
    fn drop(&mut self) {
        let fd = self.file.as_raw_fd();
        let _ = unsafe { ui_dev_destroy(fd) };
    }
}
