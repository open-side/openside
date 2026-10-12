use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

/// Agents whose skill directory openside knows without configuration.
pub const BUILTIN_AGENTS: [&str; 3] = ["claude", "codex", "omp"];

/// Resolve the skills directory of another agent.
///
/// Order: `[skills.agents]` entry from the configuration, then
/// `OPENSIDE_SKILLS_<AGENT>_DIR`, then the agent's own convention (`claude`:
/// `$CLAUDE_CONFIG_DIR/skills` or `$HOME/.claude/skills`; `codex`: `$CODEX_HOME/skills`
/// or `$HOME/.codex/skills`; `omp`: `$OMP_CODING_AGENT_DIR/skills`,
/// `$PI_CODING_AGENT_DIR/skills`, or `skills` below the agent's `agent` directory in a
/// hidden `omp` directory of `$HOME`). Any other agent needs
/// one of the first two.
pub fn agent_skills_dir(
    agent: &str,
    configured: &BTreeMap<String, PathBuf>,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    let valid = !agent.is_empty()
        && agent
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return None;
    }
    if let Some(path) = configured.get(agent) {
        return Some(path.clone());
    }
    let variable = format!(
        "OPENSIDE_SKILLS_{}_DIR",
        agent.to_ascii_uppercase().replace('-', "_")
    );
    if let Some(path) = env(&variable).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let (home_variables, dot_dirs): (&[&str], &[&str]) = match agent {
        "claude" => (&["CLAUDE_CONFIG_DIR"], &[".claude"]),
        "codex" => (&["CODEX_HOME"], &[".codex"]),
        // The omp coding agent's own convention, not ours; the components stay separate so
        // no machine-specific path is spelled out.
        "omp" => (
            &["OMP_CODING_AGENT_DIR", "PI_CODING_AGENT_DIR"],
            &[".omp", "agent"],
        ),
        _ => return None,
    };
    for variable in home_variables {
        if let Some(dir) = env(variable).filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(dir).join("skills"));
        }
    }
    let home = env("HOME")
        .or_else(|| env("USERPROFILE"))
        .filter(|v| !v.is_empty())?;
    let mut path = PathBuf::from(home);
    path.extend(dot_dirs);
    Some(path.join("skills"))
}
