use std::{future::Future, path::Path};

use crate::{
    error::Result,
    model::{AudioRef, Button, MsgRef, Press, ThreadKey},
};

/// The outbound half of a chat platform. Adapters implement it; [`crate::Core`] holds
/// everything platform-independent (thread mapping, commands, streaming, approvals).
pub trait Messenger: Send + Sync + 'static {
    /// `telegram`, `slack` or `discord`.
    const PLATFORM: &'static str;

    /// Longest text one message may carry.
    fn max_chars(&self) -> usize;

    /// Post a message in the thread, with optional inline buttons.
    fn send(
        &self,
        thread: &ThreadKey,
        text: &str,
        buttons: &[Button],
    ) -> impl Future<Output = Result<MsgRef>> + Send;

    /// Replace the text and buttons of a message the bot posted (no buttons removes them).
    fn edit(
        &self,
        thread: &ThreadKey,
        message: &MsgRef,
        text: &str,
        buttons: &[Button],
    ) -> impl Future<Output = Result<()>> + Send;

    /// Download an audio attachment to `dest`, refusing more than `max_bytes`.
    fn download_audio(
        &self,
        audio: &AudioRef,
        dest: &Path,
        max_bytes: u64,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Acknowledge a button press where the platform needs an explicit call
    /// (Telegram `answerCallbackQuery`); HTTP-callback platforms answer in the response.
    fn answer_press(&self, _press: &Press, _note: &str) -> impl Future<Output = ()> + Send {
        async {}
    }
}
