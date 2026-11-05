//! `openside update [-y]` and `openside --update`.
//!
//! openside has no release channel yet, so this prints the installed version and the command
//! that upgrades a source checkout; it never touches the network.

use std::io::Write;

use crate::output_error;

pub(crate) const UPGRADE_COMMAND: &str =
    "git pull && cargo build --release -p opensided -p openside-cli";

pub(crate) fn print<W: Write>(json: bool, stdout: &mut W) -> Result<(), String> {
    let version = env!("CARGO_PKG_VERSION");
    if json {
        return crate::emit(
            stdout,
            &serde_json::json!({"version": version, "release_channel": null, "upgrade_command": UPGRADE_COMMAND}),
        );
    }
    writeln!(
        stdout,
        "openside {version}\nThere is no release channel to update from yet. To upgrade a source checkout, run:\n  {UPGRADE_COMMAND}"
    )
    .map_err(output_error)
}
