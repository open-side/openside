//! Record daemon-path artifact scores without treating local fixtures as public-site evidence.

#[allow(dead_code)]
#[path = "botcheck/harness.rs"]
mod harness;

use std::path::PathBuf;

use harness::{Mode, Result, Run};

#[tokio::main]
async fn main() {
    match run().await {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("botcheck: {error}");
            std::process::exit(2);
        }
    }
}

async fn run() -> Result<i32> {
    let mut output = PathBuf::from("botcheck-results.json");
    let mut mode = String::from("all");
    let mut public_file = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" => output = args.next().ok_or("--output requires a file")?.into(),
            "--mode" => {
                mode = args
                    .next()
                    .ok_or("--mode requires all, stock, patched, or negative")?
            }
            "--public-urls" => {
                public_file = Some(PathBuf::from(
                    args.next().ok_or("--public-urls requires a JSON file")?,
                ))
            }
            "--help" | "-h" => {
                println!("botcheck [--mode all|stock|patched|negative] [--output FILE] [--public-urls FILE]\nDefault: loopback pages only. Public pages require the explicit --public-urls opt-in.");
                return Ok(0);
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    if !["all", "stock", "patched", "negative"].contains(&mode.as_str()) {
        return Err("--mode requires all, stock, patched, or negative".into());
    }
    let public = public_file
        .as_deref()
        .map(harness::read_public_manifest)
        .transpose()?
        .unwrap_or_default();
    if !public.is_empty() {
        eprintln!("PUBLIC OPT-IN: visiting {} user-specified public pages with a fresh temporary profile; site terms remain the user's responsibility", public.len());
    }
    let mut run = Run::new();
    if mode == "all" || mode == "stock" || mode == "negative" {
        if let Some(executable) = harness::stock_browser() {
            if mode == "negative" {
                #[cfg(unix)]
                {
                    let directory = tempfile::tempdir()?;
                    let wrapper = harness::leaky_wrapper(&executable, directory.path())?;
                    run.records.push(
                        harness::record(&wrapper, Mode::NegativeStockAutomation, &public).await?,
                    );
                }
                #[cfg(not(unix))]
                return Err("negative wrapper mode currently requires a Unix shell".into());
            } else {
                run.records
                    .push(harness::record(&executable, Mode::StockCdp, &public).await?);
            }
        } else {
            let reason = "SKIP stock: no chromium, chromium-browser, google-chrome, google-chrome-stable, or chrome binary on PATH";
            eprintln!("{reason}");
            run.skipped.push(reason.into());
        }
    }
    if mode == "all" || mode == "patched" {
        if let Some(executable) = harness::patched_browser() {
            run.records
                .push(harness::record(&executable, Mode::PatchedGateway, &public).await?);
        } else {
            let reason = "SKIP patched: OPENSIDE_PATCHED_BROWSER is unset or its binary is absent; set it to an existing built browser/ overlay binary";
            eprintln!("{reason}");
            run.skipped.push(reason.into());
        }
    }
    let serialized = serde_json::to_string_pretty(&run)?;
    std::fs::write(&output, format!("{serialized}\n"))?;
    println!("{serialized}");
    for record in &run.records {
        eprintln!(
            "{:?}: {} — {}/{} ({:.1}%), P7 {}{}",
            record.mode,
            record.browser_version,
            record.score.passed,
            record.score.total,
            record.score.ratio * 100.0,
            if record.threshold.passed {
                "pass"
            } else {
                "not passed"
            },
            if record.threshold.gated {
                " (gated)"
            } else {
                " (record only)"
            }
        );
    }
    eprintln!("Score record: {}", output.display());
    if run.records.is_empty() {
        return Ok(2);
    }
    Ok(i32::from(run.records.iter().any(|record| {
        (record.threshold.gated && !record.threshold.passed)
            || record
                .public_pages
                .iter()
                .any(|page| page.status != "scored")
    })))
}
