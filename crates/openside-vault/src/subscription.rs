use crate::{AccessPolicy, AuditAction, AuditContext, Entry, Error, PlatformLock, Secret, Vault};
use serde::{Deserialize, Serialize};

/// Local subscription metadata; upstream identities and OAuth material are excluded.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionInfo {
    /// CLI source identifier.
    pub source: String,
    /// Durable source-local id.
    pub account_id: String,
    /// Person-assigned display label.
    pub label: String,
    /// Whether the daemon owns the isolated credential location.
    pub managed: bool,
    /// Operator enablement.
    pub enabled: bool,
    /// Durable default-account tombstone; removal never touches external credentials.
    #[serde(default)]
    pub hidden: bool,
    /// Protected measured headroom percentage.
    pub reserve_percent: u8,
    /// Creation time in Unix milliseconds.
    pub created_at_ms: u64,
    /// Durable compare-and-swap revision.
    pub revision: u64,
    /// Last adopted external content digest, never token material.
    pub external_digest: Option<String>,
    /// Last adopted external modification time in nanoseconds.
    pub external_mtime_ns: Option<u128>,
    /// Whether the current revision has definitively failed authentication.
    pub auth_invalid: bool,
}

fn site(source: &str, id: &str) -> String {
    format!("subscription:{source}:{id}")
}

#[derive(Default, Deserialize)]
struct Tokens {
    #[serde(default, alias = "accessToken")]
    access_token: Option<crate::ReadSecret>,
    #[serde(default, alias = "refreshToken")]
    refresh_token: Option<crate::ReadSecret>,
    #[serde(default)]
    id_token: Option<crate::ReadSecret>,
}
#[derive(Deserialize)]
struct Envelope {
    #[serde(default, rename = "claudeAiOauth")]
    claude: Tokens,
    #[serde(default)]
    tokens: Tokens,
}
fn redaction_tokens(raw: &str) -> Vec<Secret> {
    let Ok(envelope) = serde_json::from_str::<Envelope>(raw) else {
        return Vec::new();
    };
    [
        envelope.claude.access_token,
        envelope.claude.refresh_token,
        envelope.claude.id_token,
        envelope.tokens.access_token,
        envelope.tokens.refresh_token,
        envelope.tokens.id_token,
    ]
    .into_iter()
    .flatten()
    .map(|secret| secret.0)
    .collect()
}

impl SubscriptionInfo {
    /// Refuse labels containing known OAuth tokens before emitting public metadata.
    pub fn validate_label_credentials(&self, raw: &str) -> Result<(), Error> {
        if redaction_tokens(raw)
            .iter()
            .any(|token| !token.0.is_empty() && self.label.contains(token.0.as_str()))
        {
            return Err(Error::InvalidEntry);
        }
        Ok(())
    }
}

impl Entry {
    /// Create a typed, non-browser, non-sync subscription record.
    pub fn subscription(info: SubscriptionInfo, credential: Secret) -> Result<Self, Error> {
        if !matches!(info.source.as_str(), "claude-code" | "codex-cli")
            || info.account_id.is_empty()
            || info.account_id.len() > 64
            || !info
                .account_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || info.label.trim().is_empty()
            || info.label.chars().count() > 80
            || info.label.chars().any(char::is_control)
            || info.reserve_percent > 99
            || credential.0.len() > 262_144
            || (info.account_id == "default") == info.managed
        {
            return Err(Error::InvalidEntry);
        }
        let subscription_tokens = redaction_tokens(&credential.0);
        if subscription_tokens
            .iter()
            .any(|token| !token.0.is_empty() && info.label.contains(token.0.as_str()))
        {
            return Err(Error::InvalidEntry);
        }
        Ok(Self {
            site: site(&info.source, &info.account_id),
            origins: Vec::new(),
            username: Secret::new(String::new()),
            password: credential,
            totp: None,
            policy: Some(AccessPolicy::WhileUnlocked),
            passkeys: Vec::new(),
            registration: None,
            provider_key: None,
            channel_secret: None,
            subscription: Some(info),
            subscription_tokens,
        })
    }
}

impl<L: PlatformLock> Vault<L> {
    /// Identify typed subscriptions independently of reserved id spelling.
    pub fn is_subscription(&self, site: &str) -> bool {
        self.state.as_ref().is_some_and(|s| {
            s.entries
                .iter()
                .any(|e| e.site == site && e.subscription.is_some())
        })
    }

    /// Return audited value-free subscription metadata.
    pub fn subscriptions(&self, ctx: &AuditContext<'_>) -> Result<Vec<SubscriptionInfo>, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        self.audit(
            ctx,
            AuditAction::Get,
            None,
            None,
            "allowed",
            Some("subscription_metadata"),
        )?;
        Ok(state
            .entries
            .iter()
            .filter_map(|e| e.subscription.clone())
            .collect())
    }

    /// Release the canonical document only to a trusted host callback.
    pub fn with_subscription<T>(
        &self,
        ctx: &AuditContext<'_>,
        source: &str,
        id: &str,
        consume: impl FnOnce(&SubscriptionInfo, &str) -> T,
    ) -> Result<T, Error> {
        if !self.platform.is_unlocked() {
            return Err(Error::Locked);
        }
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let entry = state
            .entries
            .iter()
            .find(|e| {
                e.subscription
                    .as_ref()
                    .is_some_and(|i| i.source == source && i.account_id == id)
            })
            .ok_or(Error::UnknownItem)?;
        self.audit(
            ctx,
            AuditAction::Get,
            Some(&entry.site),
            None,
            "allowed",
            Some("subscription_request"),
        )?;
        Ok(consume(
            entry.subscription.as_ref().ok_or(Error::UnknownItem)?,
            &entry.password.0,
        ))
    }

    /// Atomically persist a new revision; stale writers cannot recreate removed records.
    pub fn set_subscription(
        &mut self,
        ctx: &AuditContext<'_>,
        mut info: SubscriptionInfo,
        credential: Secret,
        expected_revision: Option<u64>,
    ) -> Result<SubscriptionInfo, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let id = site(&info.source, &info.account_id);
        let existing = state.entries.iter().find(|e| e.site == id);
        match (existing, expected_revision) {
            (None, None) => info.revision = 1,
            (Some(entry), Some(expected))
                if entry
                    .subscription
                    .as_ref()
                    .is_some_and(|i| i.revision == expected) =>
            {
                info.revision = expected.checked_add(1).ok_or(Error::InvalidEntry)?;
            }
            (Some(_), None) => return Err(Error::ItemExists),
            _ => return Err(Error::UnknownItem),
        }
        let entry = Entry::subscription(info.clone(), credential)?;
        self.audit(
            ctx,
            AuditAction::ItemAdd,
            Some(&id),
            None,
            "allowed",
            Some("subscription"),
        )?;
        self.put(entry)?;
        self.audit(
            ctx,
            AuditAction::ItemAdd,
            None,
            None,
            "added",
            Some("subscription"),
        )
        .map_err(|_| Error::AuditIncomplete)?;
        Ok(info)
    }

    /// Delete only the matching typed revision.
    pub fn delete_subscription(
        &mut self,
        ctx: &AuditContext<'_>,
        source: &str,
        id: &str,
        revision: u64,
    ) -> Result<(), Error> {
        let site = site(source, id);
        let entry = self
            .state
            .as_ref()
            .ok_or(Error::Locked)?
            .entries
            .iter()
            .find(|e| e.site == site)
            .ok_or(Error::UnknownItem)?;
        if !entry
            .subscription
            .as_ref()
            .is_some_and(|i| i.revision == revision)
        {
            return Err(Error::UnknownItem);
        }
        self.delete_item(ctx, &site)
    }
}

/// Read one explicitly addressed macOS generic-password item; never enumerate.
#[cfg(target_os = "macos")]
pub fn read_cli_keychain(
    service: &str,
    account: &str,
) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, Error> {
    let entry = keyring::Entry::new(service, account).map_err(|_| Error::Unavailable)?;
    match entry.get_secret() {
        Ok(bytes) => Ok(Some(zeroize::Zeroizing::new(bytes))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err(Error::Unavailable),
    }
}

/// Materialize one explicitly addressed macOS generic-password item.
#[cfg(target_os = "macos")]
pub fn write_cli_keychain(service: &str, account: &str, bytes: &[u8]) -> Result<(), Error> {
    keyring::Entry::new(service, account)
        .map_err(|_| Error::Unavailable)?
        .set_secret(bytes)
        .map_err(|_| Error::Unavailable)
}

/// Wipe one explicitly addressed managed macOS generic-password item.
#[cfg(target_os = "macos")]
pub fn delete_cli_keychain(service: &str, account: &str) -> Result<(), Error> {
    match keyring::Entry::new(service, account)
        .map_err(|_| Error::Unavailable)?
        .delete_credential()
    {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(Error::Unavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PassphraseLock;

    fn info() -> SubscriptionInfo {
        SubscriptionInfo {
            source: "codex-cli".into(),
            account_id: "account-a".into(),
            label: "Work".into(),
            managed: true,
            enabled: true,
            hidden: false,
            reserve_percent: 10,
            created_at_ms: 100,
            revision: 0,
            external_digest: None,
            external_mtime_ns: None,
            auth_invalid: false,
        }
    }

    #[test]
    fn managed_subscription_revisions_are_atomic_private_and_never_resurrected() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("vault");
        let audit = root.path().join("audit");
        let password = Secret::new("test-subscription-passphrase".into());
        let ctx = AuditContext {
            who: "test",
            session_id: None,
            tool_call_id: None,
        };
        let mut vault = Vault::create(&path, &audit, &password, PassphraseLock::default()).unwrap();
        let original = vault
            .set_subscription(&ctx, info(), Secret::new("old-refresh-canary".into()), None)
            .unwrap();
        let newer = vault
            .set_subscription(
                &ctx,
                original.clone(),
                Secret::new("new-refresh-canary".into()),
                Some(original.revision),
            )
            .unwrap();
        assert_eq!(
            vault
                .set_subscription(
                    &ctx,
                    original.clone(),
                    Secret::new("stale-refresh-canary".into()),
                    Some(original.revision)
                )
                .unwrap_err(),
            Error::UnknownItem
        );
        assert!(vault.items(&ctx).unwrap().is_empty());
        assert!(vault.item_ids().unwrap().is_empty());
        assert_eq!(
            vault
                .export_item(&ctx, "subscription:codex-cli:account-a")
                .unwrap_err(),
            Error::UnknownItem
        );
        assert!(vault.logins_for("https://example.test").unwrap().is_empty());
        let metadata = serde_json::to_string(&vault.subscriptions(&ctx).unwrap()).unwrap();
        assert!(!metadata.contains("refresh-canary"));
        // Reopening after the canonical write simulates a crash before CLI materialization.
        drop(vault);
        let mut vault = Vault::open(&path, &audit, &password, PassphraseLock::default()).unwrap();
        vault
            .with_subscription(&ctx, "codex-cli", "account-a", |info, raw| {
                assert_eq!(info.revision, newer.revision);
                assert!(raw == "new-refresh-canary");
            })
            .unwrap();
        for file in [&path, &audit] {
            assert!(
                !String::from_utf8_lossy(&std::fs::read(file).unwrap()).contains("refresh-canary")
            );
        }
        vault
            .delete_subscription(&ctx, "codex-cli", "account-a", newer.revision)
            .unwrap();
        assert_eq!(
            vault
                .set_subscription(
                    &ctx,
                    newer.clone(),
                    Secret::new("late-refresh-canary".into()),
                    Some(newer.revision)
                )
                .unwrap_err(),
            Error::UnknownItem
        );
    }
}

#[cfg(test)]
mod redaction_tests {
    use super::*;
    #[test]
    fn managed_subscription_nested_tokens_are_individually_redacted() {
        let raw = r#"{"tokens":{"access_token":"fake-access-canary","refresh_token":"fake-refresh-canary","id_token":"fake-id-canary"}}"#;
        let secrets = redaction_tokens(raw);
        let redactor = crate::Redactor::new(&secrets);
        let result = redactor.redact("fake-access-canary fake-refresh-canary fake-id-canary");
        assert!(!result.contains("canary"));
    }
}
