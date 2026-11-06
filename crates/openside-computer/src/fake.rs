//! In-memory backend for tests: scripted windows and frames, recorded focus and input.

use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use tokio::sync::Notify;

use crate::{Backend, Eligibility, Error, Feature, Frame, Input, Window};

#[derive(Default)]
struct State {
    windows: Vec<Window>,
    frame: Option<Frame>,
    inputs: Vec<(String, Input)>,
    focused: Option<String>,
    refuse_focus: bool,
    hold_input: bool,
    keyboard: bool,
    releases: usize,
}

/// A desktop held in memory.
#[derive(Clone, Default)]
pub struct Fake {
    state: Arc<Mutex<State>>,
    /// Notified when an input that [`Fake::hold_input`] holds has started.
    pub input_started: Arc<Notify>,
}

impl Fake {
    /// A desktop with these windows, every feature available.
    pub fn new(windows: Vec<Window>) -> Self {
        let fake = Self::default();
        {
            let mut state = fake.lock();
            state.windows = windows;
            state.keyboard = true;
        }
        fake
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Replace the windows.
    pub fn set_windows(&self, windows: Vec<Window>) {
        self.lock().windows = windows;
    }

    /// The PNG every capture returns.
    pub fn set_frame(&self, png: Vec<u8>, width: u32, height: u32) {
        self.lock().frame = Some(Frame { png, width, height });
    }

    /// Make focusing fail, as if another app kept the front.
    pub fn refuse_focus(&self, refuse: bool) {
        self.lock().refuse_focus = refuse;
    }

    /// Make every input wait until cancelled.
    pub fn hold_input(&self, hold: bool) {
        self.lock().hold_input = hold;
    }

    /// Report keyboard input as unavailable.
    pub fn without_keyboard(&self) {
        self.lock().keyboard = false;
    }

    /// Inputs sent so far, with the window each went to.
    pub fn inputs(&self) -> Vec<(String, Input)> {
        self.lock().inputs.clone()
    }

    /// How often per-connection resources were released.
    pub fn releases(&self) -> usize {
        self.lock().releases
    }
}

impl Backend for Fake {
    fn eligibility(&self) -> BoxFuture<'_, Eligibility> {
        Box::pin(async move {
            let keyboard = self.lock().keyboard;
            Eligibility {
                platform: "test",
                backend: Some("fake"),
                capture: Feature::via("fake"),
                pointer: Feature::via("fake"),
                keyboard: if keyboard {
                    Feature::via("fake")
                } else {
                    Feature::missing("No keyboard here")
                },
            }
        })
    }

    fn windows(&self) -> BoxFuture<'_, Result<Vec<Window>, Error>> {
        Box::pin(async move { Ok(self.lock().windows.clone()) })
    }

    fn capture<'a>(&'a self, _: &'a Window) -> BoxFuture<'a, Result<Frame, Error>> {
        Box::pin(async move {
            self.lock()
                .frame
                .clone()
                .ok_or_else(|| Error::Failed("No frame scripted".into()))
        })
    }

    fn focus<'a>(&'a self, window: &'a Window) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let mut state = self.lock();
            if state.refuse_focus {
                return Err(Error::NotFocused);
            }
            state.focused = Some(window.id.clone());
            Ok(())
        })
    }

    fn input<'a>(
        &'a self,
        window: &'a Window,
        input: &'a Input,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let hold = {
                let state = self.lock();
                if state.focused.as_deref() != Some(window.id.as_str()) {
                    return Err(Error::NotFocused);
                }
                state.hold_input
            };
            if hold {
                self.input_started.notify_one();
                std::future::pending::<()>().await;
            }
            self.lock().inputs.push((window.id.clone(), input.clone()));
            Ok(())
        })
    }

    fn release(&self) {
        self.lock().releases += 1;
    }
}
