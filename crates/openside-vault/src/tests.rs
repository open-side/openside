use super::*;

fn ctx(who: &str) -> AuditContext<'_> {
    AuditContext {
        who,
        session_id: None,
        tool_call_id: None,
    }
}
use base64::{engine::general_purpose::STANDARD, Engine as _};

fn entry(site: &str, password: String, policy: AccessPolicy) -> Entry {
    Entry::new(
        site.into(),
        vec![AllowedOrigin::parse("https://example.com").unwrap()],
        Secret::new("username".into()),
        Secret::new(password),
        None,
        policy,
    )
    .unwrap()
}
fn passphrase() -> Secret {
    Secret::new("local-test-passphrase-only".into())
}

#[test]
fn canary_zero_hits_all_public_paths() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    let audit = dir.path().join("audit.jsonl");
    let mut random = [0u8; 16];
    OsRng.fill_bytes(&mut random);
    let canary = format!("CANARY-{}", data_encoding::HEXLOWER.encode(&random));
    let seed_text = data_encoding::BASE32_NOPAD.encode(canary.as_bytes());
    let password = Secret::new(canary.clone());
    let seed = Secret::new(seed_text.clone());
    let decoded = parse_base32_seed(&seed).unwrap();
    let code = decoded
        .code(TotpParams::new(8, 30, TotpAlgorithm::Sha256).unwrap(), 59)
        .unwrap();
    let redactor = Redactor::new([&password, &seed]);
    let mut observations = vec![format!(
        "{password:?} {password} {seed:?} {decoded:?} {code:?} {redactor:?}"
    )];
    let encoded = STANDARD.encode(&canary);
    observations.push(redactor.redact(&format!("{canary} {encoded} {seed_text}")));
    let item = Entry::new(
        "test-site".into(),
        vec![AllowedOrigin::parse("https://example.com").unwrap()],
        Secret::new(canary.clone()),
        password,
        Some(Totp::parse(seed).unwrap()),
        AccessPolicy::WhileUnlocked,
    )
    .unwrap();
    observations.push(format!(
        "{item:?} {:?}",
        AllowedOrigin::parse("https://example.com").unwrap()
    ));
    let mut vault = Vault::create(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    assert!(vault.is_unlocked());
    vault.put(item).unwrap();
    observations.push(format!("{vault:?}"));
    observations.push(format!("{:?}", vault.platform_mut()));
    observations.push(vault.redactor().unwrap().redact(&canary));
    let before = std::fs::read(&path).unwrap();
    let mut saw_fill = false;
    let outcome = vault
        .vault_login(
            &ctx("host:test"),
            AuditAction::Fill,
            "test-site",
            "https://example.com",
            |secrets| {
                assert_eq!(secrets.password, canary);
                assert_eq!(secrets.username, canary);
                assert_eq!(secrets.totp.map(Totp::params), Some(TotpParams::default()));
                observations.push(format!("{secrets:?}"));
                saw_fill = true;
                Ok(())
            },
        )
        .unwrap();
    assert!(saw_fill);
    assert_eq!(outcome, LoginOutcome::Filled);
    observations.push(format!("{outcome:?}"));
    for (who, site, origin) in [
        (canary.as_str(), "test-site", "https://example.com"),
        ("host:test", canary.as_str(), "https://example.com"),
        ("host:test", "test-site", canary.as_str()),
        (
            "host:test",
            "test-site",
            &format!("https://example.com/?password={canary}"),
        ),
    ] {
        observations.push(format!(
            "{:?}",
            vault
                .vault_login(&ctx(who), AuditAction::Fill, site, origin, |_| Ok(()))
                .unwrap()
        ));
    }
    vault.lock();
    assert!(!vault.is_unlocked());
    assert_eq!(
        vault
            .vault_login(
                &ctx(&canary),
                AuditAction::Fill,
                &canary,
                &canary,
                |_| panic!("locked fill")
            )
            .unwrap(),
        LoginOutcome::Denied(Denial::Locked)
    );
    observations.push(format!("{:?}", vault.redactor().unwrap_err()));
    observations.push(format!(
        "{:?}",
        vault
            .put(entry("locked", canary.clone(), AccessPolicy::Always))
            .unwrap_err()
    ));
    assert!(vault.unlock(&Secret::new(canary.clone())).is_err());
    assert!(!vault.is_unlocked());
    vault.unlock(&passphrase()).unwrap();
    drop(vault);
    let mut reopened =
        Vault::open(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    assert_eq!(
        reopened
            .vault_login(
                &ctx("host:test"),
                AuditAction::Fill,
                "test-site",
                "https://example.com",
                |secrets| {
                    assert_eq!(secrets.password, canary);
                    Ok(())
                }
            )
            .unwrap(),
        LoginOutcome::Filled
    );
    observations.push(format!("{reopened:?}"));
    observations.push(format!(
        "{:?}",
        Vault::create(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap_err()
    ));
    observations.push(format!(
        "{:?}",
        Entry::new(
            canary.clone(),
            vec![],
            Secret::new(canary.clone()),
            Secret::new(canary.clone()),
            None,
            AccessPolicy::Never
        )
        .unwrap_err()
    ));
    observations.push(format!(
        "{:?}",
        parse_base32_seed(&Secret::new(canary.clone())).unwrap_err()
    ));
    observations.push(format!("{:?}", AllowedOrigin::parse(&canary).unwrap_err()));
    for error in [
        Error::Io,
        Error::Audit,
        Error::Format,
        Error::Authentication,
        Error::Crypto,
        Error::Locked,
        Error::InvalidOrigin,
        Error::InvalidSeed,
        Error::InvalidEntry,
        Error::AlreadyExists,
    ] {
        observations.push(format!("{error:?} {error}"));
    }
    for policy in [
        AccessPolicy::Always,
        AccessPolicy::WhileUnlocked,
        AccessPolicy::Never,
    ] {
        observations.push(format!("{policy:?}"));
    }
    for algorithm in [
        TotpAlgorithm::Sha1,
        TotpAlgorithm::Sha256,
        TotpAlgorithm::Sha512,
    ] {
        observations.push(format!("{algorithm:?}"));
    }
    let audit_bytes = std::fs::read(&audit).unwrap();
    for line in std::str::from_utf8(&audit_bytes).unwrap().lines() {
        let json: serde_json::Value = serde_json::from_str(line).unwrap();
        for field in ["when", "who", "what", "site_id", "origin", "decision"] {
            assert!(json.get(field).is_some());
        }
    }
    let outputs = [
        observations.join("\n").into_bytes(),
        before,
        std::fs::read(&path).unwrap(),
        audit_bytes,
    ];
    let hits = outputs
        .iter()
        .flat_map(|output| {
            [canary.as_bytes(), seed_text.as_bytes(), encoded.as_bytes()].map(move |needle| {
                output
                    .windows(needle.len())
                    .filter(|window| *window == needle)
                    .count()
            })
        })
        .sum::<usize>();
    assert_eq!(hits, 0, "canary output hits must be zero");
}

#[test]
fn policies_and_origin_denials_never_call_fill() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::create(
        dir.path().join("vault"),
        dir.path().join("audit"),
        &passphrase(),
        PassphraseLock::default(),
    )
    .unwrap();
    for (site, policy) in [
        ("always", AccessPolicy::Always),
        ("unlocked", AccessPolicy::WhileUnlocked),
        ("never", AccessPolicy::Never),
    ] {
        vault.put(entry(site, "password".into(), policy)).unwrap();
    }
    assert_eq!(
        vault
            .vault_login(
                &ctx("host"),
                AuditAction::Fill,
                "unlocked",
                "https://example.com",
                |_| Ok(())
            )
            .unwrap(),
        LoginOutcome::Filled
    );
    vault.platform_mut().lock();
    for (site, origin, denial) in [
        ("unlocked", "https://example.com", Denial::PlatformLocked),
        ("never", "https://example.com", Denial::PolicyNever),
        ("unknown", "https://example.com", Denial::UnknownSite),
        ("always", "http://example.com", Denial::OriginMismatch),
    ] {
        assert_eq!(
            vault
                .vault_login(&ctx("host"), AuditAction::Fill, site, origin, |_| panic!(
                    "denied fill"
                ))
                .unwrap(),
            LoginOutcome::Denied(denial)
        );
    }
    assert_eq!(
        vault
            .vault_login(
                &ctx("host"),
                AuditAction::Fill,
                "always",
                "https://example.com",
                |_| Err(())
            )
            .unwrap(),
        LoginOutcome::FillFailed
    );
    assert_eq!(
        vault
            .vault_login(
                &ctx("host"),
                AuditAction::Fill,
                "always",
                "https://example.com",
                |_| Ok(())
            )
            .unwrap(),
        LoginOutcome::Filled
    );
    vault.lock();
    assert_eq!(
        vault
            .vault_login(
                &ctx("host"),
                AuditAction::Fill,
                "always",
                "https://example.com",
                |_| panic!("locked fill")
            )
            .unwrap(),
        LoginOutcome::Denied(Denial::Locked)
    );
    let text = std::fs::read_to_string(dir.path().join("audit")).unwrap();
    assert_eq!(text.lines().count(), 11);
    assert!(!text.contains("password"));
}

#[test]
fn authenticated_encryption_wrong_password_tamper_and_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    let audit = dir.path().join("audit");
    let mut vault = Vault::create(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    vault
        .put(entry("test", "private-value".into(), AccessPolicy::Always))
        .unwrap();
    let first = std::fs::read(&path).unwrap();
    vault
        .put(entry("test", "private-value".into(), AccessPolicy::Always))
        .unwrap();
    let original = std::fs::read(&path).unwrap();
    assert_ne!(&first[28..52], &original[28..52]);
    assert!(!original
        .windows(b"private-value".len())
        .any(|window| window == b"private-value"));
    assert_eq!(
        Vault::open(
            &path,
            &audit,
            &Secret::new("wrong".into()),
            PassphraseLock::default()
        )
        .unwrap_err(),
        Error::Authentication
    );
    for offset in [12, 28, HEADER, original.len() - 1] {
        let mut tampered = original.clone();
        tampered[offset] ^= 1;
        std::fs::write(&path, tampered).unwrap();
        assert_eq!(
            vault.unlock(&passphrase()).unwrap_err(),
            Error::Authentication
        );
        assert!(!vault.is_unlocked());
    }
    let mut version = original.clone();
    version[11] = 3;
    std::fs::write(&path, version).unwrap();
    assert_eq!(vault.unlock(&passphrase()).unwrap_err(), Error::Format);
    let mut keyring_version = original.clone();
    keyring_version[11] = 2;
    std::fs::write(&path, keyring_version).unwrap();
    // A passphrase cannot open a keyring-wrapped file, and vice versa.
    assert_eq!(
        vault.unlock(&passphrase()).unwrap_err(),
        Error::Authentication
    );
    std::fs::write(&path, &original).unwrap();
    assert_eq!(
        vault.unlock_with_platform_key().unwrap_err(),
        Error::Authentication
    );
    std::fs::write(&path, b"short").unwrap();
    assert_eq!(vault.unlock(&passphrase()).unwrap_err(), Error::Format);
}

#[test]
fn audit_failure_prevents_fill() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::create(
        dir.path().join("vault"),
        dir.path().join("missing/audit"),
        &passphrase(),
        PassphraseLock::default(),
    )
    .unwrap();
    vault
        .put(entry("test", "secret".into(), AccessPolicy::Always))
        .unwrap();
    assert_eq!(
        vault
            .vault_login(
                &ctx("host"),
                AuditAction::Fill,
                "test",
                "https://example.com",
                |_| panic!("unaudited fill")
            )
            .unwrap_err(),
        Error::Audit
    );
}

#[test]
fn failed_save_preserves_previous_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    let mut vault = Vault::create(
        &path,
        dir.path().join("audit"),
        &passphrase(),
        PassphraseLock::default(),
    )
    .unwrap();
    vault
        .put(entry("test", "old".into(), AccessPolicy::Always))
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        vault
            .put(entry("test", "new".into(), AccessPolicy::Always))
            .unwrap_err(),
        Error::Io
    );
    vault
        .vault_login(
            &ctx("host"),
            AuditAction::Fill,
            "test",
            "https://example.com",
            |secrets| {
                assert_eq!(secrets.password, "old");
                Ok(())
            },
        )
        .unwrap();
}

#[cfg(unix)]
#[test]
fn creates_private_files() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    let audit = dir.path().join("audit");
    let mut vault = Vault::create(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    vault
        .vault_login(
            &ctx("host"),
            AuditAction::Fill,
            "unknown",
            "https://example.com",
            |_| panic!("unknown fill"),
        )
        .unwrap();
    for file in [path, audit] {
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

// Callbacks in these tests never await I/O, so one poll completes them.
fn ready<T>(future: impl Future<Output = T>) -> T {
    struct Noop;
    impl std::task::Wake for Noop {
        fn wake(self: std::sync::Arc<Self>) {}
    }
    let waker = std::task::Waker::from(std::sync::Arc::new(Noop));
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut std::task::Context::from_waker(&waker))
    {
        std::task::Poll::Ready(value) => value,
        std::task::Poll::Pending => panic!("test future was not immediately ready"),
    }
}

fn session_ctx<'a>(session: &'a str, call: &'a str) -> AuditContext<'a> {
    AuditContext {
        who: "opensided",
        session_id: Some(session),
        tool_call_id: Some(call),
    }
}

#[test]
fn per_item_policy_override_global_default_and_audit_query() {
    let dir = tempfile::tempdir().unwrap();
    let audit = dir.path().join("audit");
    let mut vault = Vault::create(
        dir.path().join("vault"),
        &audit,
        &passphrase(),
        PassphraseLock::default(),
    )
    .unwrap();
    const VALUE: &str = "policy-canary-value-31";
    vault
        .put(entry("item", VALUE.into(), AccessPolicy::Always))
        .unwrap();
    let fill = |vault: &mut Vault, call: &str| {
        vault
            .vault_login(
                &session_ctx("session-a", call),
                AuditAction::Fill,
                "item",
                "https://example.com",
                |_| Ok(()),
            )
            .unwrap()
    };
    assert_eq!(fill(&mut vault, "call-1"), LoginOutcome::Filled);
    // Override Never wins over a permissive default.
    vault
        .set_policy(&ctx("api"), "item", Some(AccessPolicy::Never))
        .unwrap();
    assert_eq!(
        fill(&mut vault, "call-2"),
        LoginOutcome::Denied(Denial::PolicyNever)
    );
    // Clearing the override inherits the global default.
    let info = vault.set_policy(&ctx("api"), "item", None).unwrap();
    assert_eq!(info.policy, None);
    assert_eq!(info.effective_policy, AccessPolicy::WhileUnlocked);
    vault.set_default_policy(AccessPolicy::Never);
    assert_eq!(
        fill(&mut vault, "call-3"),
        LoginOutcome::Denied(Denial::PolicyNever)
    );
    vault.set_default_policy(AccessPolicy::WhileUnlocked);
    vault.platform_mut().lock();
    assert_eq!(
        fill(&mut vault, "call-4"),
        LoginOutcome::Denied(Denial::PlatformLocked)
    );
    assert_eq!(
        vault.set_policy(&ctx("api"), "missing", None).unwrap_err(),
        Error::UnknownItem
    );
    let items = vault.items(&ctx("api")).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].site, "item");
    assert_eq!(items[0].origins, ["https://example.com:443"]);
    assert!(!format!("{items:?}").contains(VALUE));
    // Overrides persist in the encrypted file.
    vault.unlock(&passphrase()).unwrap();
    vault
        .set_policy(&ctx("api"), "item", Some(AccessPolicy::Never))
        .unwrap();
    drop(vault);
    let reopened = Vault::open(
        dir.path().join("vault"),
        &audit,
        &passphrase(),
        PassphraseLock::default(),
    )
    .unwrap();
    assert_eq!(
        reopened.items(&ctx("api")).unwrap()[0].policy,
        Some(AccessPolicy::Never)
    );

    let all = read_audit(&audit, &AuditQuery::default()).unwrap();
    assert!(!std::fs::read_to_string(&audit).unwrap().contains(VALUE));
    let session = read_audit(
        &audit,
        &AuditQuery {
            session_id: Some("session-a".into()),
            ..AuditQuery::default()
        },
    )
    .unwrap();
    let decisions: Vec<_> = session
        .iter()
        .map(|record| {
            (
                record.tool_call_id.as_deref().unwrap(),
                record.decision.as_str(),
                record.detail.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        decisions,
        [
            ("call-1", "allowed", None),
            ("call-1", "filled", None),
            ("call-2", "denied", Some("policy_never")),
            ("call-3", "denied", Some("policy_never")),
            ("call-4", "denied", Some("platform_locked")),
        ]
    );
    for record in &session {
        assert_eq!(record.what, "fill");
        assert_eq!(record.site_id, "item");
        assert_eq!(record.origin, "https://example.com:443");
        assert!(record.when > 1_600_000_000);
    }
    let updates: Vec<_> = all
        .iter()
        .filter(|record| record.what == "policy_update" && record.decision == "updated")
        .map(|record| record.detail.as_deref().unwrap())
        .collect();
    assert_eq!(updates, ["never", "default", "never"]);
    assert!(all.iter().any(|record| record.what == "get"));
    let limited = read_audit(
        &audit,
        &AuditQuery {
            site_id: Some("item".into()),
            limit: Some(2),
            ..AuditQuery::default()
        },
    )
    .unwrap();
    assert_eq!(limited.len(), 2);
    assert_eq!(limited[1].what, "policy_update");
    assert!(read_audit(
        &audit,
        &AuditQuery {
            since: Some(u64::MAX),
            ..AuditQuery::default()
        }
    )
    .unwrap()
    .is_empty());
}

#[test]
fn unwritable_audit_blocks_policy_change_and_rejections_are_audited() {
    let dir = tempfile::tempdir().unwrap();
    let audit = dir.path().join("audit");
    let mut vault = Vault::create(
        dir.path().join("vault"),
        &audit,
        &passphrase(),
        PassphraseLock::default(),
    )
    .unwrap();
    vault
        .put(entry(
            "item",
            "audit-gate-value".into(),
            AccessPolicy::Never,
        ))
        .unwrap();
    // Replace the audit file by a directory: appending to it fails.
    let _ = std::fs::remove_file(&audit);
    std::fs::create_dir(&audit).unwrap();
    assert_eq!(
        vault
            .set_policy(&ctx("api"), "item", Some(AccessPolicy::Always))
            .unwrap_err(),
        Error::Audit
    );
    std::fs::remove_dir(&audit).unwrap();
    assert_eq!(
        vault.items(&ctx("api")).unwrap()[0].policy,
        Some(AccessPolicy::Never),
        "a policy must not change when its decision cannot be audited"
    );

    assert_eq!(
        vault.set_policy(&ctx("api"), "missing", None).unwrap_err(),
        Error::UnknownItem
    );
    vault.lock();
    assert_eq!(vault.items(&ctx("api")).unwrap_err(), Error::Locked);
    assert_eq!(
        vault.set_policy(&ctx("api"), "item", None).unwrap_err(),
        Error::Locked
    );
    let records = read_audit(&audit, &AuditQuery::default()).unwrap();
    let rejected: Vec<_> = records
        .iter()
        .filter(|record| record.decision == "denied")
        .map(|record| (record.what.as_str(), record.detail.as_deref().unwrap()))
        .collect();
    assert_eq!(
        rejected,
        [
            ("policy_update", "unknown_item"),
            ("get", "locked"),
            ("policy_update", "locked"),
        ]
    );
}

#[test]
fn passkeys_are_encrypted_origin_bound_and_released_only_to_callbacks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    let audit = dir.path().join("audit");
    const KEY: &str = "UEFTU0tFWS1DQU5BUlktcHJpdmF0ZS1rZXktYnl0ZXMtOTE3";
    let mut vault = Vault::create(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    vault
        .put(
            Entry::new(
                "rp".into(),
                vec![AllowedOrigin::parse("http://localhost:4100").unwrap()],
                Secret::new("user".into()),
                Secret::new("pw".into()),
                None,
                AccessPolicy::Always,
            )
            .unwrap(),
        )
        .unwrap();
    let credential = |rp: &str| {
        PasskeyCredential::new(
            "Y3JlZC1pZA".into(),
            rp.into(),
            Some("dXNlcg".into()),
            Secret::new(KEY.into()),
            0,
        )
        .unwrap()
    };
    let ctx = session_ctx("session-p", "call-p");
    // No passkey yet.
    assert_eq!(
        ready(
            vault.passkey_login(&ctx, "rp", "http://localhost:4100", |_| {
                panic!("no passkey to release")
            })
        )
        .unwrap(),
        LoginOutcome::Denied(Denial::NoPasskey)
    );
    // A credential for a foreign RP is refused and not stored.
    assert_eq!(
        ready(
            vault.passkey_register(&ctx, "rp", "http://localhost:4100", || {
                Box::pin(async { Ok(vec![credential("evil.test")]) })
            })
        )
        .unwrap(),
        LoginOutcome::Denied(Denial::RpMismatch)
    );
    // Registration from a lookalike origin never runs the capture.
    assert_eq!(
        ready(
            vault.passkey_register(&ctx, "rp", "http://localhost.evil.test:4100", || {
                panic!("capture on wrong origin")
            })
        )
        .unwrap(),
        LoginOutcome::Denied(Denial::OriginMismatch)
    );
    assert_eq!(
        ready(
            vault.passkey_register(&ctx, "rp", "http://localhost:4100", || {
                Box::pin(async { Ok(vec![credential("localhost")]) })
            })
        )
        .unwrap(),
        LoginOutcome::Filled
    );
    assert!(vault.redactor().unwrap().redact(KEY) == "[REDACTED]");
    assert!(vault.redactor_for("rp").unwrap().redact(KEY) == "[REDACTED]");
    drop(vault);
    let mut vault = Vault::open(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    assert_eq!(vault.items(&ctx).unwrap()[0].passkey_rp_ids, ["localhost"]);
    assert_eq!(
        ready(
            vault.passkey_login(&ctx, "rp", "http://localhost:4199", |_| {
                panic!("released to another port")
            })
        )
        .unwrap(),
        LoginOutcome::Denied(Denial::OriginMismatch)
    );
    let mut released = None;
    assert_eq!(
        ready(
            vault.passkey_login(&ctx, "rp", "http://localhost:4100", |keys| {
                assert_eq!(keys.len(), 1);
                assert_eq!(keys[0].rp_id, "localhost");
                released = Some((
                    keys[0].credential_id.clone(),
                    keys[0].private_key.to_string(),
                ));
                Box::pin(async {
                    Ok(vec![SignCount {
                        credential_id: "Y3JlZC1pZA".into(),
                        sign_count: 7,
                    }])
                })
            })
        )
        .unwrap(),
        LoginOutcome::Filled
    );
    assert_eq!(released, Some(("Y3JlZC1pZA".to_owned(), KEY.to_owned())));
    drop(vault);
    let mut vault = Vault::open(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    ready(
        vault.passkey_login(&ctx, "rp", "http://localhost:4100", |keys| {
            assert_eq!(keys[0].sign_count, 7);
            Box::pin(async { Ok(Vec::new()) })
        }),
    )
    .unwrap();
    for file in [&path, &audit] {
        let bytes = std::fs::read(file).unwrap();
        assert!(!bytes
            .windows(KEY.len())
            .any(|window| window == KEY.as_bytes()));
    }
    let records = read_audit(&audit, &AuditQuery::default()).unwrap();
    let kinds: Vec<_> = records
        .iter()
        .filter(|record| record.what.starts_with("passkey"))
        .map(|record| (record.what.as_str(), record.decision.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("passkey_login", "denied"),
            ("passkey_register", "allowed"),
            ("passkey_register", "denied"),
            ("passkey_register", "denied"),
            ("passkey_register", "allowed"),
            ("passkey_register", "completed"),
            ("passkey_login", "denied"),
            ("passkey_login", "allowed"),
            ("passkey_login", "completed"),
            ("passkey_login", "allowed"),
            ("passkey_login", "completed"),
        ]
    );
    assert!(records
        .iter()
        .filter(|record| record.what.starts_with("passkey"))
        .all(|record| record.session_id.as_deref() == Some("session-p")
            && record.tool_call_id.as_deref() == Some("call-p")));
}

/// In-memory stand-in for an OS secret store, exposing the stored key to the test only.
#[derive(Clone, Default)]
struct MemoryKeyring {
    key: std::sync::Arc<parking_lot::Mutex<Option<[u8; 32]>>>,
    unlocked: bool,
}
impl PlatformLock for MemoryKeyring {
    fn unlock(&mut self) -> Result<(), Error> {
        self.unlocked = true;
        Ok(())
    }
    fn lock(&mut self) {
        self.unlocked = false;
    }
    fn is_unlocked(&self) -> bool {
        self.unlocked
    }
    fn wrapping_key(&mut self, create: bool) -> Result<Option<WrappingKey>, Error> {
        let mut slot = self.key.lock();
        if slot.is_none() && create {
            let key = WrappingKey::generate()?;
            *slot = Some(*key.bytes());
        }
        slot.as_ref()
            .map(|key| WrappingKey::from_bytes(key))
            .transpose()?
            .ok_or(Error::Unavailable)
            .map(Some)
    }
}

/// Every byte encoding a test can reasonably check for: raw, hex (both cases), base64.
pub(crate) fn key_encodings(key: &[u8]) -> Vec<Vec<u8>> {
    vec![
        key.to_vec(),
        data_encoding::HEXLOWER.encode(key).into_bytes(),
        data_encoding::HEXUPPER.encode(key).into_bytes(),
        STANDARD.encode(key).into_bytes(),
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(key)
            .into_bytes(),
    ]
}

#[test]
fn platform_key_vault_never_writes_wrapping_or_file_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    let audit = dir.path().join("audit");
    let keyring = MemoryKeyring::default();
    let mut vault = Vault::create_with_platform_key(&path, &audit, keyring.clone()).unwrap();
    vault
        .put(entry(
            "item",
            "keyring-canary-pw".into(),
            AccessPolicy::Always,
        ))
        .unwrap();
    vault
        .vault_login(
            &ctx("host"),
            AuditAction::Fill,
            "item",
            "https://example.com",
            |_| Ok(()),
        )
        .unwrap();
    drop(vault);
    let wrapping = keyring.key.lock().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[8..12], &2u32.to_be_bytes());
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&bytes[12..28]);
    let file_key = hkdf_key(&WrappingKey::from_bytes(&wrapping).unwrap(), &salt).unwrap();
    let mut needles = key_encodings(&wrapping);
    needles.extend(key_encodings(file_key.as_ref()));
    needles.push(b"keyring-canary-pw".to_vec());
    for file in std::fs::read_dir(dir.path()).unwrap() {
        let content = std::fs::read(file.unwrap().path()).unwrap();
        for needle in &needles {
            assert!(!content.windows(needle.len()).any(|window| window == needle));
        }
    }
    // Reopen with the keyring alone; a passphrase cannot open a keyring vault.
    let mut reopened = Vault::open_with_platform_key(&path, &audit, keyring.clone()).unwrap();
    assert_eq!(
        reopened
            .vault_login(
                &ctx("host"),
                AuditAction::Fill,
                "item",
                "https://example.com",
                |secrets| {
                    assert_eq!(secrets.password, "keyring-canary-pw");
                    Ok(())
                }
            )
            .unwrap(),
        LoginOutcome::Filled
    );
    assert_eq!(
        Vault::open(&path, &audit, &passphrase(), MemoryKeyring::default()).unwrap_err(),
        Error::Authentication
    );
    // A different (or missing) keyring key fails closed.
    assert_eq!(
        Vault::open_with_platform_key(&path, &audit, MemoryKeyring::default()).unwrap_err(),
        Error::Unavailable
    );
    let other = MemoryKeyring::default();
    *other.key.lock() = Some([7u8; 32]);
    assert_eq!(
        Vault::open_with_platform_key(&path, &audit, other).unwrap_err(),
        Error::Authentication
    );
    // Passphrase-only backends cannot create keyring vaults.
    assert_eq!(
        Vault::create_with_platform_key(
            dir.path().join("other"),
            &audit,
            PassphraseLock::default()
        )
        .unwrap_err(),
        Error::Unavailable
    );
}

fn fill_code(secrets: &FillSecrets<'_>, time: u64) -> String {
    let mut seen = String::new();
    ready(secrets.fill_totp(time, |code| {
        seen = code.to_owned();
        Box::pin(async { Ok(()) })
    }))
    .unwrap();
    seen
}

#[test]
fn rfc6238_codes_via_fill_totp_callback() {
    let seed = || Secret::new(data_encoding::BASE32_NOPAD.encode(b"12345678901234567890"));
    let six = Totp::parse(seed()).unwrap();
    let eight = Totp::new(seed(), TotpParams::new(8, 30, TotpAlgorithm::Sha1).unwrap()).unwrap();
    for (time, expected) in [
        (59, "94287082"),
        (1_111_111_109, "07081804"),
        (2_000_000_000, "69279037"),
    ] {
        let secrets = |totp| FillSecrets {
            username: "",
            password: "",
            totp: Some(totp),
        };
        assert_eq!(fill_code(&secrets(&eight), time), expected);
        assert_eq!(fill_code(&secrets(&six), time), &expected[2..]);
    }
}

/// Items written before TOTP parameters existed carry only `totp_seed`; they read back as
/// 6 digits / 30 s / SHA-1. Parameters from an `otpauth://` URI survive a reopen and a sync
/// export, and default parameters are not written at all.
#[test]
fn totp_parameters_persist_and_items_without_them_default_to_six_digits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    let audit = dir.path().join("audit.jsonl");
    let sha256_seed = data_encoding::BASE32_NOPAD.encode(b"12345678901234567890123456789012");
    let sha1_seed = data_encoding::BASE32_NOPAD.encode(b"12345678901234567890");
    let item = |site: &str, totp: Totp| {
        Entry::new(
            site.into(),
            vec![AllowedOrigin::parse("https://example.com").unwrap()],
            Secret::new("username".into()),
            Secret::new("password-value".into()),
            Some(totp),
            AccessPolicy::Always,
        )
        .unwrap()
    };
    let mut vault = Vault::create(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    vault
        .put(item(
            "uri",
            Totp::parse(Secret::new(format!(
                "otpauth://totp/Bank:alice?secret={sha256_seed}&algorithm=SHA256&digits=8&period=60&issuer=Bank"
            )))
            .unwrap(),
        ))
        .unwrap();
    vault
        .put(item("legacy", Totp::parse(Secret::new(sha1_seed)).unwrap()))
        .unwrap();
    let legacy = vault.export_item(&ctx("host:test"), "legacy").unwrap();
    let legacy: serde_json::Value = serde_json::from_slice(&legacy).unwrap();
    assert!(legacy.get("totp_params").is_none());
    let uri = vault.export_item(&ctx("host:test"), "uri").unwrap();
    let uri: serde_json::Value = serde_json::from_slice(&uri).unwrap();
    assert_eq!(
        uri["totp_params"],
        serde_json::json!({"digits":8,"period":60,"algorithm":"SHA256"})
    );
    drop(vault);
    let mut vault = Vault::open(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    let mut codes = Vec::new();
    for site in ["uri", "legacy"] {
        let outcome = vault
            .vault_login(
                &ctx("host:test"),
                AuditAction::Otp,
                site,
                "https://example.com",
                |secrets| {
                    // T=119 with a 60 s step is counter 1, i.e. RFC 6238's T=59 SHA-256 row.
                    codes.push(fill_code(&secrets, 119));
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(outcome, LoginOutcome::Filled);
    }
    // SHA-1 at T=119 (counter 3), default 6 digits.
    let sha1 = Totp::parse(Secret::new(
        data_encoding::BASE32_NOPAD.encode(b"12345678901234567890"),
    ))
    .unwrap()
    .code(119)
    .unwrap();
    assert_eq!(codes[0], "46119246");
    assert_eq!(codes[1].len(), 6);
    assert_eq!(codes[1], sha1.0.as_str());
    let bad = br#"{"site":"x","origins":["https://example.com"],"username":"u","password":"p","totp_seed":null,"totp_params":{"digits":6,"period":30,"algorithm":"SHA1"}}"#;
    assert!(PreparedImport::parse("x", bad).is_err());
    let bad = br#"{"site":"x","origins":["https://example.com"],"username":"u","password":"p","totp_seed":"GEZDGNBV","totp_params":{"digits":9,"period":30,"algorithm":"SHA1"}}"#;
    assert!(PreparedImport::parse("x", bad).is_err());
}

/// A sign-in typed into a login ticket becomes an item bound to exactly the page's origin;
/// the same username at that exact origin only gets its new password, an item with wider
/// origins is never joined, and `Never` refuses. Listing shows masked names only.
#[test]
fn saved_logins_bind_exact_origins_update_only_their_password_and_list_masked_names() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    let audit = dir.path().join("audit.jsonl");
    let mut vault = Vault::create(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    let mut wild = entry("wild", "wild-password-value".into(), AccessPolicy::Always);
    wild.origins = vec![AllowedOrigin::parse("https://*.example.org").unwrap()];
    vault.put(wild).unwrap();
    let save = |vault: &mut Vault, user: &str, password: &str, operation: &str| {
        vault.save_login(
            &ctx("device:phone"),
            "https://login.example.org",
            Secret::new(user.into()),
            Secret::new(password.into()),
            operation,
        )
    };
    let first = save(&mut vault, "jane@mail.test", "first-secret", "op-a")
        .unwrap()
        .unwrap();
    assert_eq!(
        first,
        SavedLogin {
            site: "login-login-example-org".into(),
            created: true
        }
    );
    assert!(vault.registered_by("op-a").unwrap());
    let updated = save(&mut vault, "jane@mail.test", "second-secret", "op-b")
        .unwrap()
        .unwrap();
    assert_eq!(updated.site, first.site);
    assert!(!updated.created);
    assert!(vault.registered_by("op-b").unwrap());
    let other = save(&mut vault, "bob", "bob-secret", "op-c")
        .unwrap()
        .unwrap();
    assert!(other.created);
    assert_ne!(other.site, first.site);

    let mut accounts = vault.logins_for("https://login.example.org").unwrap();
    accounts.sort_by(|a, b| a.site.cmp(&b.site));
    let hints: Vec<&str> = accounts.iter().map(|a| a.username_hint.as_str()).collect();
    assert_eq!(hints, ["j***@mail.test", "b***", "u***"]);
    let mut released = String::new();
    vault
        .vault_login(
            &ctx("host:test"),
            AuditAction::Fill,
            &first.site,
            "https://login.example.org",
            |secrets| {
                released = format!("{}:{}", secrets.username, secrets.password);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(released, "jane@mail.test:second-secret");
    let items = vault.items(&ctx("host:test")).unwrap();
    let saved = items.iter().find(|item| item.site == first.site).unwrap();
    assert_eq!(saved.origins, ["https://login.example.org:443"]);
    assert_eq!(saved.policy, None);
    let wild = items.iter().find(|item| item.site == "wild").unwrap();
    assert_eq!(wild.origins, ["https://*.example.org:443"]);

    vault
        .set_policy(&ctx("host:test"), "wild", Some(AccessPolicy::Never))
        .unwrap();
    assert_eq!(
        vault
            .login_save_refusal("https://login.example.org")
            .unwrap(),
        Some(Denial::PolicyNever)
    );
    assert_eq!(
        save(&mut vault, "eve", "eve-secret", "op-d").unwrap(),
        Err(Denial::PolicyNever)
    );
    assert!(!vault.registered_by("op-d").unwrap());
    let log = std::fs::read_to_string(&audit).unwrap();
    for value in [
        "first-secret",
        "second-secret",
        "jane@mail.test",
        "bob-secret",
    ] {
        assert!(!log.contains(value), "{value} reached the audit log");
    }
    assert!(log.contains(r#""what":"login_save""#));
    assert!(log.contains(r#""decision":"updated""#));
}

/// A registered authenticator always becomes a new TOTP-only item bound to exactly the
/// ticket's origin: it never joins a login item (which may carry wildcard or extra origins),
/// never replaces a saved factor (a second or stale registration is refused), and several
/// accounts holding a TOTP for one origin are reported as ambiguous instead of picking one.
#[test]
fn registered_authenticators_create_exact_origin_items_and_never_replace_or_guess() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    let audit = dir.path().join("audit.jsonl");
    let seed = data_encoding::BASE32_NOPAD.encode(b"12345678901234567890");
    let totp = || Totp::parse(Secret::new(seed.clone())).unwrap();
    let mut vault = Vault::create(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    vault
        .put(entry(
            "bank",
            "bank-password-value".into(),
            AccessPolicy::Always,
        ))
        .unwrap();
    let mut wild = entry("wild", "wild-password-value".into(), AccessPolicy::Always);
    wild.origins = vec![AllowedOrigin::parse("https://*.example.org").unwrap()];
    vault.put(wild).unwrap();
    assert_eq!(
        vault.totp_site_for("https://example.com").unwrap(),
        TotpMatch::None
    );

    let site = vault
        .register_totp(&ctx("host:test"), "https://example.com", totp(), "op-first")
        .unwrap()
        .unwrap();
    assert_eq!(site, "otp-example-com");
    assert_eq!(
        vault.totp_site_for("https://example.com").unwrap(),
        TotpMatch::One(site.clone())
    );
    assert!(vault.registered_by("op-first").unwrap());
    // Another operation is not this one, though an authenticator exists at the origin.
    assert!(!vault.registered_by("op-other").unwrap());
    // The marker is local: an export for sync does not carry it.
    let exported = vault.export_item(&ctx("host:test"), &site).unwrap();
    assert!(!String::from_utf8_lossy(&exported).contains("op-first"));
    // A synced replacement of the item (here its own export round-tripped) keeps the local
    // marker, and a marker smuggled into a payload is never taken.
    let prepared = PreparedImport::parse(&site, &exported).unwrap();
    vault.commit_import(&ctx("host:test"), prepared).unwrap();
    assert!(vault.registered_by("op-first").unwrap());
    let mut forged: serde_json::Value = serde_json::from_slice(&exported).unwrap();
    forged["registration"] = serde_json::json!("op-forged");
    let forged = serde_json::to_vec(&forged).unwrap();
    let prepared = PreparedImport::parse(&site, &forged).unwrap();
    vault.commit_import(&ctx("host:test"), prepared).unwrap();
    assert!(!vault.registered_by("op-forged").unwrap());
    assert!(vault.registered_by("op-first").unwrap());
    let items = vault.items(&ctx("host:test")).unwrap();
    let bank = items.iter().find(|item| item.site == "bank").unwrap();
    assert!(!bank.has_totp, "the login item is left alone");
    let created = items.iter().find(|item| item.site == site).unwrap();
    assert_eq!(created.origins, ["https://example.com:443"]);
    // A second (or stale) registration for the origin replaces nothing.
    assert_eq!(
        vault
            .register_totp(&ctx("host:test"), "https://example.com", totp(), "op-stale")
            .unwrap(),
        Err(Denial::AlreadyRegistered)
    );
    assert!(!vault.registered_by("op-stale").unwrap());
    assert_eq!(
        vault
            .register_totp(
                &ctx("host:test"),
                "https://c.example.net",
                totp(),
                "Not An Id"
            )
            .unwrap_err(),
        Error::InvalidEntry
    );

    // The wildcard item does not lend its scope to the new factor.
    let exact = vault
        .register_totp(
            &ctx("host:test"),
            "https://a.example.org",
            totp(),
            "op-exact",
        )
        .unwrap()
        .unwrap();
    assert_eq!(exact, "otp-a-example-org");
    assert_eq!(
        vault.totp_site_for("https://b.example.org").unwrap(),
        TotpMatch::None
    );

    // Two accounts on one origin: no item is picked.
    let mut second = entry(
        "bank-2",
        "second-password-value".into(),
        AccessPolicy::Always,
    );
    second.totp = Some(totp());
    vault.put(second).unwrap();
    assert_eq!(
        vault.totp_site_for("https://example.com").unwrap(),
        TotpMatch::Ambiguous
    );

    let local = vault
        .register_totp(
            &ctx("host:test"),
            "http://localhost:4100",
            totp(),
            "op-local",
        )
        .unwrap()
        .unwrap();
    assert_eq!(local, "otp-localhost-4100");
    assert_eq!(
        vault.totp_site_for("http://localhost:4101").unwrap(),
        TotpMatch::None
    );
    drop(vault);

    let mut vault = Vault::open(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    // The marker is saved with the item, in the same write.
    assert!(vault.registered_by("op-local").unwrap());
    let mut code = String::new();
    let outcome = vault
        .vault_login(
            &ctx("host:test"),
            AuditAction::Otp,
            &local,
            "http://localhost:4100",
            |secrets| {
                code = fill_code(&secrets, 59);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(outcome, LoginOutcome::Filled);
    assert_eq!(code, "287082");

    let mut never = entry("never", "never-password-value".into(), AccessPolicy::Never);
    never.origins = vec![AllowedOrigin::parse("https://never.test").unwrap()];
    vault.put(never).unwrap();
    assert_eq!(
        vault
            .register_totp(&ctx("host:test"), "https://never.test", totp(), "op-never")
            .unwrap(),
        Err(Denial::PolicyNever)
    );
    assert_eq!(
        vault.totp_site_for("https://never.test").unwrap(),
        TotpMatch::None
    );

    let records: Vec<_> = read_audit(&audit, &AuditQuery::default())
        .unwrap()
        .into_iter()
        .filter(|record| record.what == "otp_register")
        .map(|record| (record.site_id, record.decision, record.detail))
        .collect();
    assert_eq!(
        records,
        [
            // The item does not exist yet when the decision is recorded.
            ("unknown-site".into(), "allowed".into(), None),
            (site.clone(), "registered".into(), None),
            (
                site.clone(),
                "denied".into(),
                Some("already_registered".into())
            ),
            ("unknown-site".into(), "allowed".into(), None),
            (exact.clone(), "registered".into(), None),
            ("unknown-site".into(), "allowed".into(), None),
            (local.clone(), "registered".into(), None),
            ("never".into(), "denied".into(), Some("policy_never".into())),
        ]
    );
    assert!(!std::fs::read_to_string(&audit).unwrap().contains(&seed));
}

#[test]
fn people_add_and_delete_items_with_audited_decisions_and_no_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    let audit = dir.path().join("audit.jsonl");
    assert_eq!(stored_format(&path), Ok(None));
    const PASSWORD: &str = "added-item-password-canary-83";
    let mut vault = Vault::create(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    assert_eq!(stored_format(&path), Ok(Some(StoredFormat::Passphrase)));
    let login = || {
        Entry::with_policy_override(
            "mail".into(),
            vec![AllowedOrigin::parse("https://mail.example.com").unwrap()],
            Secret::new("person@example.com".into()),
            Secret::new(PASSWORD.into()),
            None,
            None,
        )
        .unwrap()
    };
    let info = vault.add_item(&ctx("api"), login()).unwrap();
    assert_eq!(info.site, "mail");
    assert_eq!(info.policy, None);
    assert_eq!(info.effective_policy, AccessPolicy::WhileUnlocked);
    assert_eq!(
        vault.add_item(&ctx("api"), login()).unwrap_err(),
        Error::ItemExists
    );
    drop(vault);
    let mut vault = Vault::open(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    assert_eq!(vault.item_ids().unwrap(), ["mail"]);
    assert_eq!(
        vault.delete_item(&ctx("api"), "absent").unwrap_err(),
        Error::UnknownItem
    );
    vault.delete_item(&ctx("api"), "mail").unwrap();
    drop(vault);
    let mut vault = Vault::open(&path, &audit, &passphrase(), PassphraseLock::default()).unwrap();
    assert!(vault.item_ids().unwrap().is_empty());
    vault.lock();
    assert_eq!(
        vault.add_item(&ctx("api"), login()).unwrap_err(),
        Error::Locked
    );

    let text = std::fs::read_to_string(&audit).unwrap();
    assert!(!text.contains(PASSWORD) && !text.contains("person@example.com"));
    let records: Vec<(String, String, String, Option<String>)> = text
        .lines()
        .map(|line| serde_json::from_str::<AuditRecord>(line).unwrap())
        .filter(|record| record.what.starts_with("item_"))
        .map(|record| (record.what, record.site_id, record.decision, record.detail))
        .collect();
    let row = |what: &str, site: &str, decision: &str, detail: Option<&str>| {
        (
            what.into(),
            site.into(),
            decision.into(),
            detail.map(Into::into),
        )
    };
    assert_eq!(
        records,
        [
            row("item_add", "unknown-site", "allowed", None),
            row("item_add", "mail", "added", None),
            row("item_add", "mail", "denied", Some("item_exists")),
            row(
                "item_delete",
                "unknown-site",
                "denied",
                Some("unknown_item")
            ),
            row("item_delete", "mail", "allowed", None),
            row("item_delete", "unknown-site", "deleted", None),
            row("item_add", "unknown-site", "denied", Some("locked")),
        ]
    );
}

#[test]
fn stored_format_reads_only_the_header() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    std::fs::write(&path, b"not a vault file").unwrap();
    assert_eq!(stored_format(&path), Err(Error::Format));
    let mut header = MAGIC.to_vec();
    header.extend_from_slice(&VERSION_KEYRING.to_be_bytes());
    std::fs::write(&path, &header).unwrap();
    assert_eq!(stored_format(&path), Ok(Some(StoredFormat::Keyring)));
    header[11] = 9;
    std::fs::write(&path, &header).unwrap();
    assert_eq!(stored_format(&path), Err(Error::Format));
}

/// Lifecycle records need no open vault, name no item or origin and carry only the caller,
/// the step, its outcome and a fixed reason code.
#[test]
fn lifecycle_records_are_value_free_and_attributed() {
    let dir = tempfile::tempdir().unwrap();
    let audit = dir.path().join("audit.jsonl");
    audit_lifecycle(
        &audit,
        &ctx("device:abc123"),
        AuditAction::Unlock,
        "denied",
        Some("wrong_passphrase"),
    )
    .unwrap();
    audit_lifecycle(&audit, &ctx("local"), AuditAction::Lock, "locked", None).unwrap();
    let records: Vec<_> = read_audit(&audit, &AuditQuery::default())
        .unwrap()
        .into_iter()
        .map(|record| {
            (
                record.who,
                record.what,
                record.site_id,
                record.origin,
                record.decision,
                record.detail,
            )
        })
        .collect();
    let row = |who: &str, what: &str, decision: &str, detail: Option<&str>| {
        (
            who.to_owned(),
            what.to_owned(),
            "*".to_owned(),
            "-".to_owned(),
            decision.to_owned(),
            detail.map(str::to_owned),
        )
    };
    assert_eq!(
        records,
        [
            row(
                "device:abc123",
                "unlock",
                "denied",
                Some("wrong_passphrase")
            ),
            row("local", "lock", "locked", None),
        ]
    );
}

/// A filter whose every pattern one installed filter already holds adds nothing.
#[test]
fn a_redactor_is_covered_only_by_one_filter_holding_all_of_its_patterns() {
    let a = Secret::new("first-secret-value".into());
    let b = Secret::new("second-secret-value".into());
    let both = Redactor::new([&a, &b]);
    assert!(Redactor::new([&a, &b]).covered_by(std::slice::from_ref(&both)));
    assert!(Redactor::new([&a]).covered_by(std::slice::from_ref(&both)));
    let split = [Redactor::new([&a]), Redactor::new([&b])];
    assert!(!Redactor::new([&a, &b]).covered_by(&split));
    assert!(!Redactor::new([&a]).covered_by(&[]));
}
