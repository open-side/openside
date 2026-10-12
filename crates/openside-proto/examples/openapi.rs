//! Print the generated OpenAPI document; `docs/specs/openapi.json` is this output.

use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut output, &openside_proto::openapi())?;
    writeln!(output)?;
    Ok(())
}
