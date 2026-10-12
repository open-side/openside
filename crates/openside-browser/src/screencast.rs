//! Live JPEG screencast of one agent-owned tab, for remote human takeover.
//!
//! Chromium has bounded in-flight frame credits. Takeover eagerly acknowledges
//! replacements for low latency; scaled preview keeps their IDs pending and acknowledges only
//! one per consumer tick, so repainting cannot bypass preview's capture rate limit.

use std::collections::HashMap;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::Notify;

use super::{Browser, Inner};
use crate::BrowserError;

/// One JPEG frame of the tab's 1440x900 viewport.
#[derive(Clone, Debug)]
pub struct ScreencastFrame {
    /// Base64 JPEG exactly as Chromium encoded it.
    pub data: String,
    /// Viewport size in CSS pixels that the frame shows; input coordinates use this space.
    pub viewport: (f64, f64),
    ack: i64,
}

#[derive(Default)]
pub(super) struct Slot {
    frame: Mutex<Option<ScreencastFrame>>,
    ready: Notify,
    policy: Mutex<AckPolicy>,
    #[cfg(test)]
    received: AtomicUsize,
    #[cfg(test)]
    acknowledgements: AtomicUsize,
}

#[derive(Default)]
enum AckPolicy {
    #[default]
    Eager,
    Paced {
        deferred: [Option<i64>; 3],
    },
}

impl Slot {
    fn replace(&self, frame: ScreencastFrame) -> Option<i64> {
        #[cfg(test)]
        self.received.fetch_add(1, Ordering::Relaxed);
        let replaced = lock(&self.frame).replace(frame)?;
        match &mut *lock(&self.policy) {
            AckPolicy::Eager => Some(replaced.ack),
            AckPolicy::Paced { deferred } => {
                if let Some(pending) = deferred.iter_mut().find(|pending| pending.is_none()) {
                    *pending = Some(replaced.ack);
                }
                None
            }
        }
    }

    fn acknowledge(&self, frame: i64) -> i64 {
        #[cfg(test)]
        self.acknowledgements.fetch_add(1, Ordering::Relaxed);
        match &mut *lock(&self.policy) {
            AckPolicy::Eager => frame,
            AckPolicy::Paced { deferred } => {
                let previous = deferred[0].take();
                deferred.rotate_left(1);
                if previous.is_some() {
                    if let Some(pending) = deferred.iter_mut().find(|pending| pending.is_none()) {
                        *pending = Some(frame);
                    }
                }
                previous.unwrap_or(frame)
            }
        }
    }
}

/// Active screencasts by CDP session id.
pub(super) type Slots = Mutex<HashMap<String, Arc<Slot>>>;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn ack(inner: &Inner, session: &str, frame: i64) {
    inner.cdp.send(
        Some(session),
        "Page.screencastFrameAck",
        json!({"sessionId": frame}),
    );
}

/// Route one `Page.screencastFrame` event to its consumer, or acknowledge it if it has none.
pub(super) fn deliver(inner: &Inner, event: &Value) {
    let Some(session) = event["sessionId"].as_str() else {
        return;
    };
    let params = &event["params"];
    let Some(id) = params["sessionId"].as_i64() else {
        return;
    };
    let slot = lock(&inner.screencasts).get(session).cloned();
    let (Some(slot), Some(data)) = (slot, params["data"].as_str()) else {
        ack(inner, session, id);
        return;
    };
    let metadata = &params["metadata"];
    let frame = ScreencastFrame {
        data: data.to_owned(),
        viewport: (
            metadata["deviceWidth"]
                .as_f64()
                .unwrap_or(f64::from(super::VIEWPORT.0)),
            metadata["deviceHeight"]
                .as_f64()
                .unwrap_or(f64::from(super::VIEWPORT.1)),
        ),
        ack: id,
    };
    if let Some(replaced) = slot.replace(frame) {
        #[cfg(test)]
        slot.acknowledgements.fetch_add(1, Ordering::Relaxed);
        ack(inner, session, replaced);
    }
    slot.ready.notify_one();
}

/// A running screencast; [`Screencast::stop`] or dropping it stops the stream and the focus
/// emulation it enabled.
pub struct Screencast {
    inner: Arc<Inner>,
    session: String,
    slot: Arc<Slot>,
    stopped: AtomicBool,
}

impl Screencast {
    /// Wait for the next frame. Chromium sends no further frame until this one is passed to
    /// [`Screencast::ack`], so pacing the acknowledgement paces the stream.
    pub async fn next(&self) -> ScreencastFrame {
        if self.stopped.load(Ordering::Acquire) {
            return std::future::pending().await;
        }
        loop {
            let notified = self.slot.ready.notified();
            if let Some(frame) = lock(&self.slot.frame).take() {
                return frame;
            }
            notified.await;
        }
    }

    /// Release Chromium to produce the frame after `frame`.
    pub fn ack(&self, frame: &ScreencastFrame) {
        if !self.stopped.load(Ordering::Acquire) {
            ack(&self.inner, &self.session, self.slot.acknowledge(frame.ack));
        }
    }

    /// Stop capturing now, while other holders may still reference this screencast. Pending
    /// and later frames are discarded; [`Screencast::next`] then never returns. Idempotent.
    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        {
            let mut slots = lock(&self.inner.screencasts);
            if slots
                .get(&self.session)
                .is_some_and(|slot| Arc::ptr_eq(slot, &self.slot))
            {
                slots.remove(&self.session);
            }
        }
        lock(&self.slot.frame).take();
        self.inner
            .cdp
            .send(Some(&self.session), "Page.stopScreencast", json!({}));
        self.inner.cdp.send(
            Some(&self.session),
            "Emulation.setFocusEmulationEnabled",
            json!({"enabled": false}),
        );
    }
}

impl Drop for Screencast {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Browser {
    /// Start a JPEG screencast of an explicitly owned agent tab. One screencast per tab; the
    /// tab keeps emulated focus meanwhile so forwarded keyboard input reaches its page even
    /// though agent windows are never activated.
    pub async fn screencast(
        &self,
        owner: &str,
        tab_id: &str,
        quality: u8,
    ) -> Result<Screencast, BrowserError> {
        self.start_screencast(owner, tab_id, quality, super::VIEWPORT, false)
            .await
    }

    /// Capture within smaller JPEG bounds, pacing replacement ACKs with consumer ACKs.
    pub async fn screencast_scaled(
        &self,
        owner: &str,
        tab_id: &str,
        quality: u8,
        max: (u32, u32),
    ) -> Result<Screencast, BrowserError> {
        self.start_screencast(owner, tab_id, quality, max, true)
            .await
    }

    async fn start_screencast(
        &self,
        owner: &str,
        tab_id: &str,
        quality: u8,
        max: (u32, u32),
        paced: bool,
    ) -> Result<Screencast, BrowserError> {
        let session = self.owned_session(owner, tab_id).await?;
        let slot = Arc::new(Slot::default());
        if paced {
            *lock(&slot.policy) = AckPolicy::Paced {
                deferred: [None; 3],
            };
        }
        {
            let mut slots = lock(&self.inner.screencasts);
            if slots.contains_key(&session) {
                return Err(BrowserError::InvalidAction("screencast already active"));
            }
            slots.insert(session.clone(), Arc::clone(&slot));
        }
        // Constructed before any command so every failure path stops and unregisters.
        let screencast = Screencast {
            inner: Arc::clone(&self.inner),
            session,
            slot,
            stopped: AtomicBool::new(false),
        };
        let cdp = &self.inner.cdp;
        cdp.call(
            Some(&screencast.session),
            "Emulation.setFocusEmulationEnabled",
            json!({"enabled": true}),
        )
        .await?;
        let mut parameters = json!({
            "format": "jpeg",
            "quality": quality.min(100),
            "maxWidth": max.0.min(super::VIEWPORT.0),
            "maxHeight": max.1.min(super::VIEWPORT.1),
            "everyNthFrame": 1,
        });
        if paced {
            parameters["maxFramesInFlight"] = json!(1);
        }
        cdp.call(
            Some(&screencast.session),
            "Page.startScreencast",
            parameters,
        )
        .await?;
        Ok(screencast)
    }

    /// Retire every element reference handed out for this tab: a person changed the page out
    /// of band, so an old `eN` must fail as stale rather than act on a node that merely
    /// survived, and later snapshots number nodes afresh. Unknown tabs are a no-op.
    pub async fn invalidate_references(&self, tab_id: &str) {
        if let Some(references) = self.inner.state.lock().await.references.get_mut(tab_id) {
            references.void();
        }
    }
}

#[cfg(test)]
#[path = "screencast_tests.rs"]
mod tests;
