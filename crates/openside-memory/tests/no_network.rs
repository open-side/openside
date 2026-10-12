//! A-11 ④: embedding and memory operations perform no network I/O.
//!
//! Structural: the crate's normal-dependency closure (from `cargo metadata`) contains
//! no HTTP/TLS client, async network runtime or model downloader. Behavioral (Linux):
//! a watcher samples `/proc/self/fd` while every memory operation runs and sees no
//! socket; a positive control proves the watcher detects one. This file is its own
//! test binary, so no unrelated test shares the process.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::{BTreeSet, HashMap, VecDeque};

use serde_json::Value;

const NETWORK_CRATES: &[&str] = &[
    "reqwest",
    "hyper",
    "hyper-util",
    "h2",
    "h3",
    "http",
    "ureq",
    "curl",
    "curl-sys",
    "isahc",
    "surf",
    "attohttpc",
    "hf-hub",
    "ort",
    "ort-sys",
    "native-tls",
    "openssl",
    "openssl-sys",
    "rustls",
    "tokio-rustls",
    "tungstenite",
    "tokio-tungstenite",
    "tokio",
    "mio",
    "socket2",
    "async-std",
    "smol",
];

#[test]
fn dependency_graph_has_no_network_clients() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = std::process::Command::new(rustc)
        .arg("-vV")
        .output()
        .unwrap();
    let host = String::from_utf8(version.stdout)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
        .unwrap();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    // Offline and host-filtered: the check itself must not download foreign-target crates.
    let output = std::process::Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--offline",
            "--filter-platform",
        ])
        .arg(host)
        .arg("--manifest-path")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
    let names: HashMap<&str, &str> = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|package| {
            (
                package["id"].as_str().unwrap(),
                package["name"].as_str().unwrap(),
            )
        })
        .collect();
    let nodes: HashMap<&str, &Value> = metadata["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| (node["id"].as_str().unwrap(), node))
        .collect();
    let root = names
        .iter()
        .find(|(_, name)| **name == "openside-memory")
        .map(|(id, _)| *id)
        .unwrap();
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([root]);
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        for dep in nodes[id]["deps"].as_array().unwrap() {
            // Normal edges only: build tools (the C compiler driver) never run in-process.
            let normal = dep["dep_kinds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|kind| kind["kind"].is_null());
            if normal {
                queue.push_back(dep["pkg"].as_str().unwrap());
            }
        }
    }
    let closure: BTreeSet<&str> = seen.iter().map(|id| names[id]).collect();
    for expected in ["rusqlite", "libsqlite3-sys", "regex", "openside-proto"] {
        assert!(
            closure.contains(expected),
            "walk missed {expected}: {closure:?}"
        );
    }
    let forbidden: Vec<_> = NETWORK_CRATES
        .iter()
        .filter(|name| closure.contains(*name))
        .collect();
    assert!(
        forbidden.is_empty(),
        "network-capable crates in openside-memory: {forbidden:?}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn memory_operations_open_no_sockets() {
    use std::{
        net::TcpListener,
        sync::{
            atomic::{AtomicBool, AtomicU8, Ordering},
            Arc,
        },
    };

    use parking_lot::Mutex;

    use openside_memory::{MemoryStore, NewMemory, Options, StaticEmbedder};
    use openside_proto::UpdateMemoryRequest;

    // An fd that closes between listing and readlink is skipped; an unreadable /proc is a
    // failed observation, never an empty socket set.
    fn sockets() -> BTreeSet<String> {
        std::fs::read_dir("/proc/self/fd")
            .expect("/proc/self/fd must be readable to observe sockets")
            .filter_map(|entry| std::fs::read_link(entry.ok()?.path()).ok())
            .map(|target| target.to_string_lossy().into_owned())
            .filter(|target| target.starts_with("socket:"))
            .collect()
    }

    let baseline = sockets();
    let phase = Arc::new(AtomicU8::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let observed: Arc<Mutex<[BTreeSet<String>; 2]>> = Arc::default();
    let watcher = {
        let (phase, stop, observed) = (phase.clone(), stop.clone(), observed.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                // Sample before reading the phase: a socket seen here was opened before
                // this phase load, so the control socket is never charged to phase 0.
                let new: Vec<String> = sockets().difference(&baseline).cloned().collect();
                let current = phase.load(Ordering::SeqCst) as usize;
                observed.lock()[current].extend(new);
                std::thread::yield_now();
            }
        })
    };

    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("model");
    common::write_model(&model, "F32");
    let embedder = Arc::new(StaticEmbedder::load(&model).unwrap());
    let store = MemoryStore::open(
        &dir.path().join("memory.db"),
        Options {
            embedder: Some(embedder),
            ..Options::default()
        },
    )
    .unwrap();
    for round in 0..50 {
        let memory = store
            .create(
                NewMemory::note(format!("automobile parked {round}")),
                "test",
            )
            .unwrap();
        store.search("car parking", None, 10).unwrap();
        store
            .update(
                &memory.id,
                UpdateMemoryRequest {
                    content: Some(format!("kitten on the sofa {round}")),
                    ..Default::default()
                },
                "test",
            )
            .unwrap();
        store
            .record_visit(
                &format!("https://example.test/{round}"),
                Some("t"),
                None,
                "test",
            )
            .unwrap();
        store.history(&memory.id).unwrap();
        store.delete(&memory.id, "test").unwrap();
        store.purge_expired().unwrap();
    }
    drop(store);

    // Positive control: the same watcher must see a socket opened in phase 1.
    phase.store(1, Ordering::SeqCst);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while observed.lock()[1].is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    drop(listener);
    stop.store(true, Ordering::SeqCst);
    watcher.join().unwrap();
    let observed = observed.lock();
    assert!(
        !observed[1].is_empty(),
        "watcher failed to detect a known socket"
    );
    assert!(
        observed[0].is_empty(),
        "memory operations opened sockets: {:?}",
        observed[0]
    );
}
