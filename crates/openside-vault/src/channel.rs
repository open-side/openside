use crate::{AccessPolicy, AuditAction, AuditContext, Entry, Error, PlatformLock, Secret, Vault};
use serde::{Deserialize, Serialize};

/// Typed local credential metadata; never contains token material or token suffixes.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChannelSecretInfo {
    /// Protocol identity validated before encryption.
    pub kind: String,
    /// Stable account id, not a platform user id.
    pub account_id: String,
    /// UTC creation or rotation time as Unix milliseconds.
    pub created_at_ms: u64,
}

impl Entry {
    /// Origins are empty: a platform identity must never become a browser login.
    pub fn channel_secret(
        kind: &str,
        account_id: &str,
        token: Secret,
        signing_secret: Option<Secret>,
        created_at_ms: u64,
    ) -> Result<Self, Error> {
        let valid = |s: &Secret| {
            s.0.len() >= 8 && s.0.len() <= 8192 && s.0.bytes().all(|b| b.is_ascii_graphic())
        };
        if !matches!(kind, "telegram" | "slack" | "discord")
            || account_id.is_empty()
            || account_id.len() > 40
            || !account_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
            || !valid(&token)
            || (kind == "slack" && !signing_secret.as_ref().is_some_and(valid))
            || (kind != "slack" && signing_secret.is_some())
        {
            return Err(Error::InvalidEntry);
        }
        Ok(Self {
            site: format!("channel:{kind}:{account_id}"),
            origins: Vec::new(),
            username: signing_secret.unwrap_or_else(|| Secret::new(String::new())),
            password: token,
            totp: None,
            policy: Some(AccessPolicy::WhileUnlocked),
            passkeys: Vec::new(),
            registration: None,
            subscription: None,
            subscription_tokens: Vec::new(),
            provider_key: None,
            channel_secret: Some(ChannelSecretInfo {
                kind: kind.into(),
                account_id: account_id.into(),
                created_at_ms,
            }),
        })
    }
}
impl<L: PlatformLock> Vault<L> {
    /// Match by entry type, preserving ordinary browser items with similar names.
    pub fn is_channel_secret(&self, site: &str) -> bool {
        self.state.as_ref().is_some_and(|s| {
            s.entries
                .iter()
                .any(|e| e.site == site && e.channel_secret.is_some())
        })
    }
    /// Audited metadata listing, excluded from the browser's ordinary item listing.
    pub fn channel_secrets(&self, ctx: &AuditContext<'_>) -> Result<Vec<ChannelSecretInfo>, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        self.audit(
            ctx,
            AuditAction::Get,
            None,
            None,
            "allowed",
            Some("channel_metadata"),
        )?;
        Ok(state
            .entries
            .iter()
            .filter_map(|e| e.channel_secret.clone())
            .collect())
    }
    /// Save only a typed channel entry; reject collisions with other item types.
    pub fn set_channel_secret(
        &mut self,
        ctx: &AuditContext<'_>,
        entry: Entry,
    ) -> Result<ChannelSecretInfo, Error> {
        let info = entry.channel_secret.clone().ok_or(Error::InvalidEntry)?;
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        if state
            .entries
            .iter()
            .any(|e| e.site == entry.site && e.channel_secret.is_none())
        {
            return Err(Error::ItemExists);
        }
        self.audit(
            ctx,
            AuditAction::ItemAdd,
            Some(&entry.site),
            None,
            "allowed",
            Some("channel_secret"),
        )?;
        self.put(entry)?;
        self.audit(
            ctx,
            AuditAction::ItemAdd,
            None,
            None,
            "added",
            Some("channel_secret"),
        )
        .map_err(|_| Error::AuditIncomplete)?;
        Ok(info)
    }
    /// Trusted adapter callback only; payloads must never be attached to an API response.
    /// Service identities follow the Vault's explicit lock, not OS screen/keyring idle locks.
    pub fn with_channel_secret<T>(
        &self,
        ctx: &AuditContext<'_>,
        kind: &str,
        account_id: &str,
        consume: impl FnOnce(&str, Option<&str>) -> T,
    ) -> Result<T, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let entry = state
            .entries
            .iter()
            .find(|e| {
                e.channel_secret
                    .as_ref()
                    .is_some_and(|i| i.kind == kind && i.account_id == account_id)
            })
            .ok_or(Error::UnknownItem)?;
        self.audit(
            ctx,
            AuditAction::Get,
            Some(&entry.site),
            None,
            "allowed",
            Some("channel_session"),
        )?;
        Ok(consume(
            &entry.password.0,
            (!entry.username.0.is_empty()).then_some(entry.username.0.as_str()),
        ))
    }
    /// Revoke only the requested identity, never a same-named browser entry.
    pub fn delete_channel_secret(
        &mut self,
        ctx: &AuditContext<'_>,
        kind: &str,
        account_id: &str,
    ) -> Result<(), Error> {
        self.state.as_ref().ok_or(Error::Locked)?;
        let site = format!("channel:{kind}:{account_id}");
        if !self.is_channel_secret(&site) {
            return Err(Error::UnknownItem);
        }
        self.delete_item(ctx, &site)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PassphraseLock;
    #[test]
    fn channel_canary_never_leaves_metadata_debug_browser_sync_or_disk() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("vault");
        let audit = root.path().join("audit");
        let passphrase = Secret::new("channel-vault-passphrase".into());
        let canary = "channel-secret-canary-12345678";
        let ctx = AuditContext {
            who: "local",
            session_id: None,
            tool_call_id: None,
        };
        let mut vault =
            Vault::create(&path, &audit, &passphrase, PassphraseLock::default()).unwrap();
        let entry =
            Entry::channel_secret("telegram", "work", Secret::new(canary.into()), None, 100)
                .unwrap();
        assert!(!format!("{entry:?}").contains(canary));
        let info = vault.set_channel_secret(&ctx, entry).unwrap();
        assert!(!serde_json::to_string(&info).unwrap().contains(canary));
        assert!(vault.items(&ctx).unwrap().is_empty());
        assert!(vault.item_ids().unwrap().is_empty());
        assert!(vault
            .logins_for("https://api.telegram.org")
            .unwrap()
            .is_empty());
        assert_eq!(
            vault
                .export_item(&ctx, "channel:telegram:work")
                .unwrap_err(),
            Error::UnknownItem
        );
        vault
            .with_channel_secret(&ctx, "telegram", "work", |s, _| assert_eq!(s, canary))
            .unwrap();
        drop(vault);
        for f in [&path, &audit] {
            assert!(!String::from_utf8_lossy(&std::fs::read(f).unwrap()).contains(canary));
        }
        let mut vault = Vault::open(&path, &audit, &passphrase, PassphraseLock::default()).unwrap();
        assert_eq!(vault.channel_secrets(&ctx).unwrap(), vec![info]);
        vault.lock();
        assert_eq!(
            vault
                .with_channel_secret(&ctx, "telegram", "work", |_, _| ())
                .unwrap_err(),
            Error::Locked
        );
        assert_eq!(
            vault
                .delete_channel_secret(&ctx, "telegram", "work")
                .unwrap_err(),
            Error::Locked
        );
    }
    #[test]
    fn channel_service_identity_follows_explicit_vault_lock_not_os_keyring_idle() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        struct Lock(Arc<AtomicBool>);
        impl PlatformLock for Lock {
            fn unlock(&mut self) -> Result<(), Error> {
                self.0.store(true, Ordering::SeqCst);
                Ok(())
            }
            fn lock(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
            fn is_unlocked(&self) -> bool {
                self.0.load(Ordering::SeqCst)
            }
        }
        let root = tempfile::tempdir().unwrap();
        let platform = Arc::new(AtomicBool::new(false));
        let mut vault = Vault::create(
            root.path().join("vault"),
            root.path().join("audit"),
            &Secret::new("channel-lock-passphrase".into()),
            Lock(platform.clone()),
        )
        .unwrap();
        let ctx = AuditContext {
            who: "local",
            session_id: None,
            tool_call_id: None,
        };
        vault
            .set_channel_secret(
                &ctx,
                Entry::channel_secret(
                    "telegram",
                    "work",
                    Secret::new("service-token-before-idle".into()),
                    None,
                    1,
                )
                .unwrap(),
            )
            .unwrap();
        platform.store(false, Ordering::SeqCst);
        assert!(vault.is_unlocked());
        vault
            .with_channel_secret(&ctx, "telegram", "work", |token, _| {
                assert_eq!(token, "service-token-before-idle")
            })
            .unwrap();
        vault
            .set_channel_secret(
                &ctx,
                Entry::channel_secret(
                    "telegram",
                    "work",
                    Secret::new("service-token-after-idle".into()),
                    None,
                    2,
                )
                .unwrap(),
            )
            .unwrap();
        vault
            .with_channel_secret(&ctx, "telegram", "work", |token, _| {
                assert_eq!(token, "service-token-after-idle")
            })
            .unwrap();
        vault.lock();
        assert_eq!(
            vault
                .with_channel_secret(&ctx, "telegram", "work", |_, _| ())
                .unwrap_err(),
            Error::Locked
        );
    }
}
