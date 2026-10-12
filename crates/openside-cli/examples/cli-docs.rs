//! Print the CLI reference; `docs/reference/cli.md` is this output.

fn main() {
    print!("{}", openside_cli::reference_markdown());
}
