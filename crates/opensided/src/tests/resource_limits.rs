use super::*;
use nix::sys::resource::{getrlimit, setrlimit, Resource};
use std::process::Command;

const CHILD_CASE: &str = "OPENSIDE_RESOURCE_LIMIT_TEST_CASE";

fn child(test: &str, case: &str) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--test-threads=1"])
        .env(CHILD_CASE, case)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated {case} failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    if case == "descriptor-lifetimes" {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }
}

#[test]
fn daemon_raises_open_file_limit_without_lowering_existing_soft_limit() {
    let Ok(case) = std::env::var(CHILD_CASE) else {
        for case in ["low", "bounded", "already-high"] {
            child(
                "tests::resource_limits::daemon_raises_open_file_limit_without_lowering_existing_soft_limit",
                case,
            );
        }
        return;
    };
    let (_, original_hard) = getrlimit(Resource::RLIMIT_NOFILE).unwrap();
    let hard = if case == "bounded" {
        original_hard.min(2048)
    } else {
        original_hard
    };
    let soft = hard.min(if case == "already-high" { 16384 } else { 256 });
    setrlimit(Resource::RLIMIT_NOFILE, soft, hard).unwrap();
    let expected = soft.max(hard.min(8192));
    let root = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let daemon = daemon(root.path(), &TestRuntime::default()).await;
        assert_eq!(
            getrlimit(Resource::RLIMIT_NOFILE).unwrap(),
            (expected, hard)
        );
        daemon.shutdown().await;
    });
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn short_lived_daemon_runtimes_release_their_database_descriptors() {
    if std::env::var(CHILD_CASE).as_deref() != Ok("descriptor-lifetimes") {
        child(
            "tests::resource_limits::short_lived_daemon_runtimes_release_their_database_descriptors",
            "descriptor-lifetimes",
        );
        return;
    }
    let (_, hard) = getrlimit(Resource::RLIMIT_NOFILE).unwrap();
    setrlimit(Resource::RLIMIT_NOFILE, hard.min(256), hard).unwrap();
    let descriptors = || std::fs::read_dir("/dev/fd").unwrap().count();
    let cycle = |graceful: bool| {
        let root = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let daemon = daemon(root.path(), &TestRuntime::default()).await;
            if graceful {
                daemon.shutdown().await;
            }
        });
        drop(runtime);
    };
    let initial = descriptors();
    // Signal and async-I/O reactors retain process-wide descriptors after their first use.
    cycle(true);
    let before = descriptors();
    for iteration in 0..16 {
        cycle(iteration % 2 == 0);
        let current = descriptors();
        assert!(
            current <= before + 2,
            "descriptor growth after cycle {iteration}: warmed={before}, current={current}"
        );
    }
    let after = descriptors();
    eprintln!("daemon descriptor lifetimes: initial={initial}, warmed={before}, after={after}");
    assert!(
        after <= before + 2,
        "descriptor leak: before={before}, after={after}"
    );
}
