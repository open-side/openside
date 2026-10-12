use crate::reference_markdown;

#[test]
fn committed_cli_reference_matches_the_command_definitions() {
    // Same contract as the OpenAPI snapshot test: drift means the published reference is stale.
    let committed = include_str!("../../../../docs/reference/cli.md");
    assert!(
        committed == reference_markdown(),
        "docs/reference/cli.md is stale: run `tools/gen-docs.sh`"
    );
}

#[test]
fn reference_lists_every_visible_subcommand_once_and_documents_globals_at_the_root() {
    let reference = reference_markdown();
    for command in [
        "`openside session list`",
        "`openside ticket resolve`",
        "`openside token create`",
        "`openside mcp`",
        "`openside doctor`",
    ] {
        let headings = reference
            .lines()
            .filter(|line| line.starts_with('#') && line.trim_start_matches('#').trim() == command)
            .count();
        assert_eq!(headings, 1, "{command} must be documented exactly once");
    }
    assert!(reference.contains("`--token <TOKEN>`"));
    assert!(reference.contains("Environment: `OPENSIDE_TOKEN`."));
    // Global flags are explained once at the root instead of under each subcommand.
    assert_eq!(reference.matches("`--socket <PATH>`").count(), 1);
}
