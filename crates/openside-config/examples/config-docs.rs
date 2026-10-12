//! Emit documentation inputs from the configuration API so examples cannot drift.

use std::io::{self, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = io::stdout().lock();
    writeln!(output, "=== EXAMPLE TOML ===")?;
    write!(output, "{}", openside_config::example_toml())?;
    writeln!(output, "=== JSON SCHEMA ===")?;
    serde_json::to_writer_pretty(&mut output, &openside_config::config_schema())?;
    writeln!(output)?;
    Ok(())
}
