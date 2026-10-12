//! Sidecar compatibility pin checked on every `hello` (`docs/specs/sidecar-protocol.md` §2).
//!
//! The wire protocol number guards message shapes; the `openside_agent` package range guards
//! behavior the daemon relies on beyond shapes (tool gate, snapshot format, deferred resume).

use openside_core::RuntimeError;

/// The only `hello.protocol` value this daemon speaks.
pub const PROTOCOL_VERSION: u8 = 1;
/// Lowest supported `hello.versions.openside_agent`, inclusive.
pub const MIN_AGENT_VERSION: (u64, u64, u64) = (0, 1, 0);
/// First unsupported `hello.versions.openside_agent`; 0.x minors may break behavior.
pub const MAX_AGENT_VERSION_EXCLUSIVE: (u64, u64, u64) = (0, 2, 0);

/// Supported package range in PEP 440 / Cargo comparator syntax, as published in
/// `docs/specs/sidecar-wire.schema.json` (`x-openside-agent-versions`).
pub fn supported_agent_versions() -> String {
    let (a, b, c) = MIN_AGENT_VERSION;
    let (x, y, z) = MAX_AGENT_VERSION_EXCLUSIVE;
    format!(">={a}.{b}.{c},<{x}.{y}.{z}")
}

/// `major.minor.patch` of a version such as `0.1.0`, `0.1.0.dev1` or `0.1.0+local`: the
/// part before any `+`/`-` suffix must be three numbers, optionally followed by PEP 440
/// segments that start with a letter (`dev1`, `post2`).
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let base = version.split(['+', '-']).next()?;
    let mut parts = base.split('.');
    let mut number = || parts.next()?.parse::<u64>().ok();
    let parsed = (number()?, number()?, number()?);
    parts
        .all(|part| part.starts_with(|c: char| c.is_ascii_alphabetic()))
        .then_some(parsed)
}

pub(crate) fn check_protocol(protocol: u8) -> Result<(), RuntimeError> {
    if protocol == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(RuntimeError::Unavailable(format!(
            "sidecar protocol {protocol} is unsupported; this daemon speaks protocol {PROTOCOL_VERSION}"
        )))
    }
}

pub(crate) fn check_agent_version(version: &str) -> Result<(), RuntimeError> {
    if parse(version)
        .is_some_and(|parsed| MIN_AGENT_VERSION <= parsed && parsed < MAX_AGENT_VERSION_EXCLUSIVE)
    {
        return Ok(());
    }
    // The value comes from the peer: echo it only when it is short and version-shaped.
    let shown = if version.len() <= 32
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
    {
        version
    } else {
        "<invalid>"
    };
    Err(RuntimeError::Unavailable(format!(
        "sidecar openside_agent {shown} is outside the supported range {}; reinstall the sidecar from this release",
        supported_agent_versions()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_version_range_is_half_open_and_tolerates_local_suffixes() {
        for ok in ["0.1.0", "0.1.9", "0.1.0.dev1", "0.1.3+local"] {
            assert!(check_agent_version(ok).is_ok(), "{ok}");
        }
        for bad in [
            "0.0.9", "0.2.0", "1.0.0", "test", "", "0.1", "0+1-0", "0.1+0.0", "0.1.0.5",
        ] {
            assert!(
                matches!(check_agent_version(bad), Err(RuntimeError::Unavailable(_))),
                "{bad}"
            );
        }
        assert_eq!(supported_agent_versions(), ">=0.1.0,<0.2.0");
    }

    #[test]
    fn rejection_never_echoes_unbounded_peer_text() {
        let Err(RuntimeError::Unavailable(message)) = check_agent_version(&"x".repeat(100)) else {
            panic!("long version must be rejected");
        };
        assert!(
            message.contains("<invalid>") && !message.contains("xxxx"),
            "{message}"
        );
    }
}
