use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::SocketAddr as UnixSocketAddr;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use openside_core::{
    AgentRuntime, AgentSession, ContentPart, HostToolHandler, MessageId, QueueMode, RuntimeError,
    RuntimeEvent, SessionInfo, SessionSpec, SubagentControl, TicketId, TicketResolution,
};
use parking_lot::Mutex;
use rand::RngCore;
use tempfile::TempDir;
use tokio::io::AsyncRead;
use tokio::net::UnixListener;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

use crate::framing::Lines;
use crate::{Connection, SidecarConfig, SidecarRuntime};

const STDERR_LINE_BYTES: usize = 2048;
const STDERR_LINES: usize = 64;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

type StderrTail = Arc<Mutex<VecDeque<String>>>;

struct Process {
    child: Child,
    group: Option<Pid>,
    stderr: StderrTail,
    stderr_task: Option<JoinHandle<()>>,
    directory: TempDir,
}

impl Process {
    fn kill_group(&mut self) {
        if let Some(group) = self.group.take() {
            let _ = killpg(group, Signal::SIGKILL);
        }
        let _ = self.child.start_kill();
    }

    async fn finish(&mut self) {
        self.kill_group();
        let _ = self.child.wait().await;
        // SIGKILL also closes inherited stderr in descendants, so this drain is finite.
        if let Some(task) = self.stderr_task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.kill_group();
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

/// One ready sidecar process, connection, and bounded stderr diagnostic buffer.
///
/// After exit, the reader has at most five seconds to drain final observations.
/// Buffered events, including the final snapshot, remain available after shutdown.
pub struct SidecarSession {
    connection: Connection,
    process: Process,
    ended: bool,
    exit_deadline: Option<tokio::time::Instant>,
}

impl SidecarSession {
    /// Latest at most 64 stderr lines, each at most 2 KiB of UTF-8 text.
    ///
    /// The sidecar must sanitize diagnostics before writing them; the daemon never
    /// formats or logs this buffer implicitly.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.process.stderr.lock().iter().cloned().collect()
    }
}

impl AgentRuntime for SidecarRuntime {
    type Session = SidecarSession;

    async fn open_session(
        &self,
        spec: SessionSpec,
        tools: Arc<dyn HostToolHandler>,
    ) -> Result<Self::Session, RuntimeError> {
        let started = Instant::now();
        let deadline = tokio::time::Instant::now() + self.config.startup_timeout;
        let (listener, mut process, nonce) = spawn(&self.config)?;
        let startup = async {
            let (socket, _) = tokio::select! {
                biased;
                accepted = listener.accept() => accepted.map_err(|_| launch_error("could not accept sidecar connection"))?,
                _ = process.child.wait() => return Err(launch_error("sidecar exited during startup")),
            };
            fs::remove_file(process.directory.path().join("rpc.sock"))
                .map_err(|_| launch_error("could not remove accepted sidecar socket"))?;
            Connection::open(socket, &nonce, spec, tools).await
        };
        let result = tokio::time::timeout_at(deadline, startup)
            .await
            .unwrap_or_else(|_| Err(launch_error("sidecar startup timed out")));
        match result {
            Ok(mut connection) => {
                connection
                    .set_startup_ms(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX));
                Ok(SidecarSession {
                    connection,
                    process,
                    ended: false,
                    exit_deadline: None,
                })
            }
            Err(error) => {
                process.finish().await;
                Err(error)
            }
        }
    }
}

impl AgentSession for SidecarSession {
    fn info(&self) -> &SessionInfo {
        self.connection.info()
    }

    async fn send(
        &mut self,
        id: MessageId,
        text: String,
        mode: QueueMode,
    ) -> Result<(), RuntimeError> {
        self.connection.send(id, text, mode).await
    }

    async fn send_with_parts(
        &mut self,
        id: MessageId,
        text: String,
        parts: Vec<ContentPart>,
        mode: QueueMode,
        author_kind: openside_core::MessageAuthorKind,
    ) -> Result<(), RuntimeError> {
        self.connection
            .send_with_parts(id, text, parts, mode, author_kind)
            .await
    }

    async fn abort(&mut self) -> Result<(), RuntimeError> {
        self.connection.abort().await
    }

    async fn resolve_ticket(
        &mut self,
        ticket: TicketId,
        tool_call_id: String,
        resolution: TicketResolution,
    ) -> Result<(), RuntimeError> {
        self.connection
            .resolve_ticket(ticket, tool_call_id, resolution)
            .await
    }

    async fn control_subagent(
        &mut self,
        subagent_id: String,
        control: SubagentControl,
    ) -> Result<(), RuntimeError> {
        self.connection.control_subagent(subagent_id, control).await
    }

    async fn discard_deferred(&mut self) -> Result<(), RuntimeError> {
        self.connection.discard_deferred().await
    }

    async fn next_event(&mut self) -> Option<RuntimeEvent> {
        if self.ended {
            return self.connection.try_next_event();
        }
        let event = if let Some(deadline) = self.exit_deadline {
            tokio::time::timeout_at(deadline, self.connection.next_event())
                .await
                .unwrap_or(None)
        } else {
            tokio::select! {
                biased;
                event = self.connection.next_event() => event,
                _ = self.process.child.wait() => {
                    // The final fatal error or snapshot may still be in the socket buffer.
                    let deadline = tokio::time::Instant::now() + SHUTDOWN_TIMEOUT;
                    self.exit_deadline = Some(deadline);
                    tokio::time::timeout_at(deadline, self.connection.next_event()).await.unwrap_or(None)
                },
            }
        };
        if event.is_none() {
            self.ended = true;
            self.connection.close();
        }
        event
    }

    async fn shutdown(&mut self) -> Result<(), RuntimeError> {
        if self.process.group.is_none() {
            return Ok(());
        }
        let result = tokio::time::timeout(SHUTDOWN_TIMEOUT, async {
            let send = self.connection.request_shutdown().await;
            let _ = tokio::join!(self.process.child.wait(), self.connection.wait_closed());
            send
        })
        .await;
        self.connection.close();
        self.process.finish().await;
        self.ended = true;
        match result {
            Ok(result) => result,
            // Forced termination is the specified shutdown fallback, not a failure.
            Err(_) => Ok(()),
        }
    }
}

fn launch_error(message: &str) -> RuntimeError {
    RuntimeError::Unavailable(message.into())
}

fn private_socket_directory(runtime_dir: &Path) -> Result<TempDir, RuntimeError> {
    // Reserve the randomized suffix and filename before choosing a parent; Darwin has a
    // shorter sun_path than Linux, and the configured runtime directory can be arbitrarily deep.
    let candidate = runtime_dir.join("sidecar-000000/rpc.sock");
    let parent = if UnixSocketAddr::from_pathname(&candidate).is_ok() {
        runtime_dir
    } else {
        Path::new("/tmp")
    };
    fs::create_dir_all(parent).map_err(|_| launch_error("could not create runtime directory"))?;
    let directory = tempfile::Builder::new()
        .prefix("sidecar-")
        .rand_bytes(6)
        .tempdir_in(parent)
        .map_err(|_| launch_error("could not create private socket directory"))?;
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
        .map_err(|_| launch_error("could not secure private socket directory"))?;
    Ok(directory)
}

#[allow(unsafe_code)]
fn protect_child(command: &mut Command) {
    // The closure runs after fork: only async-signal-safe syscalls, no allocation or locks.
    unsafe {
        command.pre_exec(|| {
            nix::sys::resource::setrlimit(nix::sys::resource::Resource::RLIMIT_CORE, 0, 0)
                .map_err(|error| std::io::Error::from_raw_os_error(error as i32))?;
            #[cfg(target_os = "linux")]
            nix::sys::prctl::set_dumpable(false)
                .map_err(|error| std::io::Error::from_raw_os_error(error as i32))?;
            Ok(())
        });
    }
}

fn spawn(config: &SidecarConfig) -> Result<(UnixListener, Process, String), RuntimeError> {
    let (program, args) = config
        .argv
        .split_first()
        .ok_or_else(|| launch_error("sidecar command is empty"))?;
    let directory = private_socket_directory(&config.runtime_dir)?;
    let path = directory.path().join("rpc.sock");
    let listener =
        UnixListener::bind(&path).map_err(|_| launch_error("could not bind sidecar socket"))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|_| launch_error("could not secure sidecar socket"))?;
    let mut random = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut random);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut nonce = String::with_capacity(32);
    for byte in random {
        nonce.push(HEX[usize::from(byte >> 4)] as char);
        nonce.push(HEX[usize::from(byte & 15)] as char);
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .envs(config.env.iter().map(|(key, value)| (key, value)))
        .env("OPENSIDE_SIDECAR_SOCKET", &path)
        .env("OPENSIDE_SIDECAR_NONCE", &nonce)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    protect_child(&mut command);
    let mut child = command
        .spawn()
        .map_err(|_| launch_error("could not spawn sidecar command"))?;
    let group = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .map(Pid::from_raw);
    let Some(stderr) = child.stderr.take() else {
        if let Some(group) = group {
            let _ = killpg(group, Signal::SIGKILL);
        }
        return Err(launch_error("could not capture sidecar stderr"));
    };
    let tail: StderrTail = Arc::default();
    let stderr_tail = tail.clone();
    let stderr_task = tokio::spawn(capture_stderr(stderr, stderr_tail));
    Ok((
        listener,
        Process {
            child,
            group,
            stderr: tail,
            stderr_task: Some(stderr_task),
            directory,
        },
        nonce,
    ))
}

async fn capture_stderr<R: AsyncRead + Unpin>(read: R, tail: StderrTail) {
    let mut lines = Lines::new(read, STDERR_LINE_BYTES);
    while let Ok(Some(frame)) = lines.next().await {
        let mut line = String::from_utf8_lossy(&frame.bytes).into_owned();
        if line.len() > STDERR_LINE_BYTES {
            let mut boundary = STDERR_LINE_BYTES;
            while !line.is_char_boundary(boundary) {
                boundary -= 1;
            }
            line.truncate(boundary);
        }
        let mut buffer = tail.lock();
        if buffer.len() == STDERR_LINES {
            buffer.pop_front();
        }
        buffer.push_back(line);
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;

#[cfg(test)]
mod socket_directory_tests {
    use super::*;

    #[test]
    fn short_runtime_paths_keep_the_configured_private_socket_layout() {
        let root = tempfile::Builder::new()
            .prefix("ossc-")
            .tempdir_in("/tmp")
            .unwrap();
        let runtime = root.path().join("run");
        let directory = private_socket_directory(&runtime).unwrap();
        assert_eq!(directory.path().parent(), Some(runtime.as_path()));
        assert_eq!(
            fs::metadata(directory.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let listener =
            std::os::unix::net::UnixListener::bind(directory.path().join("rpc.sock")).unwrap();
        drop(listener);
    }

    #[test]
    fn oversized_runtime_paths_use_a_short_private_socket_directory() {
        let root = tempfile::Builder::new()
            .prefix("ossc-")
            .tempdir_in("/tmp")
            .unwrap();
        let runtime = root.path().join("long-".repeat(30));
        assert!(UnixSocketAddr::from_pathname(runtime.join("sidecar-000000/rpc.sock")).is_err());
        let directory = private_socket_directory(&runtime).unwrap();
        assert_eq!(directory.path().parent(), Some(Path::new("/tmp")));
        assert_eq!(
            fs::metadata(directory.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let path = directory.path().join("rpc.sock");
        assert!(UnixSocketAddr::from_pathname(&path).is_ok());
        let listener = std::os::unix::net::UnixListener::bind(path).unwrap();
        drop(listener);
    }
}
