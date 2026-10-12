use std::{path::Path, process::Stdio, time::Duration};

use tokio::{io::AsyncReadExt, process::Command};

use crate::error::{ChannelError, Result};

const MAX_TRANSCRIPT_BYTES: u64 = 64 * 1024;

/// Local speech-to-text hook: argv + audio path, stdout is the transcript.
/// There is deliberately no remote STT path; the audio never leaves the machine
/// through this crate.
#[derive(Clone, Debug)]
pub struct Stt {
    argv: Vec<String>,
    timeout: Duration,
    /// Largest accepted audio attachment in bytes.
    pub max_bytes: u64,
}

impl Stt {
    /// `argv` is run without a shell with the audio file path appended.
    pub fn new(argv: Vec<String>, timeout: Duration, max_bytes: u64) -> Result<Self> {
        if argv
            .first()
            .map_or(true, |program| program.trim().is_empty())
        {
            return Err(ChannelError::Config("stt_cmd needs an executable".into()));
        }
        Ok(Self {
            argv,
            timeout,
            max_bytes,
        })
    }

    /// Transcribe one audio file.
    pub async fn transcribe(&self, audio: &Path) -> Result<String> {
        let mut child = Command::new(&self.argv[0])
            .args(&self.argv[1..])
            .arg(audio)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| ChannelError::Config("stt_cmd could not start".into()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| ChannelError::Io("stt stdout unavailable".into()))?;
        let run = async {
            let mut buffer = Vec::new();
            (&mut stdout)
                .take(MAX_TRANSCRIPT_BYTES)
                .read_to_end(&mut buffer)
                .await?;
            let status = child.wait().await?;
            Ok::<_, std::io::Error>((status, buffer))
        };
        let (status, buffer) = tokio::time::timeout(self.timeout, run)
            .await
            .map_err(|_| ChannelError::Refused("transcription timed out".into()))??;
        if !status.success() {
            return Err(ChannelError::Refused("transcription failed".into()));
        }
        let text = String::from_utf8_lossy(&buffer).trim().to_owned();
        if text.is_empty() {
            return Err(ChannelError::Refused("no speech recognized".into()));
        }
        Ok(text)
    }
}
