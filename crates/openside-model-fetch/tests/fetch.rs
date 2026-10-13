//! Checksum verification, progress and cancellation of the model download against a local hub.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    body::Body,
    extract::{Path as UrlPath, State},
    http::{header, Response},
    routing::get,
    Router,
};
use openside_model_fetch::{fetch, CancellationToken, Failure, Phase, Progress, Source};
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha1::Sha1;
use sha2::{Digest as _, Sha256};
use tokio::{net::TcpListener, task::JoinHandle};

/// WordPiece tokenizer and an F32 safetensors file in the layout `StaticEmbedder` loads.
fn model_files() -> (Vec<u8>, Vec<u8>) {
    let words = ["[PAD]", "[UNK]", "car", "garage"];
    let vocab: serde_json::Map<String, Value> = words
        .iter()
        .enumerate()
        .map(|(id, word)| ((*word).to_owned(), json!(id)))
        .collect();
    let tokenizer = serde_json::to_vec(&json!({
        "normalizer": {"type": "BertNormalizer", "lowercase": true},
        "pre_tokenizer": {"type": "BertPreTokenizer"},
        "model": {"type": "WordPiece", "unk_token": "[UNK]", "continuing_subword_prefix": "##", "vocab": vocab}
    }))
    .unwrap();
    let rows: [[f32; 2]; 4] = [[0.0, 0.0], [0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
    let data: Vec<u8> = rows
        .iter()
        .flatten()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let mut header = serde_json::to_vec(&json!({
        "embeddings": {"dtype": "F32", "shape": [4, 2], "data_offsets": [0, data.len()]}
    }))
    .unwrap();
    while header.len() % 8 != 0 {
        header.push(b' ');
    }
    let mut model = (header.len() as u64).to_le_bytes().to_vec();
    model.extend(header);
    model.extend(data);
    (tokenizer, model)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// What the hub's git storage publishes for a plain (non-LFS) file.
fn git_blob_id(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", bytes.len()).as_bytes());
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// The listing a hub publishes: the tokenizer as a plain git file, the weights as an LFS object.
fn listing(tokenizer: &[u8], model: &[u8]) -> Value {
    json!({
        "sha": "0123456789abcdef0123456789abcdef01234567",
        "siblings": [
            {"rfilename": "tokenizer.json", "size": tokenizer.len(), "blobId": git_blob_id(tokenizer)},
            {"rfilename": "model.safetensors", "size": model.len(), "blobId": "a".repeat(40),
             "lfs": {"sha256": sha256(model), "size": model.len()}},
        ]
    })
}

struct Hub {
    endpoint: String,
    requested: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl Drop for Hub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone)]
struct HubState {
    tokenizer: Vec<u8>,
    model: Vec<u8>,
    listing: Option<Value>,
    /// The weights stall after their first bytes while this is set.
    stall: Arc<AtomicBool>,
    requested: Arc<Mutex<Vec<String>>>,
}

async fn hub(tokenizer: Vec<u8>, model: Vec<u8>, listing: Option<Value>, stall: bool) -> Hub {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let requested: Arc<Mutex<Vec<String>>> = Arc::default();
    let state = HubState {
        tokenizer,
        model,
        listing,
        stall: Arc::new(AtomicBool::new(stall)),
        requested: requested.clone(),
    };
    let router = Router::new()
        .route(
            "/api/models/{owner}/{name}/revision/{revision}",
            get(|State(state): State<HubState>| async move {
                match state.listing {
                    Some(listing) => Response::builder()
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(listing.to_string()))
                        .unwrap(),
                    None => Response::builder().status(404).body(Body::empty()).unwrap(),
                }
            }),
        )
        .route(
            "/{owner}/{name}/resolve/{revision}/{file}",
            get(
                |State(state): State<HubState>,
                 UrlPath((_, _, revision, file)): UrlPath<(String, String, String, String)>| async move {
                    state.requested.lock().push(format!("{revision}/{file}"));
                    match file.as_str() {
                        "tokenizer.json" => Response::new(Body::from(state.tokenizer)),
                        "model.safetensors" => {
                            let head = state.model[..16].to_vec();
                            let rest = state.model[16..].to_vec();
                            let stall = state.stall;
                            let stream = futures_util::stream::unfold(0u8, move |step| {
                                let (head, rest, stall) = (head.clone(), rest.clone(), stall.clone());
                                async move {
                                    match step {
                                        0 => Some((Ok::<_, std::io::Error>(head), 1)),
                                        1 => {
                                            while stall.load(Ordering::SeqCst) {
                                                tokio::time::sleep(Duration::from_millis(10)).await;
                                            }
                                            Some((Ok(rest), 2))
                                        }
                                        _ => None,
                                    }
                                }
                            });
                            Response::new(Body::from_stream(stream))
                        }
                        _ => Response::builder().status(404).body(Body::empty()).unwrap(),
                    }
                },
            ),
        )
        .with_state(state);
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Hub {
        endpoint,
        requested,
        task,
    }
}

fn source(hub: &Hub) -> Source {
    Source {
        endpoint: hub.endpoint.clone(),
        repo: "acme/static-mini".into(),
        revision: "main".into(),
    }
}

async fn run(
    hub: &Hub,
    target: &std::path::Path,
    cancel: &CancellationToken,
) -> (
    Result<openside_model_fetch::Installed, Failure>,
    Vec<Progress>,
) {
    let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
    let sink = seen.clone();
    let result = fetch(
        &source(hub),
        &[],
        target,
        &move |progress| sink.lock().push(progress),
        cancel,
    )
    .await;
    let progress = seen.lock().clone();
    (result, progress)
}

#[tokio::test]
async fn installs_a_model_whose_checksums_match_and_reports_progress() {
    let (tokenizer, model) = model_files();
    let hub = hub(
        tokenizer.clone(),
        model.clone(),
        Some(listing(&tokenizer, &model)),
        false,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("models").join("static");

    let (result, progress) = run(&hub, &target, &CancellationToken::new()).await;
    let installed = result.unwrap();
    assert!(installed.model_id.starts_with("static-"));
    assert_eq!(
        installed.revision, "0123456789abcdef0123456789abcdef01234567",
        "downloads the commit the checksums describe, not the branch"
    );
    assert_eq!(
        std::fs::read(target.join("tokenizer.json")).unwrap(),
        tokenizer
    );
    assert_eq!(
        std::fs::read(target.join("model.safetensors")).unwrap(),
        model
    );
    assert_eq!(
        *hub.requested.lock(),
        [
            "0123456789abcdef0123456789abcdef01234567/tokenizer.json",
            "0123456789abcdef0123456789abcdef01234567/model.safetensors"
        ]
    );

    let phases: Vec<Phase> = progress.iter().map(|step| step.phase).collect();
    assert_eq!(phases.first(), Some(&Phase::Resolving));
    assert_eq!(phases.last(), Some(&Phase::Installing));
    let last_download = progress
        .iter()
        .rev()
        .find(|step| step.phase == Phase::Downloading)
        .unwrap();
    assert_eq!(last_download.files_done, 2);
    assert_eq!(
        last_download.bytes_done,
        (tokenizer.len() + model.len()) as u64
    );
    assert_eq!(last_download.bytes_total, last_download.bytes_done);
}

#[tokio::test]
async fn a_checksum_mismatch_installs_nothing_and_keeps_the_current_model() {
    let (tokenizer, model) = model_files();
    // The source claims different weights than the ones it serves.
    let mut tampered = model.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xff;
    let hub = hub(
        tokenizer.clone(),
        tampered,
        Some(listing(&tokenizer, &model)),
        false,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("models").join("static");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("marker"), b"current").unwrap();

    let (result, _) = run(&hub, &target, &CancellationToken::new()).await;
    let error = result.unwrap_err();
    assert!(matches!(error, Failure::ChecksumMismatch(_)), "{error:?}");
    assert_eq!(error.code(), "checksum_mismatch");
    assert_eq!(std::fs::read(target.join("marker")).unwrap(), b"current");
    assert!(!target.join("model.safetensors").exists());
    let staging: Vec<_> =
        std::fs::read_dir(directory.path().join("models").join(".static.generations"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
    assert!(staging.is_empty(), "staging is removed: {staging:?}");
}

#[tokio::test]
async fn a_plain_file_is_checked_against_its_git_blob_id() {
    let (tokenizer, model) = model_files();
    let mut published = listing(&tokenizer, &model);
    published["siblings"][0]["blobId"] = json!("b".repeat(40));
    let hub = hub(tokenizer, model, Some(published), false).await;
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("static");

    let (result, _) = run(&hub, &target, &CancellationToken::new()).await;
    assert!(matches!(result, Err(Failure::ChecksumMismatch(_))));
    assert!(!target.exists());
}

#[tokio::test]
async fn a_source_without_checksums_is_refused_before_any_download() {
    let (tokenizer, model) = model_files();
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("static");

    for published in [
        None,
        Some(
            json!({"siblings": [{"rfilename": "tokenizer.json"}, {"rfilename": "model.safetensors"}]}),
        ),
        Some(json!({"nothing": true})),
    ] {
        let missing_listing = published.is_none();
        let hub = hub(tokenizer.clone(), model.clone(), published, false).await;
        let (result, _) = run(&hub, &target, &CancellationToken::new()).await;
        let error = result.unwrap_err();
        if missing_listing {
            assert_eq!(error.code(), "invalid_source", "{error:?}");
        } else {
            assert_eq!(error.code(), "checksum_unavailable", "{error:?}");
        }
        assert!(hub.requested.lock().is_empty(), "no file was requested");
    }
    assert!(!target.exists());
}

#[tokio::test]
async fn cancelling_stops_the_transfer_and_leaves_no_model() {
    let (tokenizer, model) = model_files();
    let hub = hub(
        tokenizer.clone(),
        model.clone(),
        Some(listing(&tokenizer, &model)),
        true,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("models").join("static");
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
    let sink = seen.clone();
    let task = tokio::spawn({
        let source = source(&hub);
        let target = target.clone();
        async move {
            fetch(
                &source,
                &[],
                &target,
                &move |progress| {
                    // Cancel once the first bytes of the weights arrived.
                    if progress.file.as_deref() == Some("model.safetensors")
                        && progress.bytes_done > 0
                    {
                        trigger.cancel();
                    }
                    sink.lock().push(progress);
                },
                &cancel,
            )
            .await
        }
    });
    let result = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("a cancelled fetch returns promptly")
        .unwrap();
    assert_eq!(result.unwrap_err(), Failure::Cancelled);
    assert!(!target.exists());
    assert!(seen.lock().iter().any(|step| step.bytes_done > 0));
}

#[tokio::test]
async fn rejects_unsafe_sources() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("static");
    for (endpoint, repo, revision) in [
        ("http://example.invalid", "acme/x", "main"),
        ("https://hub.example.test", "../etc", "main"),
        ("https://hub.example.test", "acme/x", "a/b"),
        ("not a url", "acme/x", "main"),
    ] {
        let error = fetch(
            &Source {
                endpoint: endpoint.into(),
                repo: repo.into(),
                revision: revision.into(),
            },
            &[],
            &target,
            &|_| {},
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.code(),
            "invalid_source",
            "{endpoint} {repo} {revision}"
        );
    }
}
