use crate::{AccessPolicy, AuditAction, AuditContext, Entry, Error, PlatformLock, Secret, Vault};
use serde::{Deserialize, Serialize};

/// Public metadata of an encrypted provider entry; never contains credential material.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderKeyInfo {
    /// Managed API provider, not an upstream account identity.
    pub provider: String,
    /// Durable source-local account id; old single-provider entries retain `legacy`.
    #[serde(default = "legacy_id")]
    pub key_id: String,
    /// Person-assigned display label, never derived from upstream identity.
    #[serde(default = "legacy_label")]
    pub label: String,
    /// Operator enablement, separate from authentication/quota readiness.
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// Remaining measured usage protected from automatic selection.
    #[serde(default = "reserve")]
    pub reserve_percent: u8,
    /// Four trailing characters only; computed from the value inside the Vault.
    pub last_four: String,
    /// UTC creation/replacement time as Unix milliseconds.
    pub created_at_ms: u64,
}

fn legacy_id() -> String {
    "legacy".into()
}
fn legacy_label() -> String {
    "API key".into()
}
fn enabled() -> bool {
    true
}
fn reserve() -> u8 {
    10
}

fn site(provider: &str, key_id: &str) -> String {
    // Preserve encrypted single-provider entries without renaming unrelated browser items.
    if key_id == "legacy" {
        format!("provider:{provider}")
    } else {
        format!("provider:{provider}:{key_id}")
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_label(label: &str, key: &str) -> bool {
    !label.trim().is_empty()
        && label.chars().count() <= 80
        && !label.chars().any(char::is_control)
        && !label.contains(key)
}

impl Entry {
    /// Empty origins prevent provider credentials from being released through browser fill.
    pub fn provider_key(
        provider: &str,
        key_id: &str,
        label: &str,
        key: Secret,
        created_at_ms: u64,
    ) -> Result<Self, Error> {
        if !matches!(provider, "anthropic" | "openai" | "google")
            || key.0.len() < 8
            || key.0.len() > 8192
            || !key.0.bytes().all(|byte| byte.is_ascii_graphic())
            || !valid_id(key_id)
            || !valid_label(label, &key.0)
        {
            return Err(Error::InvalidEntry);
        }
        let last_four = key.0[key.0.len() - 4..].to_owned();
        Ok(Self {
            site: site(provider, key_id),
            origins: Vec::new(),
            username: Secret::new(String::new()),
            password: key,
            totp: None,
            policy: Some(AccessPolicy::WhileUnlocked),
            passkeys: Vec::new(),
            registration: None,
            provider_key: Some(ProviderKeyInfo {
                provider: provider.into(),
                key_id: key_id.into(),
                label: label.into(),
                last_four,
                created_at_ms,
                enabled: true,
                reserve_percent: 10,
            }),
            channel_secret: None,
            subscription: None,
            subscription_tokens: Vec::new(),
        })
    }
}

impl<L: PlatformLock> Vault<L> {
    /// Type metadata, not id spelling, keeps old browser items with provider-like labels valid.
    pub fn is_provider_key(&self, site: &str) -> bool {
        self.state.as_ref().is_some_and(|state| {
            state
                .entries
                .iter()
                .any(|entry| entry.site == site && entry.provider_key.is_some())
        })
    }
    /// Audited administrator metadata; provider entries remain outside browser item listings.
    pub fn provider_keys(&self, ctx: &AuditContext<'_>) -> Result<Vec<ProviderKeyInfo>, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        self.audit(
            ctx,
            AuditAction::Get,
            None,
            None,
            "allowed",
            Some("provider_metadata"),
        )?;
        Ok(state
            .entries
            .iter()
            .filter_map(|entry| entry.provider_key.clone())
            .collect())
    }

    /// Audited insert/replace; the caller registers the entry's redactor before persistence.
    pub fn set_provider_key(
        &mut self,
        ctx: &AuditContext<'_>,
        mut entry: Entry,
    ) -> Result<ProviderKeyInfo, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        if state
            .entries
            .iter()
            .any(|existing| existing.site == entry.site && existing.provider_key.is_none())
        {
            return Err(Error::ItemExists);
        }
        if let Some(previous) = state
            .entries
            .iter()
            .find(|previous| previous.site == entry.site)
            .and_then(|previous| previous.provider_key.as_ref())
        {
            let info = entry.provider_key.as_mut().ok_or(Error::InvalidEntry)?;
            info.enabled = previous.enabled;
            info.reserve_percent = previous.reserve_percent;
        }
        let info = entry.provider_key.clone().ok_or(Error::InvalidEntry)?;
        self.audit(
            ctx,
            AuditAction::ItemAdd,
            Some(&entry.site),
            None,
            "allowed",
            Some("provider_key"),
        )?;
        self.put(entry)?;
        self.audit(
            ctx,
            AuditAction::ItemAdd,
            None,
            None,
            "added",
            Some("provider_key"),
        )
        .map_err(|_| Error::AuditIncomplete)?;
        Ok(info)
    }

    /// Mutate only account metadata, preserving the secret and rolling back failed persistence.
    pub fn update_provider_key(
        &mut self,
        ctx: &AuditContext<'_>,
        provider: &str,
        key_id: &str,
        label: Option<&str>,
        enabled: Option<bool>,
        reserve_percent: Option<u8>,
    ) -> Result<ProviderKeyInfo, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let index = state
            .entries
            .iter()
            .position(|entry| {
                entry
                    .provider_key
                    .as_ref()
                    .is_some_and(|info| info.provider == provider && info.key_id == key_id)
            })
            .ok_or(Error::UnknownItem)?;
        let entry = &state.entries[index];
        if label.is_some_and(|label| !valid_label(label, &entry.password.0))
            || reserve_percent.is_some_and(|reserve| reserve > 99)
        {
            return Err(Error::InvalidEntry);
        }
        self.audit(
            ctx,
            AuditAction::PolicyUpdate,
            Some(&entry.site),
            None,
            "allowed",
            Some("provider_metadata"),
        )?;
        let state = self.state.as_mut().ok_or(Error::Locked)?;
        let info = state.entries[index]
            .provider_key
            .as_mut()
            .ok_or(Error::UnknownItem)?;
        let previous = info.clone();
        if let Some(label) = label {
            info.label = label.into();
        }
        if let Some(enabled) = enabled {
            info.enabled = enabled;
        }
        if let Some(reserve) = reserve_percent {
            info.reserve_percent = reserve;
        }
        if let Err(error) = self.persist(None, false) {
            if let Some(state) = self.state.as_mut() {
                state.entries[index].provider_key = Some(previous);
            }
            return Err(error);
        }
        self.audit(
            ctx,
            AuditAction::PolicyUpdate,
            None,
            None,
            "updated",
            Some("provider_metadata"),
        )
        .map_err(|_| Error::AuditIncomplete)?;
        self.state
            .as_ref()
            .and_then(|state| state.entries[index].provider_key.clone())
            .ok_or(Error::Locked)
    }

    /// Release exactly one enabled account through an audited private callback.
    pub fn with_provider_key<T>(
        &self,
        ctx: &AuditContext<'_>,
        provider: &str,
        key_id: &str,
        consume: impl FnOnce(&str) -> T,
    ) -> Result<T, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        if !self.platform.is_unlocked() {
            return Err(Error::Locked);
        }
        let entry = state
            .entries
            .iter()
            .find(|entry| {
                entry.provider_key.as_ref().is_some_and(|info| {
                    info.provider == provider && info.key_id == key_id && info.enabled
                })
            })
            .ok_or(Error::UnknownItem)?;
        self.audit(
            ctx,
            AuditAction::Get,
            Some(&entry.site),
            None,
            "allowed",
            Some("provider_request"),
        )?;
        Ok(consume(&entry.password.0))
    }

    /// Delete only a typed provider account, never an ordinary item with a similar id.
    pub fn delete_provider_key(
        &mut self,
        ctx: &AuditContext<'_>,
        provider: &str,
        key_id: &str,
    ) -> Result<(), Error> {
        let entry = self
            .state
            .as_ref()
            .ok_or(Error::Locked)?
            .entries
            .iter()
            .find(|entry| {
                entry
                    .provider_key
                    .as_ref()
                    .is_some_and(|info| info.provider == provider && info.key_id == key_id)
            })
            .ok_or(Error::UnknownItem)?;
        let site = entry.site.clone();
        self.delete_item(ctx, &site)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PassphraseLock, Vault};

    #[test]
    fn provider_key_roundtrip_is_encrypted_and_never_a_browser_or_sync_item() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("vault");
        let audit = root.path().join("audit.jsonl");
        let passphrase = Secret::new("test-provider-passphrase".into());
        let canary = "provider-key-canary-abcdef1234";
        let ctx = AuditContext {
            who: "local",
            session_id: None,
            tool_call_id: None,
        };
        let mut vault =
            Vault::create(&path, &audit, &passphrase, PassphraseLock::default()).unwrap();
        let entry = Entry::provider_key(
            "anthropic",
            "legacy",
            "API key",
            Secret::new(canary.into()),
            1_000,
        )
        .unwrap();
        let info = vault.set_provider_key(&ctx, entry).unwrap();
        assert_eq!(info.last_four, "1234");
        assert!(vault.items(&ctx).unwrap().is_empty());
        assert!(vault.item_ids().unwrap().is_empty());
        assert_eq!(
            vault.export_item(&ctx, "provider:anthropic").unwrap_err(),
            Error::UnknownItem
        );
        assert!(vault
            .logins_for("https://api.anthropic.com")
            .unwrap()
            .is_empty());
        vault
            .with_provider_key(&ctx, "anthropic", "legacy", |key| assert!(key == canary))
            .unwrap();
        drop(vault);
        for file in [&path, &audit] {
            let bytes = std::fs::read(file).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains(canary));
        }
        let mut vault = Vault::open(&path, &audit, &passphrase, PassphraseLock::default()).unwrap();
        assert_eq!(vault.provider_keys(&ctx).unwrap(), vec![info]);
        vault.lock();
        assert_eq!(
            vault
                .with_provider_key(&ctx, "anthropic", "legacy", |_| ())
                .unwrap_err(),
            Error::Locked
        );
    }

    #[test]
    fn provider_key_collision_preserves_legacy_browser_entry() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("vault");
        let audit = root.path().join("audit");
        let passphrase = Secret::new("test-provider-passphrase".into());
        let ctx = AuditContext {
            who: "local",
            session_id: None,
            tool_call_id: None,
        };
        let mut vault =
            Vault::create(&path, &audit, &passphrase, PassphraseLock::default()).unwrap();
        vault
            .put(
                Entry::new(
                    "provider:anthropic".into(),
                    vec![crate::AllowedOrigin::parse("https://example.test").unwrap()],
                    Secret::new(String::new()),
                    Secret::new("ordinary-browser-password".into()),
                    None,
                    AccessPolicy::Always,
                )
                .unwrap(),
            )
            .unwrap();
        let key = Entry::provider_key(
            "anthropic",
            "legacy",
            "API key",
            Secret::new("new-provider-key-abcd".into()),
            1_000,
        )
        .unwrap();
        assert_eq!(
            vault.set_provider_key(&ctx, key).unwrap_err(),
            Error::ItemExists
        );
        drop(vault);
        let vault = Vault::open(&path, &audit, &passphrase, PassphraseLock::default()).unwrap();
        assert!(!vault.is_provider_key("provider:anthropic"));
        assert_eq!(vault.items(&ctx).unwrap().len(), 1);
    }
}

#[cfg(test)]
mod multi_tests {
    use super::*;
    use crate::PassphraseLock;

    #[test]
    fn labelled_provider_accounts_roundtrip_and_mutate_independently_without_secret_output() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("vault");
        let audit = root.path().join("audit");
        let passphrase = Secret::new("test-multi-provider-passphrase".into());
        let ctx = AuditContext {
            who: "local",
            session_id: None,
            tool_call_id: None,
        };
        let mut vault =
            Vault::create(&path, &audit, &passphrase, PassphraseLock::default()).unwrap();
        for (id, label, key) in [
            ("first", "Personal", "provider-first-canary-1111"),
            ("second", "Work", "provider-second-canary-2222"),
        ] {
            vault
                .set_provider_key(
                    &ctx,
                    Entry::provider_key("anthropic", id, label, Secret::new(key.into()), 1)
                        .unwrap(),
                )
                .unwrap();
        }
        let info = vault
            .update_provider_key(
                &ctx,
                "anthropic",
                "first",
                Some("Primary"),
                Some(false),
                Some(20),
            )
            .unwrap();
        assert_eq!(info.label, "Primary");
        assert_eq!(info.reserve_percent, 20);
        assert!(!info.enabled);
        assert_eq!(
            vault
                .with_provider_key(&ctx, "anthropic", "first", |_| ())
                .unwrap_err(),
            Error::UnknownItem
        );
        vault
            .with_provider_key(&ctx, "anthropic", "second", |key| {
                assert_eq!(key, "provider-second-canary-2222")
            })
            .unwrap();
        assert!(vault.items(&ctx).unwrap().is_empty());
        assert!(vault.item_ids().unwrap().is_empty());
        drop(vault);
        let mut vault = Vault::open(&path, &audit, &passphrase, PassphraseLock::default()).unwrap();
        assert_eq!(vault.provider_keys(&ctx).unwrap().len(), 2);
        assert_eq!(vault.provider_keys(&ctx).unwrap()[0], info);
        vault
            .delete_provider_key(&ctx, "anthropic", "first")
            .unwrap();
        vault
            .with_provider_key(&ctx, "anthropic", "second", |_| ())
            .unwrap();
        assert_eq!(vault.provider_keys(&ctx).unwrap().len(), 1);
        for file in [&path, &audit] {
            let bytes = std::fs::read(file).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(!text.contains("provider-first-canary-1111"));
            assert!(!text.contains("provider-second-canary-2222"));
        }
    }
}
