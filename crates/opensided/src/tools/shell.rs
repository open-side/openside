use std::{path::Path, process::Stdio, time::Duration};

use futures_util::future::BoxFuture;
use openside_core::ApprovalTier;
use openside_policy::{Action, Capability};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};

use super::{files::canonical, parse, spec, DaemonTool, ToolEnv, ToolError, ToolExec};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BashArgs {
    command: String,
    cwd: Option<String>,
    timeout_ms: Option<u64>,
}

pub(crate) struct Bash;
impl DaemonTool for Bash {
    fn name(&self) -> &'static str {
        "bash"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<BashArgs>(self.name(), "Run a shell command in a canonical working directory. Bubblewrap isolates the workspace and disables network when available. timeout_ms defaults to 30000 and is at most 120000.", ApprovalTier::Exec, false)
    }
    fn visible(&self, ctx: &ToolEnv) -> bool {
        cfg!(unix)
            && (!ctx.shared.config.loaded.config.sandbox.required
                || ctx.shared.config.bwrap.is_some())
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            let args: BashArgs = parse(args)?;
            if args.command.trim().is_empty() {
                return Err(ToolError("command must not be empty".into()));
            }
            if args.timeout_ms.is_some_and(|t| t == 0 || t > 120_000) {
                return Err(ToolError("timeout_ms must be between 1 and 120000".into()));
            }
            let cwd = match args.cwd {
                Some(cwd) => canonical(&cwd, ctx, false)?,
                None => ctx.workspace.clone().ok_or_else(|| {
                    ToolError("bash requires cwd or a configured workspace".into())
                })?,
            };
            if !cwd.is_dir() {
                return Err(ToolError("cwd must be a directory".into()));
            }
            if ctx.shared.config.bwrap.is_some() && ctx.workspace.is_none() {
                return Err(ToolError(
                    "Sandboxed bash requires a configured workspace".into(),
                ));
            }
            Ok(vec![Action::new(
                Capability::Exec,
                cwd.to_string_lossy().into_owned(),
            )])
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        json!({"command":"[REDACTED]","cwd":args["cwd"],"timeout_ms":args["timeout_ms"]})
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move { execute(args, ctx).await })
    }
}

#[cfg(unix)]
struct ShellChild {
    child: tokio::process::Child,
    group: Option<nix::unistd::Pid>,
}
#[cfg(unix)]
impl Drop for ShellChild {
    fn drop(&mut self) {
        if let Some(group) = self.group {
            let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
        }
        let _ = self.child.start_kill();
    }
}

#[cfg(unix)]
async fn execute(args: Value, ctx: &ToolEnv) -> ToolExec {
    let Ok(args) = parse::<BashArgs>(&args) else {
        return failed();
    };
    let Ok(cwd) = ctx.target(Capability::Exec) else {
        return failed();
    };
    let Some(shell) =
        openside_config::find_on_path("bash").or_else(|| openside_config::find_on_path("sh"))
    else {
        return ToolExec::Denied {
            reason: "No shell executable is available".into(),
            hint: None,
        };
    };
    let sandboxed = ctx.shared.config.bwrap.is_some();
    let readiness =
        sandboxed.then(|| format!("openside-shell-ready-{}", crate::persistence::random_id()));
    let mut command = if let Some(bwrap) = &ctx.shared.config.bwrap {
        let Some(workspace) = &ctx.workspace else {
            return failed();
        };
        let mut command = tokio::process::Command::new(bwrap);
        command.args([
            "--die-with-parent",
            "--new-session",
            "--unshare-all",
            "--clearenv",
        ]);
        for path in ["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc"] {
            if Path::new(path).exists() {
                command.arg("--ro-bind").arg(path).arg(path);
            }
        }
        // network.allow admits destinations for the sidecar and channel/model-fetch HTTP
        // clients only; never translate it into --share-net (which grants every endpoint).
        command
            .args([
                "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--bind",
            ])
            .arg(workspace)
            .arg(workspace);
        // Mask credential source files after exposing the workspace. Symlink aliases
        // reach the masked target, and setup failures never fall back to the host shell.
        let masks = match ctx.shared.credential_paths.sandbox_masks(workspace) {
            Ok(masks) => masks,
            Err(reason) => {
                return ToolExec::Denied {
                    reason: reason.into(),
                    hint: None,
                }
            }
        };
        for path in masks {
            if ctx.shared.credential_paths.sandbox_directory(&path) {
                command.arg("--tmpfs").arg(path);
            } else {
                command.arg("--ro-bind").arg("/dev/null").arg(path);
            }
        }
        command
            .arg("--chdir")
            .arg(cwd)
            .arg("--setenv")
            .arg("PATH")
            .arg("/usr/local/bin:/usr/bin:/bin")
            .arg("--setenv")
            .arg("HOME")
            .arg(workspace)
            .arg("--")
            .arg(&shell);
        command
    } else {
        tokio::process::Command::new(&shell)
    };
    if let Some(marker) = &readiness {
        command
            .arg("-c")
            .arg(format!("printf '%s\\n' '{marker}'; exec \"$@\""))
            .arg("openside-shell")
            .arg(&shell);
    }
    command
        .arg("-c")
        .arg(&args.command)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(_) if sandboxed => return sandbox_denied(),
        Err(_) => return failed(),
    };
    let group = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .map(nix::unistd::Pid::from_raw);
    let mut process = ShellChild { child, group };
    let Some(stdout) = process.child.stdout.take() else {
        return failed();
    };
    let Some(stderr) = process.child.stderr.take() else {
        return failed();
    };
    let operation = async { tokio::join!(process.child.wait(), capture(stdout), capture(stderr)) };
    let result = tokio::select! {
        biased;
        () = ctx.cancellation.cancelled() => None,
        result = tokio::time::timeout(
            Duration::from_millis(args.timeout_ms.unwrap_or(30_000)),
            operation,
        ) => Some(result),
    };
    let Some(result) = result else {
        if let Some(group) = process.group.take() {
            let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
        }
        let _ = process.child.start_kill();
        let _ = process.child.wait().await;
        return ToolExec::Failed("Task tool cancelled".into());
    };
    match result {
        Ok((Ok(status), Ok(stdout), Ok(stderr))) => {
            // The trusted wrapper reports readiness only after sandbox setup and
            // before parsing the requested command. Nonzero command exits remain
            // ordinary tool results; missing readiness never falls back unsandboxed.
            let stdout = if let Some(marker) = &readiness {
                let prefix = format!("{marker}\n");
                let Some(output) = stdout.strip_prefix(&prefix) else {
                    return sandbox_denied();
                };
                output.to_owned()
            } else {
                stdout
            };
            ToolExec::Done {
                for_model: json!({"stdout":stdout,"stderr":stderr,"exit_code":status.code()}),
                for_app: None,
            }
        }
        Err(_) => ToolExec::Failed(
            "Shell command timed out; reduce its scope or increase timeout_ms".into(),
        ),
        _ => failed(),
    }
}

#[cfg(not(unix))]
async fn execute(_: Value, _: &ToolEnv) -> ToolExec {
    ToolExec::Denied {
        reason: "Shell execution is unsupported on this platform".into(),
        hint: None,
    }
}

async fn capture(mut pipe: impl AsyncRead + Unpin) -> std::io::Result<String> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let size = pipe.read(&mut buffer).await?;
        if size == 0 {
            break;
        }
        let keep = size.min((32 * 1024_usize).saturating_sub(output.len()));
        output.extend_from_slice(&buffer[..keep]);
        truncated |= keep < size;
    }
    let mut output = String::from_utf8_lossy(&output).into_owned();
    if truncated {
        output.push_str("\n[Output truncated; narrow the command to retrieve more.]");
    }
    Ok(output)
}
fn failed() -> ToolExec {
    ToolExec::Failed("Shell execution failed".into())
}
fn sandbox_denied() -> ToolExec {
    ToolExec::Denied {
        reason: "Bubblewrap execution failed; unsandboxed fallback is forbidden".into(),
        hint: Some("Check sandbox support with openside doctor".into()),
    }
}
