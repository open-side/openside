use std::io::Write;

use openside_proto::AccountsResponse;

use crate::{client::Client, emit, output_error};

pub(crate) async fn run<W: Write>(
    client: &Client,
    json: bool,
    stdout: &mut W,
) -> Result<(), String> {
    let response: AccountsResponse = client.get("/v1/accounts").await?;
    if json {
        return emit(stdout, &response);
    }
    writeln!(
        stdout,
        "SOURCE\tACCOUNT\tPROVIDER\tMODELS\tUSAGE\tBLOCKED UNTIL (MS)"
    )
    .map_err(output_error)?;
    for source in &response.sources {
        if source.accounts.is_empty() {
            writeln!(stdout, "{}\t{}\t-\t-\t-\t-", source.name, source.reason)
                .map_err(output_error)?;
        }
        for account in &source.accounts {
            let usage = account
                .usage_windows
                .iter()
                .map(|window| match window.used_percent {
                    Some(used) => format!("{}: {used:.1}%", window.name),
                    None => format!("{}: unknown", window.name),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let blocked = account
                .blocked_until_ms
                .map(|value| format!("{value:.0}"))
                .unwrap_or_else(|| "-".into());
            writeln!(
                stdout,
                "{}\t{}\t{}\t{}\t{}\t{}",
                source.name,
                account.label,
                account.provider,
                account.models.join(", "),
                if usage.is_empty() { "-" } else { &usage },
                blocked
            )
            .map_err(output_error)?;
        }
    }
    if response.truncated {
        writeln!(
            stdout,
            "Account metadata truncated at the documented limits"
        )
        .map_err(output_error)?;
    }
    Ok(())
}
