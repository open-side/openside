//! `openside service unit|install` (PP-4): run opensided as a per-user service that
//! starts at login and restarts after a crash.
//!
//! Linux gets a systemd user unit, macOS a launchd agent. Both only restart on failure, so
//! `openside` stopping the daemon on purpose stays stopped. Sessions survive the restart
//! because the daemon replays its event log on start. `install` writes the file and
//! prints the command that activates it; it never starts services itself.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};

use crate::output_error;

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Platform {
    Systemd,
    Launchd,
}

impl Platform {
    /// The service manager of this OS; Windows has none that `render` can target.
    fn current() -> Result<Self, String> {
        if cfg!(windows) {
            return Err("openside service has no Windows service definition; the daemon does not run on Windows yet (see docs/PLATFORMS.md). Use --platform to render a systemd or launchd file for another machine.".into());
        }
        Ok(if cfg!(target_os = "macos") {
            Self::Launchd
        } else {
            Self::Systemd
        })
    }
}

#[derive(Args)]
pub(crate) struct UnitArgs {
    /// Service manager; default: launchd on macOS, systemd on other Unix. Windows has no default.
    #[arg(long, value_enum)]
    platform: Option<Platform>,
    /// Absolute path of opensided; default: next to this openside binary, then PATH.
    #[arg(long, value_name = "PATH")]
    daemon: Option<PathBuf>,
}

#[derive(Subcommand)]
pub(crate) enum ServiceCommand {
    /// Print the service definition.
    Unit(UnitArgs),
    /// Write the service definition into the user's service directory.
    Install {
        #[command(flatten)]
        unit: UnitArgs,
        /// Directory to write into instead of the platform's per-user default.
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
}

pub(crate) const LAUNCHD_LABEL: &str = "dev.openside.daemon";

fn daemon_path(explicit: Option<&Path>) -> Result<PathBuf, String> {
    let path = match explicit {
        Some(path) => path.to_owned(),
        None => std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("opensided")))
            .filter(|path| path.is_file())
            .or_else(|| {
                std::env::var_os("PATH").and_then(|paths| {
                    std::env::split_paths(&paths)
                        .map(|dir| dir.join("opensided"))
                        .find(|path| path.is_file())
                })
            })
            .ok_or("opensided not found next to openside or on PATH; pass --daemon")?,
    };
    if !path.is_absolute() {
        return Err("--daemon must be an absolute path".into());
    }
    // A newline would end ExecStart= and start a directive of the attacker's choosing.
    if path.to_string_lossy().contains(char::is_control) {
        return Err("--daemon must not contain control characters".into());
    }
    Ok(path)
}

/// One `ExecStart=` word: quoted when needed, with systemd's `%` and `\` escaped.
fn systemd_word(path: &Path) -> String {
    let text = path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('%', "%%")
        .replace('$', "$$")
        .replace('"', "\\\"");
    if text.contains(char::is_whitespace) {
        format!("\"{text}\"")
    } else {
        text
    }
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn render(platform: Platform, daemon: &Path) -> String {
    match platform {
        Platform::Systemd => format!(
            "[Unit]\n\
             Description=openside daemon\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={}\n\
             # The daemon decides whether Chromium outlives it (pending browser tickets).\n\
             KillMode=process\n\
             Restart=on-failure\n\
             RestartSec=2\n\
             # User units get a minimal PATH; the sidecar needs uv, usually in ~/.local/bin.\n\
             Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin:/bin\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            systemd_word(daemon)
        ),
        Platform::Launchd => format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \t<key>Label</key>\n\
             \t<string>{LAUNCHD_LABEL}</string>\n\
             \t<key>ProgramArguments</key>\n\
             \t<array>\n\
             \t\t<string>{}</string>\n\
             \t</array>\n\
             \t<key>RunAtLoad</key>\n\
             \t<true/>\n\
             \t<key>KeepAlive</key>\n\
             \t<dict>\n\
             \t\t<key>SuccessfulExit</key>\n\
             \t\t<false/>\n\
             \t</dict>\n\
             \t<!-- Stop only the daemon; it decides whether Chromium outlives it (pending browser tickets). -->\n\
             \t<key>AbandonProcessGroup</key>\n\
             \t<true/>\n\
             \t<!-- Agents get a minimal PATH; the sidecar needs uv (Homebrew or ~/.local/bin installs). -->\n\
             \t<key>EnvironmentVariables</key>\n\
             \t<dict>\n\
             \t\t<key>PATH</key>\n\
             \t\t<string>/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin</string>\n\
             \t</dict>\n\
             </dict>\n\
             </plist>\n",
            xml(&daemon.to_string_lossy())
        ),
    }
}

fn resolve_platform(explicit: Option<Platform>) -> Result<Platform, String> {
    match explicit {
        Some(platform) => Ok(platform),
        None => Platform::current(),
    }
}

fn default_dir(platform: Platform) -> Result<PathBuf, String> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    match platform {
        Platform::Systemd => std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|home| home.join(".config")))
            .map(|config| config.join("systemd/user")),
        Platform::Launchd => home().map(|home| home.join("Library/LaunchAgents")),
    }
    .ok_or_else(|| "HOME is not set; pass --dir".to_owned())
}

pub(crate) fn execute<W: Write>(command: &ServiceCommand, stdout: &mut W) -> Result<(), String> {
    match command {
        ServiceCommand::Unit(unit) => {
            let platform = resolve_platform(unit.platform)?;
            let text = render(platform, &daemon_path(unit.daemon.as_deref())?);
            stdout.write_all(text.as_bytes()).map_err(output_error)
        }
        ServiceCommand::Install { unit, dir } => {
            let platform = resolve_platform(unit.platform)?;
            let text = render(platform, &daemon_path(unit.daemon.as_deref())?);
            let dir = match dir {
                Some(dir) => dir.clone(),
                None => default_dir(platform)?,
            };
            let (name, activate) = match platform {
                Platform::Systemd => (
                    "openside.service".to_owned(),
                    "systemctl --user daemon-reload && systemctl --user enable --now openside.service\n\
                     To keep it running while logged out: loginctl enable-linger"
                        .to_owned(),
                ),
                Platform::Launchd => {
                    let name = format!("{LAUNCHD_LABEL}.plist");
                    let plist = dir.join(&name).to_string_lossy().replace('\'', r"'\''");
                    let activate = format!("launchctl bootstrap gui/$(id -u) '{plist}'");
                    (name, activate)
                }
            };
            std::fs::create_dir_all(&dir)
                .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
            let path = dir.join(name);
            std::fs::write(&path, text)
                .map_err(|error| format!("could not write {}: {error}", path.display()))?;
            writeln!(
                stdout,
                "wrote {}\nActivate with: {activate}",
                path.display()
            )
            .map_err(output_error)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[cfg(unix)]
    /// The unit is accepted by systemd's own verifier when it is available.
    #[test]
    fn systemd_unit_restarts_on_failure_and_passes_systemd_analyze() {
        let root = tempfile::tempdir().unwrap();
        let daemon = root.path().join("bin dir/opensided");
        std::fs::create_dir_all(daemon.parent().unwrap()).unwrap();
        std::fs::write(&daemon, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&daemon, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let unit = render(Platform::Systemd, &daemon);
        assert!(unit.contains(&format!("ExecStart=\"{}\"\n", daemon.display())));
        assert!(unit.contains("Restart=on-failure\n"));
        // A user manager's default PATH has no ~/.local/bin, where uv usually lives; without it
        // the daemon cannot start the sidecar (observed in a real VM boot, PP-4).
        assert!(unit.contains("Environment=PATH=%h/.local/bin:"));
        assert!(unit.contains("KillMode=process\n"));
        assert!(unit.contains("WantedBy=default.target\n"));
        let path = root.path().join("openside.service");
        std::fs::write(&path, &unit).unwrap();
        let verifier = ["/usr/bin/systemd-analyze", "/bin/systemd-analyze"]
            .into_iter()
            .find(|path| Path::new(path).is_file());
        let Some(verifier) = verifier else {
            eprintln!("systemd-analyze not installed; static checks only");
            return;
        };
        let output = std::process::Command::new(verifier)
            // `--user` needs a running user manager; system mode checks the same syntax and
            // that ExecStart resolves, which is what can go wrong in a generated unit.
            .args(["verify", "--man=no"])
            .arg(&path)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && !stderr.contains("openside.service:"),
            "systemd-analyze verify: {stderr}"
        );
    }

    #[test]
    fn launchd_agent_runs_at_load_and_restarts_only_after_a_failure() {
        let plist = render(Platform::Launchd, Path::new("/opt/a&b/opensided"));
        for expected in [
            "<string>dev.openside.daemon</string>",
            "<string>/opt/a&amp;b/opensided</string>",
            "<key>RunAtLoad</key>\n\t<true/>",
            "<key>SuccessfulExit</key>\n\t\t<false/>",
            "<key>AbandonProcessGroup</key>\n\t<true/>",
            "<string>/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin</string>",
        ] {
            assert!(plist.contains(expected), "{expected} missing:\n{plist}");
        }
        if Path::new("/usr/bin/plutil").is_file() {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("agent.plist");
            std::fs::write(&path, &plist).unwrap();
            let status = std::process::Command::new("/usr/bin/plutil")
                .arg("-lint")
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_has_no_default_service_manager() {
        let error = Platform::current().err().unwrap();
        assert!(error.contains("docs/PLATFORMS.md"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn install_writes_the_unit_into_the_given_directory() {
        let root = tempfile::tempdir().unwrap();
        let command = ServiceCommand::Install {
            unit: UnitArgs {
                platform: Some(Platform::Systemd),
                daemon: Some(PathBuf::from("/opt/openside/opensided")),
            },
            dir: Some(root.path().join("user")),
        };
        let mut out = Vec::new();
        execute(&command, &mut out).unwrap();
        let written = std::fs::read_to_string(root.path().join("user/openside.service")).unwrap();
        assert!(written.contains("ExecStart=/opt/openside/opensided\n"));
        assert!(String::from_utf8(out)
            .unwrap()
            .contains("systemctl --user enable --now openside.service"));
        let relative = ServiceCommand::Unit(UnitArgs {
            platform: Some(Platform::Systemd),
            daemon: Some(PathBuf::from("opensided")),
        });
        assert!(execute(&relative, &mut Vec::new()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unit_paths_cannot_inject_directives_or_shell() {
        let unit = render(Platform::Systemd, Path::new("/opt/$HOME %h/opensided"));
        assert!(
            unit.contains(r#"ExecStart="/opt/$$HOME %%h/opensided""#),
            "{unit}"
        );
        let injected = ServiceCommand::Unit(UnitArgs {
            platform: Some(Platform::Systemd),
            daemon: Some(PathBuf::from("/opt/x\nExecStartPre=/bin/sh")),
        });
        assert!(execute(&injected, &mut Vec::new()).is_err());
        let root = tempfile::tempdir().unwrap();
        let command = ServiceCommand::Install {
            unit: UnitArgs {
                platform: Some(Platform::Launchd),
                daemon: Some(PathBuf::from("/opt/openside/opensided")),
            },
            dir: Some(root.path().join("it's $(here)")),
        };
        let mut out = Vec::new();
        execute(&command, &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains(r"it'\''s $(here)/dev.openside.daemon.plist'"),
            "{out}"
        );
    }
}
