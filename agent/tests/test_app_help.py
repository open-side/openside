from pathlib import Path

from openside_agent.runtime.subagents import FRONT_APP_HELP, FRONT_INSTRUCTIONS
from openside_agent.runtime.task_policy import SUBAGENT_INSTRUCTIONS


def test_front_help_exactly_matches_checked_ui_feature_map():
    root = Path(__file__).resolve().parents[2]
    help_text = (root / "docs/specs/app-help.md").read_text()
    assert FRONT_APP_HELP == help_text
    assert len(help_text.encode()) <= 1500
    assert FRONT_APP_HELP in FRONT_INSTRUCTIONS
    assert FRONT_APP_HELP not in SUBAGENT_INSTRUCTIONS
    assert "app-help.md" not in (root / "crates/opensided/src/prompt.rs").read_text()


def test_front_describes_broken_output_and_candidate_signins_without_guessing():
    policy = " ".join(FRONT_INSTRUCTIONS.split())
    assert "use the static openside app help" in policy
    assert "Never invent a menu or control" in policy
    assert (
        "the chief's home and teammates' home conversations offer neither "
        "Delete conversation nor Clear" in policy
    )
    assert "Archive is reversible and is not deletion" in policy
    assert "acknowledge that it is broken without speculating" in policy
    assert "Markdown document attachment or a native result card" in policy
    assert "only claim delivery after the tool succeeds" in policy
    assert (
        "discovered sign-in suggested by cookies or history, "
        "not verified authentication" in policy
    )
    assert "Active status and authorization for this agent are separate facts" in policy
    assert "Candidate status alone does not mean failure" in policy
    assert "not a reason to ask the person to reconnect" in policy
