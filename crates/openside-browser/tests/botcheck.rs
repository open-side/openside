//! Loopback fixtures separate artifact regressions from user-run public-page evidence.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
#[path = "../examples/botcheck/harness.rs"]
mod harness;

use harness::{Extractor, Mode, PublicPage};
use serde_json::json;
use tokio::sync::Mutex;

static SERIAL: Mutex<()> = Mutex::const_new(());

fn stock() -> Option<std::path::PathBuf> {
    let found = harness::stock_browser();
    if found.is_none() {
        eprintln!("SKIP botcheck stock: no Chromium-compatible browser on PATH");
    }
    found
}

#[tokio::test]
async fn local_stock_records_real_detection_signals() {
    let _serial = SERIAL.lock().await;
    let Some(executable) = stock() else {
        return;
    };
    let record = harness::record(&executable, Mode::StockCdp, &[])
        .await
        .unwrap();
    println!("STOCK {}", serde_json::to_string(&record).unwrap());
    assert_eq!(record.score.total, 14);
    assert!(!record.threshold.gated);
    assert!(
        record.runtime_enable_sent,
        "stock uses the daemon's CDP fallback path"
    );
    assert_eq!(
        record
            .checks
            .iter()
            .find(|check| check.id == "cdp.runtime_console_getter")
            .unwrap()
            .observed["runtime_enable_sent"],
        true
    );
}

#[tokio::test]
async fn local_patched_passes_every_p7_automation_artifact_check() {
    let _serial = SERIAL.lock().await;
    let Some(executable) = harness::patched_browser() else {
        eprintln!("SKIP botcheck patched: OPENSIDE_PATCHED_BROWSER is unset or its binary is absent; build browser/ and set it to that binary");
        return;
    };
    let record = harness::record(&executable, Mode::PatchedGateway, &[])
        .await
        .unwrap();
    println!("PATCHED {}", serde_json::to_string(&record).unwrap());
    assert_eq!(record.score.total, 15);
    assert!(record.threshold.gated);
    assert!(record.threshold.not_observed.is_empty());
    assert!(
        record.threshold.passed,
        "P7 automation checks must all pass: {:?}",
        record.checks
    );
    assert!(!record.runtime_enable_sent);
    assert_eq!(
        record
            .checks
            .iter()
            .find(|check| check.id == "browser.automation_infobar")
            .unwrap()
            .observed["automation_infobar_count"],
        0
    );
}

#[cfg(unix)]
#[tokio::test]
async fn deliberately_leaky_stock_configuration_is_detected() {
    let _serial = SERIAL.lock().await;
    let Some(executable) = stock() else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let wrapper = harness::leaky_wrapper(&executable, directory.path()).unwrap();
    let record = harness::record(&wrapper, Mode::NegativeStockAutomation, &[])
        .await
        .unwrap();
    println!("NEGATIVE {}", serde_json::to_string(&record).unwrap());
    for id in [
        "navigator.webdriver",
        "user_agent.headless",
        "cdp.runtime_console_getter",
    ] {
        let check = record.checks.iter().find(|check| check.id == id).unwrap();
        assert!(
            !check.passed,
            "deliberately leaky stock launch escaped {id}: {check:?}"
        );
    }
    assert_eq!(
        record
            .checks
            .iter()
            .find(|check| check.id == "navigator.webdriver")
            .unwrap()
            .observed["value"],
        true
    );
    assert!(record.score.passed < record.score.total);
}

#[tokio::test]
async fn public_extractors_read_real_loopback_page_scores() {
    let _serial = SERIAL.lock().await;
    let Some(executable) = stock() else {
        return;
    };
    let server = harness::FixtureServer::start().await.unwrap();
    let (browser, _runtime) = harness::launch(&executable, Mode::StockCdp).await.unwrap();
    let table = PublicPage {
        id: "local-table".into(),
        url: format!("{}/public-table", server.origin),
        timeout_seconds: 10,
        extractor: Extractor::Table {
            row_selector: "#checks tr".into(),
            result_selector: "td:last-child".into(),
            pass_classes: vec!["passed".into()],
            fail_classes: vec!["failed".into()],
            pass_when_no_fail_class: false,
        },
    };
    let failure_only = PublicPage {
        id: "failure-only".into(),
        url: format!("{}/public-failure-only", server.origin),
        timeout_seconds: 10,
        extractor: Extractor::Table {
            row_selector: "#checks tr".into(),
            result_selector: "td:last-child".into(),
            pass_classes: vec![],
            fail_classes: vec!["failed".into()],
            pass_when_no_fail_class: true,
        },
    };
    let pending = PublicPage {
        id: "pending".into(),
        url: format!("{}/public-pending", server.origin),
        timeout_seconds: 1,
        extractor: Extractor::Table {
            row_selector: "#checks tr".into(),
            result_selector: "td:last-child".into(),
            pass_classes: vec![],
            fail_classes: vec!["failed".into()],
            pass_when_no_fail_class: true,
        },
    };
    let numeric = PublicPage {
        id: "local-numeric".into(),
        url: format!("{}/public-score", server.origin),
        timeout_seconds: 10,
        extractor: Extractor::Numeric {
            passed_selector: "#passed".into(),
            total_selector: "#total".into(),
        },
    };
    let missing = PublicPage {
        id: "missing-score".into(),
        url: format!("{}/public-score", server.origin),
        timeout_seconds: 1,
        extractor: Extractor::Numeric {
            passed_selector: "#absent".into(),
            total_selector: "#total".into(),
        },
    };
    let table_result = harness::extract_public(&browser, &table).await;
    let failure_only_result = harness::extract_public(&browser, &failure_only).await;
    let pending_result = harness::extract_public(&browser, &pending).await;
    let numeric_result = harness::extract_public(&browser, &numeric).await;
    let missing_result = harness::extract_public(&browser, &missing).await;
    browser.shutdown().await.unwrap();
    assert_eq!(table_result.status, "scored");
    assert_eq!(
        table_result.score,
        Some(json!({"passed":1,"total":2,"ratio":0.5}))
    );
    assert_eq!(table_result.observed[1]["passed"], false);
    assert_eq!(failure_only_result.status, "scored");
    assert_eq!(
        failure_only_result.score,
        Some(json!({"passed":1,"total":2,"ratio":0.5}))
    );
    assert_eq!(pending_result.status, "extraction_error");
    assert!(
        pending_result.score.is_none(),
        "an unfinished failure-only table must not be counted as passing"
    );
    assert_eq!(numeric_result.status, "scored");
    assert_eq!(
        numeric_result.score,
        Some(json!({"passed":7,"total":10,"ratio":0.7}))
    );
    assert_eq!(missing_result.status, "extraction_error");
    assert!(
        missing_result.score.is_none(),
        "missing public score must not become a synthetic passing score"
    );
}
