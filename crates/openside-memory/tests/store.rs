//! Memory store behavior: hybrid recall, retention, CRUD history, secret blocking.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc,
};

use chrono::{DateTime, Duration, TimeZone, Utc};
use openside_memory::{Clock, Embedder, Error, MemoryStore, NewMemory, Options, StaticEmbedder};
use openside_proto::{
    MemoryKind, MemoryMatch, MemoryOp, MemorySearchMode, Retention, UpdateMemoryRequest,
};

struct FakeClock(AtomicI64);
impl FakeClock {
    fn new() -> Arc<Self> {
        Arc::new(Self(AtomicI64::new(
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .unwrap()
                .timestamp_millis(),
        )))
    }
    fn advance(&self, by: Duration) {
        self.0.fetch_add(by.num_milliseconds(), Ordering::SeqCst);
    }
}
impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        Utc.timestamp_millis_opt(self.0.load(Ordering::SeqCst))
            .unwrap()
    }
}

fn embedder(dir: &std::path::Path) -> Arc<dyn Embedder> {
    let model = dir.join("model");
    common::write_model(&model, "F32");
    Arc::new(StaticEmbedder::load(&model).unwrap())
}

fn open(dir: &std::path::Path, options: Options) -> MemoryStore {
    MemoryStore::open(&dir.join("memory.db"), options).unwrap()
}

#[test]
fn bundled_sqlite_has_fts5() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let options: Vec<String> = conn
        .prepare("PRAGMA compile_options")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        options.iter().any(|option| option == "ENABLE_FTS5"),
        "{options:?}"
    );
}

#[test]
fn vector_recall_beats_lexical_on_a_paraphrase() {
    let dir = tempfile::tempdir().unwrap();
    let documents = [
        "The automobile is parked in the garage",
        "The kitten sleeps on the sofa",
    ];
    std::fs::create_dir_all(dir.path().join("lexical")).unwrap();
    let lexical = open(&dir.path().join("lexical"), Options::default());
    let hybrid = open(
        dir.path(),
        Options {
            embedder: Some(embedder(dir.path())),
            ..Options::default()
        },
    );
    for document in documents {
        lexical.create(NewMemory::note(document), "test").unwrap();
        hybrid.create(NewMemory::note(document), "test").unwrap();
    }
    let query = "car parking";
    let plain = lexical.search(query, None, 5).unwrap();
    assert_eq!(plain.mode, MemorySearchMode::LexicalOnly);
    assert!(
        plain.hits.is_empty(),
        "paraphrase shares no term, lexical must miss: {:?}",
        plain.hits
    );
    let semantic = hybrid.search(query, None, 5).unwrap();
    assert_eq!(semantic.mode, MemorySearchMode::Hybrid);
    assert_eq!(
        semantic.hits.len(),
        1,
        "unrelated memory must stay below MIN_COSINE"
    );
    assert_eq!(semantic.hits[0].memory.content, documents[0]);
    assert_eq!(semantic.hits[0].matched, MemoryMatch::Vector);

    let couch = hybrid.search("cat couch", None, 5).unwrap();
    assert_eq!(couch.hits[0].memory.content, documents[1]);
    // A shared literal term plus semantics are both reported.
    let both = hybrid.search("kitten", None, 5).unwrap();
    assert_eq!(both.hits[0].matched, MemoryMatch::Both);
}

#[test]
fn half_precision_model_loads_with_the_same_geometry() {
    let dir = tempfile::tempdir().unwrap();
    common::write_model(&dir.path().join("f16"), "F16");
    let half = StaticEmbedder::load(&dir.path().join("f16")).unwrap();
    let full = embedder(dir.path());
    let a = half.embed("automobile garage").unwrap();
    let b = full.embed("Automobile, garage!").unwrap();
    let cosine: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    assert!(cosine > 0.999, "{cosine}");
    assert!(full.embed("zzz qqq").is_none());
    assert_ne!(half.id(), full.id(), "model id is content-derived");
}

#[test]
fn missing_or_unsupported_model_reports_reason_without_contents() {
    let dir = tempfile::tempdir().unwrap();
    let error = StaticEmbedder::load(dir.path()).unwrap_err();
    assert!(error.0.contains("tokenizer.json"), "{error}");
    std::fs::write(
        dir.path().join("tokenizer.json"),
        r#"{"model":{"type":"Unigram","vocab":[["secret-ish",0.0]]}}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("model.safetensors"), b"x").unwrap();
    let error = StaticEmbedder::load(dir.path()).unwrap_err();
    assert!(error.0.contains("only WordPiece"), "{error}");
    assert!(!error.0.contains("secret-ish"));
}

#[test]
fn lexical_search_filters_kind_and_lists_recent() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), Options::default());
    store
        .create(NewMemory::note("User prefers dark mode in editors"), "test")
        .unwrap();
    store
        .record_visit(
            "https://docs.example.test/dark-mode",
            Some("Dark mode guide"),
            Some("How to enable dark mode"),
            "browser_history:s1",
        )
        .unwrap();
    let all = store.search("dark", None, 10).unwrap();
    assert_eq!(all.hits.len(), 2);
    assert!(all
        .hits
        .iter()
        .all(|hit| hit.matched == MemoryMatch::Lexical));
    let notes = store.search("dark", Some(MemoryKind::Note), 10).unwrap();
    assert_eq!(notes.hits.len(), 1);
    assert_eq!(notes.hits[0].memory.kind, MemoryKind::Note);
    // Prefix matching tolerates suffixes (e.g. Korean particles) and query syntax is inert.
    store
        .create(NewMemory::note("검색은 로컬에서 동작한다"), "test")
        .unwrap();
    assert_eq!(store.search("검색", None, 10).unwrap().hits.len(), 1);
    assert!(store.search("\"dark\" OR NEAR(", None, 10).is_ok());
    let recent = store.search("", None, 2).unwrap();
    assert_eq!(recent.hits.len(), 2);
    assert!(recent
        .hits
        .iter()
        .all(|hit| hit.matched == MemoryMatch::Recent));
}

#[test]
fn retention_purges_with_injected_clock_and_update_restarts_it() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let store = open(
        dir.path(),
        Options {
            clock: clock.clone(),
            default_retention: Retention::Days90,
            ..Options::default()
        },
    );
    let mut short = NewMemory::note("thirty day note");
    short.retention = Some(Retention::Days30);
    let short = store.create(short, "test").unwrap();
    let default = store
        .create(NewMemory::note("default ninety"), "test")
        .unwrap();
    assert_eq!(default.retention, Retention::Days90);
    let mut forever = NewMemory::note("permanent note");
    forever.retention = Some(Retention::Permanent);
    let forever = store.create(forever, "test").unwrap();
    assert!(forever.expires_at.is_none());
    assert_eq!(
        short.expires_at.unwrap() - short.created_at,
        Duration::days(30)
    );

    clock.advance(Duration::days(29));
    assert_eq!(store.purge_expired().unwrap(), 0);
    // Rewriting at day 29 restarts the 90-day window for `default`.
    store
        .update(
            &default.id,
            UpdateMemoryRequest {
                content: Some("default ninety, edited".into()),
                ..Default::default()
            },
            "test",
        )
        .unwrap();
    clock.advance(Duration::days(2));
    // Expired memories are hidden before the purge job runs.
    assert_eq!(store.get(&short.id).unwrap_err(), Error::NotFound);
    assert_eq!(store.purge_expired().unwrap(), 1);
    let history = store.history(&short.id).unwrap();
    let last = history.versions.last().unwrap();
    assert_eq!(
        (last.op, last.actor.as_str()),
        (MemoryOp::Purge, "retention")
    );
    assert!(history.versions.iter().all(|v| v.content.is_none()));

    clock.advance(Duration::days(87));
    assert_eq!(
        store.purge_expired().unwrap(),
        0,
        "edit at day 29 extends the deadline to day 119"
    );
    clock.advance(Duration::days(1));
    assert_eq!(store.purge_expired().unwrap(), 1);
    clock.advance(Duration::days(3650));
    assert_eq!(store.purge_expired().unwrap(), 0);
    assert_eq!(store.get(&forever.id).unwrap().content, "permanent note");
    assert_eq!(store.count().unwrap(), 1);
}

#[test]
fn crud_records_versions_with_time_and_actor() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let store = open(
        dir.path(),
        Options {
            clock: clock.clone(),
            ..Options::default()
        },
    );
    let mut new = NewMemory::note("first draft");
    new.tags = vec!["work".into(), " work ".into(), "pref".into()];
    let created = store.create(new, "agent:s1").unwrap();
    assert_eq!(created.tags, vec!["work", "pref"]);
    assert_eq!(created.version, 1);
    clock.advance(Duration::minutes(5));
    let updated = store
        .update(
            &created.id,
            UpdateMemoryRequest {
                content: Some("second draft".into()),
                tags: Some(vec!["home".into()]),
                retention: Some(Retention::Days30),
            },
            "api",
        )
        .unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(store.get(&created.id).unwrap(), updated);
    assert_eq!(
        updated.updated_at - created.created_at,
        Duration::minutes(5)
    );
    assert!(matches!(
        store.update(
            &created.id,
            UpdateMemoryRequest {
                content: Some("  ".into()),
                ..Default::default()
            },
            "api"
        ),
        Err(Error::Invalid(_))
    ));

    clock.advance(Duration::minutes(5));
    store.delete(&created.id, "cli").unwrap();
    assert_eq!(store.get(&created.id).unwrap_err(), Error::NotFound);
    assert_eq!(
        store.delete(&created.id, "cli").unwrap_err(),
        Error::NotFound
    );
    assert!(store.search("draft", None, 10).unwrap().hits.is_empty());
    let history = store.history(&created.id).unwrap();
    let ops: Vec<_> = history
        .versions
        .iter()
        .map(|v| (v.version, v.op, v.actor.as_str()))
        .collect();
    assert_eq!(
        ops,
        vec![
            (1, MemoryOp::Create, "agent:s1"),
            (2, MemoryOp::Update, "api"),
            (3, MemoryOp::Delete, "cli"),
        ]
    );
    assert_eq!(history.versions[1].at, updated.updated_at);
    assert_eq!(
        history.versions[2].at - history.versions[1].at,
        Duration::minutes(5)
    );
    // Deletion scrubs every stored snapshot of the content.
    assert!(history
        .versions
        .iter()
        .all(|v| v.content.is_none() && v.tags.is_none()));
    assert_eq!(store.history("mem_missing").unwrap_err(), Error::NotFound);
}

#[test]
fn history_visits_upsert_by_url_with_thirty_day_retention() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), Options::default());
    let url = "https://news.example.test/article";
    let first = store
        .record_visit(url, Some("Article"), None, "browser_history:s1")
        .unwrap();
    assert_eq!(first.kind, MemoryKind::History);
    assert_eq!(first.retention, Retention::Days30);
    assert_eq!(first.content, "Article");
    let second = store
        .record_visit(url, None, Some("Body summary text"), "browser_history:s1")
        .unwrap();
    assert_eq!(second.id, first.id);
    assert_eq!(second.title.as_deref(), Some("Article"));
    assert_eq!(second.content, "Body summary text");
    assert_eq!(
        store
            .search("", Some(MemoryKind::History), 10)
            .unwrap()
            .hits
            .len(),
        1
    );
}

#[test]
fn backfill_embeds_existing_memories_when_a_model_appears() {
    let dir = tempfile::tempdir().unwrap();
    open(dir.path(), Options::default())
        .create(
            NewMemory::note("The automobile is parked in the garage"),
            "test",
        )
        .unwrap();
    let store = open(
        dir.path(),
        Options {
            embedder: Some(embedder(dir.path())),
            ..Options::default()
        },
    );
    let hits = store.search("car", None, 5).unwrap().hits;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].matched, MemoryMatch::Vector);
}

#[test]
fn secrets_are_rejected_and_never_reach_database_files() {
    let dir = tempfile::tempdir().unwrap();
    let vault_canary = "vault-canary-q8Zr51";
    let probe = vault_canary.to_owned();
    let store = open(
        dir.path(),
        Options {
            embedder: Some(embedder(dir.path())),
            known_secret: Some(Arc::new(move |text: &str| text.contains(&probe))),
            ..Options::default()
        },
    );
    let canaries = [
        "sk-ant-api03-CANARYcanaryCANARY0001",
        "ghp_CANARYcanaryCANARYcanaryCANARY000002",
        "AKIACANARY0000000003",
        "eyJhbGciOiJIUzI1NiJ9.eyJjYW5hcnkiOiI0In0.Q0FOQVJZc2lnbmF0dXJl",
        "-----BEGIN RSA PRIVATE KEY-----\nCANARYPEM5",
        "password=CanaryPassw0rd6",
        "4111-1111-1111-1111",
        vault_canary,
    ];
    let keep = store
        .create(NewMemory::note("harmless note"), "test")
        .unwrap();
    for canary in canaries {
        let text = format!("remember this: {canary}");
        assert_eq!(
            store
                .create(NewMemory::note(text.clone()), "test")
                .unwrap_err(),
            Error::SecretDetected,
            "{canary}"
        );
        let mut tagged = NewMemory::note("tag carrier");
        tagged.tags = vec![canary.into()];
        assert_eq!(
            store.create(tagged, "test").unwrap_err(),
            Error::SecretDetected
        );
        assert_eq!(
            store
                .update(
                    &keep.id,
                    UpdateMemoryRequest {
                        content: Some(text.clone()),
                        ..Default::default()
                    },
                    "test"
                )
                .unwrap_err(),
            Error::SecretDetected
        );
        assert_eq!(
            store
                .record_visit("https://example.test/", Some("t"), Some(&text), "test")
                .unwrap_err(),
            Error::SecretDetected
        );
        assert!(!Error::SecretDetected.to_string().contains(canary));
    }
    assert_eq!(store.get(&keep.id).unwrap().content, "harmless note");
    assert_eq!(store.history(&keep.id).unwrap().versions.len(), 1);
    drop(store);
    let mut checked = 0;
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_file() || !path.to_string_lossy().contains("memory.db") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        checked += bytes.len();
        for canary in canaries {
            let needle = canary.as_bytes();
            assert!(
                !bytes.windows(needle.len()).any(|window| window == needle),
                "{canary} found in {}",
                path.display()
            );
        }
    }
    assert!(checked > 0, "database file was inspected");
}

fn database_bytes(dir: &std::path::Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("memory.db")
        {
            bytes.extend(std::fs::read(path).unwrap());
        }
    }
    bytes
}

fn holds(bytes: &[u8], needle: &str) -> bool {
    bytes
        .windows(needle.len())
        .any(|part| part == needle.as_bytes())
}

#[test]
fn expired_memories_vanish_from_history_and_vector_search_before_the_purge_job() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let store = open(
        dir.path(),
        Options {
            clock: clock.clone(),
            embedder: Some(embedder(dir.path())),
            ..Options::default()
        },
    );
    let note = store
        .create(
            NewMemory {
                retention: Some(Retention::Days30),
                ..NewMemory::note("my car is parked in the garage")
            },
            "test",
        )
        .unwrap();
    assert_eq!(
        store.history(&note.id).unwrap().versions[0]
            .content
            .as_deref(),
        Some("my car is parked in the garage")
    );
    store
        .record_visit(
            "https://example.com/a",
            Some("Cats"),
            Some("kitten sleeps on the sofa"),
            "test",
        )
        .unwrap();
    clock.advance(Duration::days(31));
    // No purge_expired() call: reads alone must enforce retention.
    let history = store.history(&note.id).unwrap();
    assert!(
        history
            .versions
            .iter()
            .all(|version| version.content.is_none()),
        "{history:?}"
    );
    assert_eq!(history.versions.last().unwrap().op, MemoryOp::Purge);
    assert!(store
        .search("automobile garage", None, 10)
        .unwrap()
        .hits
        .is_empty());

    // A revisit after expiry is a new memory: no old summary, version 1, fresh lifetime.
    let revisit = store
        .record_visit("https://example.com/a", Some("Cats"), None, "test")
        .unwrap();
    assert_eq!(revisit.version, 1);
    assert_eq!(revisit.content, "Cats");
    assert!(!holds(
        &database_bytes(dir.path()),
        "kitten sleeps on the sofa"
    ));
}

#[test]
fn concurrent_updates_and_first_visits_are_serialized_without_lost_changes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(open(dir.path(), Options::default()));
    let note = store.create(NewMemory::note("start"), "test").unwrap();
    let workers: Vec<_> = (0..8)
        .map(|worker| {
            let (store, id) = (store.clone(), note.id.clone());
            std::thread::spawn(move || {
                let patch = if worker % 2 == 0 {
                    UpdateMemoryRequest {
                        content: Some(format!("content {worker}")),
                        ..Default::default()
                    }
                } else {
                    UpdateMemoryRequest {
                        tags: Some(vec![format!("tag{worker}")]),
                        ..Default::default()
                    }
                };
                store.update(&id, patch, "test").unwrap();
                store
                    .record_visit("https://example.com/same", Some("Same"), None, "test")
                    .unwrap();
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let history = store.history(&note.id).unwrap();
    let versions: Vec<u32> = history
        .versions
        .iter()
        .map(|version| version.version)
        .collect();
    assert_eq!(
        versions,
        (1..=9).collect::<Vec<_>>(),
        "strictly ordered, no duplicates"
    );
    let current = store.get(&note.id).unwrap();
    assert_eq!(current.version, 9);
    assert!(
        current.content.starts_with("content "),
        "content edit survived tag edits"
    );
    assert_eq!(current.tags.len(), 1, "tag edit survived content edits");
    let visits = store
        .search("", Some(MemoryKind::History), 10)
        .unwrap()
        .hits;
    assert_eq!(visits.len(), 1, "one row per URL");
    assert_eq!(visits[0].memory.version, 8);
}

#[test]
fn deleted_and_purged_content_is_erased_from_database_files() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let options = || Options {
        clock: clock.clone(),
        ..Options::default()
    };
    let store = open(dir.path(), options());
    let deleted = store
        .create(NewMemory::note("erasure canary alpha zebra-7193"), "test")
        .unwrap();
    store
        .update(
            &deleted.id,
            UpdateMemoryRequest {
                content: Some("erasure canary beta zebra-8264".into()),
                ..Default::default()
            },
            "test",
        )
        .unwrap();
    store
        .create(
            NewMemory {
                retention: Some(Retention::Days30),
                ..NewMemory::note("erasure canary gamma zebra-9375")
            },
            "test",
        )
        .unwrap();
    let before = database_bytes(dir.path());
    for canary in ["zebra-7193", "zebra-8264", "zebra-9375"] {
        assert!(holds(&before, canary), "{canary} must first be on disk");
    }
    store.delete(&deleted.id, "test").unwrap();
    clock.advance(Duration::days(31));
    assert_eq!(store.purge_expired().unwrap(), 1);
    drop(store);
    let reopened = open(dir.path(), options());
    assert_eq!(reopened.count().unwrap(), 0);
    let after = database_bytes(dir.path());
    for canary in ["zebra-7193", "zebra-8264", "zebra-9375"] {
        assert!(
            !holds(&after, canary),
            "{canary} survived in database or journal bytes"
        );
    }
    let mode: String = rusqlite::Connection::open(dir.path().join("memory.db"))
        .unwrap()
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
}

#[test]
fn secrets_split_across_fields_encoded_or_in_actor_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), Options::default());
    let cases = [
        NewMemory {
            title: Some("password".into()),
            ..NewMemory::note(": hunter2-split")
        },
        NewMemory::note(r#"{"password": "hunter2-json"}"#),
        NewMemory::note("token%3Dghp_abcdefghijklmnopqrstuvwxyz0123456789"),
        NewMemory::note("gh\u{200B}p_abcdefghijklmnopqrstuvwxyz0123456789"),
        NewMemory {
            tags: vec!["pass".into()],
            ..NewMemory::note(r"\u0070assword=hunter2-escaped")
        },
    ];
    for case in cases {
        assert_eq!(
            store.create(case, "test").unwrap_err(),
            Error::SecretDetected
        );
    }
    assert_eq!(
        store
            .create(
                NewMemory::note("ordinary"),
                "agent:ghp_abcdefghijklmnopqrstuvwxyz0123456789"
            )
            .unwrap_err(),
        Error::SecretDetected
    );
    assert_eq!(
        store
            .create(NewMemory::note("ordinary"), "free text actor")
            .unwrap_err(),
        Error::Invalid("actor must be 1-128 identifier characters")
    );
    assert_eq!(store.count().unwrap(), 0);
    let bytes = database_bytes(dir.path());
    for canary in ["hunter2", "ghp_abcdefghij"] {
        assert!(!holds(&bytes, canary), "{canary} reached the database");
    }
}

#[test]
fn tokenizer_transformations_it_cannot_reproduce_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("model");
    common::write_model(&model, "F32");
    let path = model.join(openside_memory::TOKENIZER_FILE);
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for (section, kind) in [("pre_tokenizer", "Metaspace"), ("normalizer", "NFKC")] {
        let mut tokenizer = original.clone();
        tokenizer[section] = serde_json::json!({"type": kind});
        std::fs::write(&path, serde_json::to_vec(&tokenizer).unwrap()).unwrap();
        let Err(error) = StaticEmbedder::load(&model) else {
            panic!("unsupported {kind} tokenizer must not load")
        };
        assert!(error.to_string().contains(kind), "{error}");
    }
}

#[test]
fn team_memory_scope_precedes_limit_and_transfer_replays_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), Options::default());
    let shared = store
        .create(NewMemory::note("shared project"), "test")
        .unwrap();
    let own = store
        .create_owned(NewMemory::note("owned project"), "test", Some("mina"))
        .unwrap();
    for _ in 0..60 {
        store
            .create_owned(NewMemory::note("other project"), "test", Some("other"))
            .unwrap();
    }
    for query in ["", "project"] {
        let hits = store
            .search_scoped(query, None, 2, Some(Some("mina")), true)
            .unwrap()
            .hits;
        assert_eq!(hits.len(), 2);
        assert!(hits
            .iter()
            .all(|hit| hit.memory.id == own.id || hit.memory.id == shared.id));
    }
    store
        .transfer_owner(&own.id, Some("mina"), Some("chief"))
        .unwrap();
    drop(store);
    let store = open(dir.path(), Options::default());
    store
        .transfer_owner(&own.id, Some("mina"), Some("chief"))
        .unwrap();
    assert_eq!(store.get(&own.id).unwrap().owner.as_deref(), Some("chief"));
    assert!(store
        .transfer_owner(&own.id, Some("mina"), Some("other"))
        .is_err());
}

#[test]
fn team_memory_owner_column_migrates_existing_database() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path(), Options::default());
    let memory = store
        .create(NewMemory::note("old shared note"), "test")
        .unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
    conn.execute_batch("ALTER TABLE memories DROP COLUMN owner")
        .unwrap();
    drop(conn);
    let store = open(dir.path(), Options::default());
    assert_eq!(store.get(&memory.id).unwrap().owner, None);
    drop(store);
    assert_eq!(
        open(dir.path(), Options::default())
            .get(&memory.id)
            .unwrap()
            .owner,
        None
    );
}
