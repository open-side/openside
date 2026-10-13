//! Hermetic real-process acceptance harness for the Python sidecar.
//!
//! Each daemon and CLI receives private HOME/XDG directories and an empty
//! environment except PATH and explicit test settings. The private `bin/`
//! directory leads PATH so tests can shadow host tools (a fake `tailscale`)
//! instead of depending on what the runner happens to have installed.
//! Scripted FunctionModel responses require no credentials. Missing repository
//! Python environments skip with a diagnostic; an installed but broken sidecar
//! fails normally.
//! Cleanup tracks descendants across separate daemon, sidecar and browser process
//! groups, reaps them, and checks that no owned child remains. On Linux the test process is
//! a child subreaper, so it also reaps the orphans it adopts while the test runs.

#![cfg(unix)]

pub mod login_fixture;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::{Output, Stdio},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use futures_util::StreamExt;
use nix::{
    sys::{
        signal::{kill, killpg, Signal},
        wait::{waitpid, WaitPidFlag},
    },
    unistd::Pid,
};
use openside_proto::{Event, EventKind, SessionId};
use serde_json::Value;
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
};

/// HTTP status and parsed response body.
pub struct Response {
    /// Numeric HTTP status.
    pub status: u16,
    /// JSON response, or null for an empty body.
    pub body: Value,
}

/// Private daemon, durable storage and scripted real Python runtime.
pub struct Harness {
    /// Private Unix HTTP endpoint.
    pub socket: PathBuf,
    /// Durable daemon storage retained across restart.
    pub data_dir: PathBuf,
    directory: tempfile::TempDir,
    daemon_bin: String,
    child: Option<Child>,
    client: reqwest::Client,
    human_token: tokio::sync::OnceCell<String>,
    owned: Arc<Mutex<Owned>>,
    reaper: Option<tokio::task::JoinHandle<()>>,
}

/// Processes this harness answers for. `pids` roots descendant tracking and cleanup;
/// `direct` are the daemons it spawned itself, which only their tokio `Child` may reap.
#[derive(Default)]
struct Owned {
    pids: HashSet<i32>,
    direct: HashSet<i32>,
}

fn lock(owned: &Mutex<Owned>) -> MutexGuard<'_, Owned> {
    owned.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Harness {
    /// Prepare isolated configuration, or explain why the Python environment is missing.
    pub fn prepare(daemon_bin: &str, script: &Value) -> Option<Self> {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let python = repo.join("agent/.venv/bin/python");
        if !python.is_file() {
            eprintln!("SKIP: real sidecar Python is missing; run uv sync --project agent");
            return None;
        }
        #[cfg(target_os = "linux")]
        nix::sys::prctl::set_child_subreaper(true)
            .unwrap_or_else(|e| panic!("child subreaper: {e}"));
        let directory = tempfile::tempdir().unwrap_or_else(|e| panic!("test directory: {e}"));
        for name in [
            "home",
            "config/openside",
            "data",
            "state",
            "cache",
            "run",
            "tmp",
            "workspace",
            "stderr",
            "python-hooks",
            "bin",
        ] {
            std::fs::create_dir_all(directory.path().join(name))
                .unwrap_or_else(|e| panic!("private directory: {e}"));
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            directory.path().join("run"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap_or_else(|e| panic!("runtime permissions: {e}"));
        let socket = directory.path().join("run/opensided.sock");
        let data_dir = directory.path().join("data");
        let argv = serde_json::json!([python, "-m", "openside_agent"]);
        // Scripts drive the session agent directly; front-agent tests opt in with
        // `orchestration = "front"`.
        let config = format!(
            "[agent]\ncommand = {argv}\norchestration = \"single\"\n[models]\ndefault = \"script\"\n[policy]\nmode = \"full\"\n"
        );
        std::fs::write(directory.path().join("config/openside/config.toml"), config)
            .unwrap_or_else(|e| panic!("test config: {e}"));
        // Python startup instrumentation captures raw stderr without changing the
        // configured `python -m openside_agent` entrypoint or the RPC transport.
        std::fs::write(directory.path().join("python-hooks/sitecustomize.py"),
            "import os\nfrom pathlib import Path\nif os.environ.get('OPENSIDE_SIDECAR_NONCE'):\n    root = Path(os.environ['OPENSIDE_TEST_STDERR'])\n    fd = os.open(root / f'sidecar-{os.getpid()}.stderr', os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)\n    os.dup2(fd, 2)\n    os.close(fd)\n")
            .unwrap_or_else(|e| panic!("stderr capture: {e}"));
        let client = reqwest::Client::builder()
            .unix_socket(socket.clone())
            .build()
            .unwrap_or_else(|e| panic!("Unix client: {e}"));
        let harness = Self {
            socket,
            data_dir,
            directory,
            daemon_bin: daemon_bin.into(),
            child: None,
            client,
            human_token: tokio::sync::OnceCell::new(),
            owned: Arc::default(),
            reaper: None,
        };
        harness.replace_script(script);
        Some(harness)
    }

    /// Prepare and start a credential-free real Python sidecar daemon.
    pub async fn start(daemon_bin: &str, script: &Value) -> Option<Self> {
        let mut harness = Self::prepare(daemon_bin, script)?;
        harness.launch().await;
        Some(harness)
    }

    /// Root containing only disposable test artifacts.
    pub fn root(&self) -> &Path {
        self.directory.path()
    }

    /// PID of the supervised daemon, for resource sampling during the soak.
    pub fn daemon_pid(&self) -> u32 {
        self.child
            .as_ref()
            .and_then(Child::id)
            .unwrap_or_else(|| panic!("daemon is not running"))
    }

    /// Session workspace containing only test fixtures.
    pub fn workspace(&self) -> PathBuf {
        self.root().join("workspace")
    }

    /// Append additional TOML sections before launching the daemon.
    pub fn append_config(&self, text: &str) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(self.root().join("config/openside/config.toml"))
            .unwrap_or_else(|e| panic!("config append: {e}"));
        file.write_all(text.as_bytes())
            .unwrap_or_else(|e| panic!("config append: {e}"));
    }

    /// Replace the on-disk script; a newly spawned sidecar resumes at its saved cursor.
    pub fn replace_script(&self, script: &Value) {
        std::fs::write(self.root().join("script.json"), script.to_string())
            .unwrap_or_else(|e| panic!("script: {e}"));
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        // A harness child must die with the test process: a harness killed mid-test runs no
        // Drop, and an orphaned daemon leaks its sidecar and browser tree. The signal binds
        // to the spawning thread; the harness launches from its test runtime's threads, which
        // live exactly as long as the test.
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: the hook runs only async-signal-safe work between fork and exec: one
            // prctl(2) syscall through nix.
            #[allow(unsafe_code)]
            unsafe {
                command.as_std_mut().pre_exec(|| {
                    nix::sys::prctl::set_pdeathsig(nix::sys::signal::Signal::SIGKILL)
                        .map_err(std::io::Error::from)
                });
            }
        }
        command
            .env_clear()
            .current_dir(self.workspace())
            .env("HOME", self.root().join("home"))
            .env("XDG_CONFIG_HOME", self.root().join("config"))
            .env("XDG_DATA_HOME", &self.data_dir)
            .env("XDG_STATE_HOME", self.root().join("state"))
            .env("XDG_CACHE_HOME", self.root().join("cache"))
            .env("XDG_RUNTIME_DIR", self.root().join("run"))
            .env("TMPDIR", self.root().join("tmp"))
            .env("OPENSIDE_AGENT_SCRIPT", self.root().join("script.json"))
            .env("OPENSIDE_TEST_STDERR", self.root().join("stderr"))
            .env("PYTHONPATH", self.root().join("python-hooks"))
            .kill_on_drop(true);
        // The private bin directory first: a fixture executable there (e.g. a fake
        // `tailscale`) decides what the daemon can find, not the runner's host.
        let mut path = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .collect::<Vec<_>>();
        path.insert(0, self.root().join("bin"));
        command.env(
            "PATH",
            std::env::join_paths(path)
                .unwrap_or_else(|e| panic!("test PATH entries are joinable: {e}")),
        );
        if std::env::var_os("OPENSIDE_TEST_DISABLE_PREWARM").is_some() {
            command.env("OPENSIDE_TEST_DISABLE_PREWARM", "1");
        }
        command
    }

    /// Launch the real daemon binary against durable private configuration.
    pub async fn launch(&mut self) {
        let args = vec![
            "--data-dir".into(),
            self.data_dir.clone().into_os_string(),
            "--socket".into(),
            self.socket.clone().into_os_string(),
            "--listen".into(),
            "127.0.0.1:0".into(),
        ];
        self.launch_with(args).await;
    }

    /// Launch the daemon with exactly `args` (none: as a service manager would, from the
    /// configuration file alone), in this harness's private environment.
    pub async fn launch_with(&mut self, args: Vec<std::ffi::OsString>) {
        let stderr = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root().join("daemon.stderr"))
            .unwrap_or_else(|e| panic!("daemon capture: {e}"));
        let child = self
            .command(&self.daemon_bin)
            .args(args)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .unwrap_or_else(|e| panic!("daemon spawn: {e}"));
        if let Some(pid) = child.id() {
            let mut owned = lock(&self.owned);
            owned.pids.insert(pid as i32);
            owned.direct.insert(pid as i32);
        }
        self.child = Some(child);
        #[cfg(target_os = "linux")]
        if self.reaper.is_none() {
            self.reaper = Some(spawn_reaper(Arc::clone(&self.owned)));
        }
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if self
                    .client
                    .get("http://localhost/v1/health")
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success())
                {
                    break;
                }
                if let Some(status) = self
                    .child
                    .as_mut()
                    .and_then(|c| c.try_wait().ok())
                    .flatten()
                {
                    let diagnostics = std::fs::read_to_string(self.root().join("daemon.stderr"))
                        .unwrap_or_default();
                    panic!("daemon exited {status}: {diagnostics}");
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("daemon startup deadline"));
    }

    /// Send JSON HTTP over the administrator-only Unix endpoint.
    pub async fn request(&self, method: &str, path: &str, body: Option<Value>) -> Response {
        self.request_as(method, path, body, None).await
    }

    /// Mark Unix requests with reduced client authority to exercise non-owner queue lanes.
    pub async fn request_as(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        client_kind: Option<&str>,
    ) -> Response {
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .unwrap_or_else(|e| panic!("method: {e}"));
        let mut request = self
            .client
            .request(method, format!("http://localhost{path}"));
        if let Some(client_kind) = client_kind {
            request = request.header("x-openside-client", client_kind);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .unwrap_or_else(|e| panic!("Unix request: {e}"));
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .unwrap_or_else(|e| panic!("response: {e}"));
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("JSON response {status}: {e}"))
        };
        Response { status, body }
    }

    /// GET over the Unix endpoint, returning the status and the raw body (non-JSON downloads).
    pub async fn get_bytes(&self, path: &str) -> (u16, Vec<u8>) {
        let response = self
            .client
            .get(format!("http://localhost{path}"))
            .send()
            .await
            .unwrap_or_else(|e| panic!("Unix request: {e}"));
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .unwrap_or_else(|e| panic!("response: {e}"));
        (status, bytes.to_vec())
    }

    /// Open SSE after an exclusive durable cursor.
    pub async fn events(&self, session: &str, last_seq: u64) -> SseReader {
        self.events_query(session, last_seq, "").await
    }

    /// Open SSE with a raw query string such as `subagents=false`.
    pub async fn events_query(&self, session: &str, last_seq: u64, query: &str) -> SseReader {
        let response = self
            .client
            .get(format!(
                "http://localhost/v1/sessions/{session}/events?{query}"
            ))
            .header("Last-Event-ID", last_seq.to_string())
            .send()
            .await
            .unwrap_or_else(|e| panic!("SSE request: {e}"));
        assert!(response.status().is_success());
        SseReader {
            stream: Box::pin(response.bytes_stream().map(|r| r.map(|b| b.to_vec()))),
            buffer: Vec::new(),
        }
    }

    /// Execute the real CLI with hermetic settings and optional piped stdin.
    pub async fn cli(&self, binary: &str, args: &[&str], stdin: Option<&[u8]>) -> Output {
        let mut command = self.command(binary);
        command.arg("--socket").arg(&self.socket).args(args);
        Self::piped_cli(command, stdin).await
    }

    /// Exercise the CLI as a paired person, including hidden stdin for login or enrollment.
    pub async fn human_cli(&self, binary: &str, args: &[&str], stdin: Option<&[u8]>) -> Output {
        let port = std::fs::read_to_string(self.data_dir.join("opensided.port"))
            .unwrap_or_else(|error| panic!("daemon port file: {error}"));
        let mut command = self.command(binary);
        command
            .arg("--url")
            .arg(format!("http://127.0.0.1:{}", port.trim()))
            .env("OPENSIDE_TOKEN", self.human_token().await)
            .args(args);
        Self::piped_cli(command, stdin).await
    }

    async fn piped_cli(mut command: Command, stdin: Option<&[u8]>) -> Output {
        let mut child = command
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("CLI spawn: {e}"));
        if let Some(input) = stdin {
            let mut pipe = child.stdin.take().unwrap_or_else(|| panic!("CLI stdin"));
            pipe.write_all(input)
                .await
                .unwrap_or_else(|e| panic!("CLI input: {e}"));
            drop(pipe);
        }
        tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
            .await
            .unwrap_or_else(|_| panic!("CLI deadline"))
            .unwrap_or_else(|e| panic!("CLI output: {e}"))
    }

    /// Execute the real CLI without `--socket`, with extra environment such as
    /// `OPENSIDE_REMOTE`: how a remote computer reaches this daemon through a saved profile.
    pub async fn cli_remote(&self, binary: &str, args: &[&str], env: &[(&str, &str)]) -> Output {
        let child = self
            .command(binary)
            .envs(env.iter().copied())
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("CLI spawn: {e}"));
        tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
            .await
            .unwrap_or_else(|_| panic!("CLI deadline"))
            .unwrap_or_else(|e| panic!("CLI output: {e}"))
    }

    /// Execute the real CLI with stdout attached to a pseudo-terminal, returning its exit
    /// status and everything it printed there.
    pub async fn cli_on_tty(
        &self,
        binary: &str,
        args: &[&str],
    ) -> (std::process::ExitStatus, String) {
        use std::io::Read;
        let pty = nix::pty::openpty(None, None).unwrap_or_else(|e| panic!("openpty: {e}"));
        // The temporary `Command` owns the slave end and is dropped right after spawn, so the
        // master reads to EOF once the child exits.
        let mut child = self
            .command(binary)
            .arg("--socket")
            .arg(&self.socket)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(std::fs::File::from(pty.slave)))
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("CLI spawn: {e}"));
        let mut master = std::fs::File::from(pty.master);
        let reader = tokio::task::spawn_blocking(move || {
            let mut text = Vec::new();
            let mut chunk = [0u8; 4096];
            // Linux reports EIO instead of EOF when the last slave descriptor closes.
            while let Ok(n) = master.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                text.extend_from_slice(&chunk[..n]);
            }
            String::from_utf8_lossy(&text).into_owned()
        });
        let status = tokio::time::timeout(Duration::from_secs(30), child.wait())
            .await
            .unwrap_or_else(|_| panic!("CLI deadline"))
            .unwrap_or_else(|e| panic!("CLI wait: {e}"));
        let text = reader.await.unwrap_or_else(|e| panic!("pty reader: {e}"));
        (status, text)
    }

    /// Execute the real CLI with stdout and stderr on one pseudo-terminal of `columns` x `rows`,
    /// as a person's terminal window would show it. `env` is added to the hermetic environment
    /// (for example `TERM`, `LANG`, `NO_COLOR`); raw bytes are returned, escape codes included.
    pub async fn cli_on_pty(
        &self,
        binary: &str,
        args: &[&str],
        env: &[(&str, &str)],
        columns: u16,
        rows: u16,
    ) -> (std::process::ExitStatus, Vec<u8>) {
        use std::io::Read;
        let size = nix::pty::Winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = nix::pty::openpty(Some(&size), None).unwrap_or_else(|e| panic!("openpty: {e}"));
        let slave = std::fs::File::from(pty.slave);
        let stderr = slave
            .try_clone()
            .unwrap_or_else(|e| panic!("pty clone: {e}"));
        let mut child = self
            .command(binary)
            .envs(env.iter().copied())
            .arg("--socket")
            .arg(&self.socket)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(slave))
            .stderr(Stdio::from(stderr))
            .spawn()
            .unwrap_or_else(|e| panic!("CLI spawn: {e}"));
        let mut master = std::fs::File::from(pty.master);
        let reader = tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 4096];
            // Linux reports EIO instead of EOF when the last slave descriptor closes.
            while let Ok(n) = master.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..n]);
            }
            bytes
        });
        let status = tokio::time::timeout(Duration::from_secs(60), child.wait())
            .await
            .unwrap_or_else(|_| panic!("CLI deadline"))
            .unwrap_or_else(|e| panic!("CLI wait: {e}"));
        let bytes = reader.await.unwrap_or_else(|e| panic!("pty reader: {e}"));
        (status, bytes)
    }

    /// A distinct paired person identity; Unix-socket clients are agent identities.
    pub async fn human_token(&self) -> &str {
        self.human_token
            .get_or_init(|| async {
                let paired = self
                    .request(
                        "POST",
                        "/v1/devices/pair",
                        Some(serde_json::json!({
                            "name":"Fixture person","kind":"person_device",
                            "scopes":["read","approve","human_step"]
                        })),
                    )
                    .await;
                assert_eq!(paired.status, 201, "{}", paired.body);
                paired.body["token"]
                    .as_str()
                    .unwrap_or_else(|| panic!("person token missing"))
                    .to_owned()
            })
            .await
            .as_str()
    }

    /// Simulate a person's response without granting approval to the initiating local agent.
    pub async fn human_request(&self, method: &str, path: &str, body: Option<Value>) -> Response {
        self.tcp_request(method, path, Some(self.human_token().await), body)
            .await
    }

    /// Send JSON HTTP over the loopback TCP listener as a remote client holding `token`.
    pub async fn tcp_request(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Response {
        let port = std::fs::read_to_string(self.data_dir.join("opensided.port"))
            .unwrap_or_else(|e| panic!("daemon port file: {e}"));
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .unwrap_or_else(|e| panic!("method: {e}"));
        let mut request = reqwest::Client::new()
            .request(method, format!("http://127.0.0.1:{}{path}", port.trim()));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .unwrap_or_else(|e| panic!("TCP request: {e}"));
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .unwrap_or_else(|e| panic!("response: {e}"));
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Response { status, body }
    }

    /// Read a committed Python state snapshot, including conversation and script cursor.
    pub fn snapshot(&self, session: &str) -> Value {
        let store = openside_sessions::Store::open(self.data_dir.join("events.db"))
            .unwrap_or_else(|e| panic!("snapshot store: {e}"));
        let state = store
            .agent_state(&SessionId::new(session))
            .unwrap_or_else(|e| panic!("snapshot read: {e}"))
            .unwrap_or_else(|| panic!("missing committed snapshot"));
        assert_eq!(state.format, "openside-agent-state/1");
        serde_json::from_str(&state.data).unwrap_or_else(|e| panic!("Python snapshot JSON: {e}"))
    }

    fn sidecar_pids(&self) -> Vec<i32> {
        std::fs::read_dir(self.root().join("stderr"))
            .unwrap_or_else(|e| panic!("stderr directory: {e}"))
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_prefix("sidecar-"))
                    .and_then(|name| name.strip_suffix(".stderr"))
                    .and_then(|pid| pid.parse().ok())
            })
            .collect()
    }

    /// PID of the only currently live scripted Python process in this harness.
    pub fn sidecar_pid(&mut self) -> i32 {
        self.track_descendants();
        let live: Vec<_> = self
            .sidecar_pids()
            .into_iter()
            .filter(|pid| alive(*pid))
            .collect();
        assert_eq!(live.len(), 1, "expected one active sidecar");
        live[0]
    }

    /// SIGKILL only the sidecar, preserving its daemon, browser and durable queues.
    pub fn kill_sidecar(&mut self) -> i32 {
        let pid = self.sidecar_pid();
        kill(Pid::from_raw(pid), Signal::SIGKILL)
            .unwrap_or_else(|e| panic!("sidecar SIGKILL: {e}"));
        pid
    }

    fn track_descendants(&mut self) {
        let sidecars = self.sidecar_pids();
        let mut owned = lock(&self.owned);
        owned.pids.extend(sidecars);
        let table = process_table();
        loop {
            let mut changed = false;
            for &(pid, parent) in &table {
                if owned.pids.contains(&parent) {
                    changed |= owned.pids.insert(pid);
                }
            }
            if !changed {
                break;
            }
        }
        // Crashpad handlers double-fork out of the browser's tree straight to this subreaper;
        // only their command line (the profile's crash database) ties them to this harness.
        #[cfg(target_os = "linux")]
        {
            let root = self.directory.path().as_os_str().as_encoded_bytes();
            for process in processes() {
                if process.adopted() && !process.zombie {
                    let mentions_root = std::fs::read(format!("/proc/{}/cmdline", process.pid))
                        .is_ok_and(|cmdline| cmdline.windows(root.len()).any(|part| part == root));
                    if mentions_root {
                        owned.pids.insert(process.pid);
                    }
                }
            }
        }
    }

    /// Kill and reap only the daemon and sidecar; browser processes remain for restart recovery.
    pub async fn crash_daemon(&mut self) {
        self.track_descendants();
        for pid in self.sidecar_pids() {
            let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }

    /// Restart the daemon after an abrupt crash, retaining browser targets and storage.
    pub async fn restart(&mut self) {
        self.crash_daemon().await;
        self.launch().await;
    }

    /// Terminate, reap and assert absence of all owned descendants.
    pub async fn kill(&mut self) {
        self.track_descendants();
        if let Some(mut child) = self.child.take() {
            if let Some(pid) = child.id() {
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
            }
            if tokio::time::timeout(Duration::from_secs(8), child.wait())
                .await
                .is_err()
            {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
        self.track_descendants();
        let owned: Vec<i32> = lock(&self.owned).pids.iter().copied().collect();
        for &pid in &owned {
            let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
        }
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                for &pid in &owned {
                    let _ = waitpid(Pid::from_raw(pid), Some(WaitPidFlag::WNOHANG));
                }
                if owned.iter().all(|pid| !alive(*pid)) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "owned child processes survived cleanup: {:?}",
                owned.iter().filter(|pid| alive(**pid)).collect::<Vec<_>>()
            )
        });
        #[cfg(target_os = "linux")]
        reap_orphans(&self.owned);
    }

    /// Assert canaries never appear in daemon stores, logs, snapshots or raw sidecar stderr.
    pub fn assert_no_canaries(&self, canaries: &[&str]) {
        let mut files = vec![
            self.data_dir.clone(),
            self.root().join("stderr"),
            self.root().join("daemon.stderr"),
        ];
        while let Some(path) = files.pop() {
            if path.is_dir() {
                files.extend(
                    std::fs::read_dir(&path)
                        .unwrap_or_else(|e| panic!("canary directory: {e}"))
                        .map(|entry| entry.unwrap_or_else(|e| panic!("canary entry: {e}")).path()),
                );
            } else if path.is_file() {
                let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("canary read: {e}"));
                for canary in canaries {
                    assert!(
                        !bytes
                            .windows(canary.len())
                            .any(|part| part == canary.as_bytes()),
                        "canary leaked into {}",
                        path.display()
                    );
                }
            }
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(reaper) = self.reaper.take() {
            reaper.abort();
        }
        self.track_descendants();
        let owned: Vec<i32> = lock(&self.owned).pids.iter().copied().collect();
        for &pid in &owned {
            let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
        // Reap adopted descendants even during assertion unwinding.
        for &pid in &owned {
            let _ = waitpid(Pid::from_raw(pid), None);
        }
        #[cfg(target_os = "linux")]
        reap_orphans(&self.owned);
    }
}

fn alive(pid: i32) -> bool {
    kill(Pid::from_raw(pid), None).is_ok()
}

#[cfg(target_os = "linux")]
struct Process {
    pid: i32,
    parent: i32,
    group: i32,
    zombie: bool,
}

#[cfg(target_os = "linux")]
impl Process {
    /// A child of this process that cannot be one it spawned: those stay in this process's
    /// group or lead their own (`process_group(0)`, `setsid`), so a member of a foreign group
    /// was forked by a descendant and re-parented here.
    fn adopted(&self) -> bool {
        self.parent == std::process::id() as i32
            && self.group != self.pid
            && self.group != nix::unistd::getpgrp().as_raw()
    }
}

#[cfg(target_os = "linux")]
fn processes() -> Vec<Process> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let pid = entry.ok()?.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            // `comm` may contain spaces and parentheses; the fields follow the last ')'.
            let mut fields = stat.rsplit_once(") ")?.1.split(' ');
            let zombie = fields.next()? == "Z";
            let parent = fields.next()?.parse().ok()?;
            let group = fields.next()?.parse().ok()?;
            Some(Process {
                pid,
                parent,
                group,
                zombie,
            })
        })
        .collect()
}

/// The harness makes this process a child subreaper so no descendant escapes cleanup, which
/// also makes it the parent of every orphan: renderers outliving a SIGKILLed browser, crashpad
/// handlers, a browser adopted across a daemon restart. Nothing else waits for those; left
/// as zombies they accumulate without bound in a soak, and a zombie still answers
/// `kill(pid, 0)`, so a daemon waiting for an adopted browser to exit times out. `waitpid(-1)`
/// would steal the statuses tokio `Child`s of this process (daemons, CLIs, browsers launched
/// by tests) wait for; only zombies that cannot be such a child are reaped: adopted ones, and
/// owned group leaders this harness did not spawn itself (a browser or sidecar whose daemon
/// died first).
#[cfg(target_os = "linux")]
fn reap_orphans(owned: &Mutex<Owned>) {
    use nix::sys::wait::WaitStatus;
    let me = std::process::id() as i32;
    for process in processes() {
        if !process.zombie || process.parent != me {
            continue;
        }
        let mut owned = lock(owned);
        let reapable = process.adopted()
            || (owned.pids.contains(&process.pid) && !owned.direct.contains(&process.pid));
        if reapable
            && matches!(
                waitpid(Pid::from_raw(process.pid), Some(WaitPidFlag::WNOHANG)),
                Ok(WaitStatus::Exited(..) | WaitStatus::Signaled(..))
            )
        {
            // A reaped pid may be reused by an unrelated child; it is no longer ours.
            owned.pids.remove(&process.pid);
        }
    }
}

/// Reap on every SIGCHLD, with a periodic pass because SIGCHLDs coalesce.
#[cfg(target_os = "linux")]
fn spawn_reaper(owned: Arc<Mutex<Owned>>) -> tokio::task::JoinHandle<()> {
    let mut children = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child())
        .unwrap_or_else(|e| panic!("SIGCHLD listener: {e}"));
    tokio::spawn(async move {
        loop {
            reap_orphans(&owned);
            tokio::select! {
                _ = children.recv() => {}
                () = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
    })
}

fn process_table() -> Vec<(i32, i32)> {
    let output = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=", "-o", "ppid="])
        .output()
        .unwrap_or_else(|e| panic!("process inventory: {e}"));
    assert!(output.status.success(), "process inventory failed");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        })
        .collect()
}

type ByteStream =
    std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Vec<u8>, reqwest::Error>> + Send>>;

/// Incremental SSE reader verifying the durable sequence identifier.
pub struct SseReader {
    stream: ByteStream,
    buffer: Vec<u8>,
}

impl SseReader {
    /// Read the next event, failing on stalled or prematurely closed streams.
    pub async fn next(&mut self) -> Event {
        self.next_with_timeout(Duration::from_secs(30)).await
    }

    /// Keep real-model QA bounded without imposing scripted-model latency on providers.
    pub async fn next_with_timeout(&mut self, deadline: Duration) -> Event {
        tokio::time::timeout(deadline, self.read())
            .await
            .unwrap_or_else(|_| panic!("SSE event deadline"))
    }

    /// Read through the next settled boundary.
    pub async fn settled(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            let event = self.next().await;
            let done = matches!(event.kind, EventKind::SessionSettled {});
            events.push(event);
            if done {
                return events;
            }
        }
    }

    /// Whether the server ends the stream within `limit`; frames still in flight are discarded.
    pub async fn ends_within(&mut self, limit: Duration) -> bool {
        tokio::time::timeout(limit, async { while self.stream.next().await.is_some() {} })
            .await
            .is_ok()
    }

    async fn read(&mut self) -> Event {
        loop {
            if let Some(end) = self.buffer.windows(2).position(|window| window == b"\n\n") {
                let frame: Vec<_> = self.buffer.drain(..end + 2).collect();
                let text = String::from_utf8(frame).unwrap_or_else(|e| panic!("SSE encoding: {e}"));
                let id = text
                    .lines()
                    .find_map(|line| line.strip_prefix("id:").map(str::trim));
                let data = text
                    .lines()
                    .find_map(|line| line.strip_prefix("data:").map(str::trim));
                if let Some(data) = data {
                    let event: Event =
                        serde_json::from_str(data).unwrap_or_else(|e| panic!("SSE JSON: {e}"));
                    assert_eq!(id.and_then(|id| id.parse::<u64>().ok()), Some(event.seq));
                    return event;
                }
            } else {
                let chunk = self
                    .stream
                    .next()
                    .await
                    .unwrap_or_else(|| panic!("SSE ended"))
                    .unwrap_or_else(|e| panic!("SSE body: {e}"));
                self.buffer.extend(chunk);
            }
        }
    }
}
