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

    /// Time allowed for one transcription.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Transcribe one audio file.
    pub async fn transcribe(&self, audio: &Path) -> Result<String> {
        let mut command = Command::new(&self.argv[0]);
        command
            .args(&self.argv[1..])
            .arg(audio)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // Its own process group, so a timeout or a dropped caller also ends whatever the
        // command started (a shell's background job, a helper still holding the clip open).
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|_| ChannelError::Config("stt_cmd could not start".into()))?;
        let mut group = Group(child.id());
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
        // A clean run keeps whatever it deliberately left running (a warm model server).
        group.0 = None;
        let text = String::from_utf8_lossy(&buffer).trim().to_owned();
        if text.is_empty() {
            return Err(ChannelError::Refused("no speech recognized".into()));
        }
        Ok(text)
    }
}

/// Kills the command's process group when transcription ends early: on timeout, on failure,
/// or when the caller drops the future. `kill_on_drop` alone reaches only the leader.
struct Group(Option<u32>);

impl Drop for Group {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.0.and_then(|pid| i32::try_from(pid).ok()) {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(group),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// `sh` starts a background `sleep` and records its pid, then waits on a second one.
    fn forking(root: &Path, timeout: Duration) -> (Stt, std::path::PathBuf) {
        let pid = root.join("pid");
        let script = format!("sleep 30 & echo $! > '{}'; sleep 30", pid.display());
        let stt = Stt::new(
            vec!["sh".into(), "-c".into(), script, "sh".into()],
            timeout,
            1024,
        )
        .unwrap();
        (stt, pid)
    }

    async fn background_pid(path: &Path) -> i32 {
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(pid) = text.trim().parse() {
                    return pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the command never recorded its background pid");
    }

    /// Gone, or a zombie its new parent has yet to reap: either way no longer running.
    async fn assert_ends(pid: i32) {
        for _ in 0..100 {
            let status = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            let state = status
                .rsplit(')')
                .next()
                .and_then(|rest| rest.split_whitespace().next());
            if matches!(state, None | Some("Z") | Some("X")) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("background process {pid} outlived the transcription");
    }

    #[tokio::test]
    async fn a_timeout_kills_the_commands_whole_process_group() {
        let root = tempfile::tempdir().unwrap();
        let (stt, pid) = forking(root.path(), Duration::from_millis(500));
        let audio = root.path().join("clip.ogg");
        std::fs::write(&audio, b"x").unwrap();
        let result = stt.transcribe(&audio).await;
        assert!(
            matches!(result, Err(ChannelError::Refused(reason)) if reason == "transcription timed out")
        );
        assert_ends(background_pid(&pid).await).await;
    }

    #[tokio::test]
    async fn dropping_the_transcription_kills_the_commands_whole_process_group() {
        let root = tempfile::tempdir().unwrap();
        let (stt, pid) = forking(root.path(), Duration::from_secs(60));
        let audio = root.path().join("clip.ogg");
        std::fs::write(&audio, b"x").unwrap();
        // Boxed so that `drop` below drops the future itself, not a pinned reference to it.
        let mut transcription = Box::pin(stt.transcribe(&audio));
        let background = tokio::select! {
            _ = &mut transcription => panic!("the command cannot finish on its own"),
            pid = background_pid(&pid) => pid,
        };
        drop(transcription);
        assert_ends(background).await;
    }
}
