use crate::{
    AccessPolicy, AuditAction, AuditContext, Entry, Error, PlatformLock, ReadSecret, Secret, Vault,
};
use serde::{Deserialize, Serialize};

const MAX_PAYLOAD_BYTES: usize = 65_536;

/// Local OAuth account metadata; upstream identities and token material are excluded.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OAuthInfo {
    /// Storage provider (pool/catalog namespace).
    pub provider: String,
    /// Durable provider-local id allocated by the daemon.
    pub account_id: String,
    /// Person-assigned display label.
    pub label: String,
    /// Operator enablement.
    pub enabled: bool,
    /// Protected measured headroom percentage.
    pub reserve_percent: u8,
    /// Creation time in Unix milliseconds.
    pub created_at_ms: u64,
    /// Compare-and-swap revision, assigned by the vault.
    pub revision: u64,
    /// Whether the current revision has definitively failed authentication.
    pub auth_invalid: bool,
}

fn site(provider: &str, account_id: &str) -> String {
    format!("oauth:{provider}:{account_id}")
}

/// Helper output shape; parsed only to validate it and to collect redaction tokens.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct Payload {
    access: ReadSecret,
    refresh: ReadSecret,
    expires: u64,
    #[serde(default)]
    project_id: Option<ReadSecret>,
    #[serde(default)]
    account_id: Option<ReadSecret>,
    #[serde(default)]
    enterprise_url: Option<ReadSecret>,
    #[serde(default)]
    api_endpoint: Option<ReadSecret>,
}

/// Validate the payload and return the token strings that must be redacted.
fn payload_tokens(raw: &str) -> Result<Vec<Secret>, Error> {
    if raw.len() > MAX_PAYLOAD_BYTES {
        return Err(Error::InvalidEntry);
    }
    let payload: Payload = serde_json::from_str(raw).map_err(|_| Error::InvalidEntry)?;
    if payload.access.0 .0.is_empty() {
        return Err(Error::InvalidEntry);
    }
    Ok([payload.access.0, payload.refresh.0]
        .into_iter()
        .filter(|token| !token.0.is_empty())
        .collect())
}

fn valid_info(info: &OAuthInfo) -> bool {
    let provider = info.provider.as_bytes();
    let account = info.account_id.as_bytes();
    !provider.is_empty()
        && provider.len() <= 64
        && provider
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        && !account.is_empty()
        && account.len() <= 64
        && account
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
        && !info.label.trim().is_empty()
        && info.label.chars().count() <= 80
        && !info.label.chars().any(char::is_control)
        && info.reserve_percent <= 99
}

impl Entry {
    /// Create a typed, non-browser, non-sync OAuth account record.
    pub(crate) fn oauth(info: OAuthInfo, payload: Secret) -> Result<Self, Error> {
        if !valid_info(&info) {
            return Err(Error::InvalidEntry);
        }
        let oauth_tokens = payload_tokens(&payload.0)?;
        if oauth_tokens
            .iter()
            .any(|token| info.label.contains(token.0.as_str()))
        {
            return Err(Error::InvalidEntry);
        }
        Ok(Self {
            site: site(&info.provider, &info.account_id),
            origins: Vec::new(),
            username: Secret::new(String::new()),
            password: payload,
            totp: None,
            policy: Some(AccessPolicy::WhileUnlocked),
            passkeys: Vec::new(),
            registration: None,
            provider_key: None,
            channel_secret: None,
            subscription: None,
            subscription_tokens: Vec::new(),
            oauth: Some(info),
            oauth_tokens,
        })
    }
}

impl<L: PlatformLock> Vault<L> {
    /// Identify typed OAuth records independently of id spelling.
    pub(crate) fn is_oauth(&self, site: &str) -> bool {
        self.state.as_ref().is_some_and(|s| {
            s.entries
                .iter()
                .any(|e| e.site == site && e.oauth.is_some())
        })
    }

    /// Return audited value-free OAuth account metadata.
    pub fn oauth_accounts(&self, ctx: &AuditContext<'_>) -> Result<Vec<OAuthInfo>, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        self.audit(
            ctx,
            AuditAction::Get,
            None,
            None,
            "allowed",
            Some("oauth_metadata"),
        )?;
        Ok(state
            .entries
            .iter()
            .filter_map(|e| e.oauth.clone())
            .collect())
    }

    /// Release the stored payload only to a trusted host callback.
    pub fn with_oauth<T>(
        &self,
        ctx: &AuditContext<'_>,
        provider: &str,
        account_id: &str,
        consume: impl FnOnce(&OAuthInfo, &str) -> T,
    ) -> Result<T, Error> {
        if !self.platform.is_unlocked() {
            return Err(Error::Locked);
        }
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let entry = state
            .entries
            .iter()
            .find(|e| {
                e.oauth
                    .as_ref()
                    .is_some_and(|i| i.provider == provider && i.account_id == account_id)
            })
            .ok_or(Error::UnknownItem)?;
        self.audit(
            ctx,
            AuditAction::Get,
            Some(&entry.site),
            None,
            "allowed",
            Some("oauth_request"),
        )?;
        Ok(consume(
            entry.oauth.as_ref().ok_or(Error::UnknownItem)?,
            &entry.password.0,
        ))
    }

    /// Next revision under the CAS rules; stale writers cannot recreate removed records.
    fn next_oauth_revision(&self, id: &str, expected: Option<u64>) -> Result<u64, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let existing = state.entries.iter().find(|e| e.site == id);
        match (existing, expected) {
            (None, None) => Ok(1),
            (Some(entry), Some(expected))
                if entry.oauth.as_ref().is_some_and(|i| i.revision == expected) =>
            {
                expected.checked_add(1).ok_or(Error::InvalidEntry)
            }
            (Some(_), None) => Err(Error::ItemExists),
            _ => Err(Error::UnknownItem),
        }
    }

    /// Atomically persist a new payload revision.
    pub fn set_oauth(
        &mut self,
        ctx: &AuditContext<'_>,
        mut info: OAuthInfo,
        payload: Secret,
        expected_revision: Option<u64>,
    ) -> Result<OAuthInfo, Error> {
        let id = site(&info.provider, &info.account_id);
        info.revision = self.next_oauth_revision(&id, expected_revision)?;
        if let Some(previous) = self.find_oauth(&id) {
            if previous.created_at_ms != info.created_at_ms {
                return Err(Error::InvalidEntry);
            }
        }
        let entry = Entry::oauth(info.clone(), payload)?;
        self.commit_oauth(ctx, &id, entry)?;
        Ok(info)
    }

    /// Change metadata only, keeping the stored payload; same CAS rules as [`Self::set_oauth`].
    pub fn update_oauth_info(
        &mut self,
        ctx: &AuditContext<'_>,
        mut info: OAuthInfo,
        expected_revision: u64,
    ) -> Result<OAuthInfo, Error> {
        let id = site(&info.provider, &info.account_id);
        info.revision = self.next_oauth_revision(&id, Some(expected_revision))?;
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let previous = state
            .entries
            .iter()
            .find(|e| e.site == id)
            .ok_or(Error::UnknownItem)?;
        if previous
            .oauth
            .as_ref()
            .map_or(true, |p| p.created_at_ms != info.created_at_ms)
        {
            return Err(Error::InvalidEntry);
        }
        let payload = Secret::new(previous.password.0.as_str().to_owned());
        let entry = Entry::oauth(info.clone(), payload)?;
        self.commit_oauth(ctx, &id, entry)?;
        Ok(info)
    }

    /// Delete only the matching typed revision.
    pub fn delete_oauth(
        &mut self,
        ctx: &AuditContext<'_>,
        provider: &str,
        account_id: &str,
        revision: u64,
    ) -> Result<(), Error> {
        let site = site(provider, account_id);
        if self.state.is_none() {
            return Err(Error::Locked);
        }
        if !self
            .find_oauth(&site)
            .is_some_and(|i| i.revision == revision)
        {
            return Err(Error::UnknownItem);
        }
        self.delete_item(ctx, &site)
    }

    fn find_oauth(&self, site: &str) -> Option<&OAuthInfo> {
        self.state
            .as_ref()?
            .entries
            .iter()
            .find(|e| e.site == site)?
            .oauth
            .as_ref()
    }

    fn commit_oauth(
        &mut self,
        ctx: &AuditContext<'_>,
        id: &str,
        entry: Entry,
    ) -> Result<(), Error> {
        self.audit(
            ctx,
            AuditAction::ItemAdd,
            Some(id),
            None,
            "allowed",
            Some("oauth"),
        )?;
        self.put(entry)?;
        self.audit(
            ctx,
            AuditAction::ItemAdd,
            None,
            None,
            "added",
            Some("oauth"),
        )
        .map_err(|_| Error::AuditIncomplete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PassphraseLock;

    const PAYLOAD: &str = r#"{"access":"fake-access-canary","refresh":"fake-refresh-canary","expires":1700000000000,"project_id":"fake-project"}"#;

    fn ctx() -> AuditContext<'static> {
        AuditContext {
            who: "test",
            session_id: None,
            tool_call_id: None,
        }
    }

    fn info() -> OAuthInfo {
        OAuthInfo {
            provider: "google-gemini-cli".into(),
            account_id: "acct_1".into(),
            label: "Work".into(),
            enabled: true,
            reserve_percent: 10,
            created_at_ms: 100,
            revision: 0,
            auth_invalid: false,
        }
    }

    fn payload() -> Secret {
        Secret::new(PAYLOAD.into())
    }

    fn vault(root: &tempfile::TempDir) -> Vault<PassphraseLock> {
        Vault::create(
            root.path().join("vault"),
            root.path().join("audit"),
            &Secret::new("test-oauth-passphrase".into()),
            PassphraseLock::default(),
        )
        .unwrap()
    }

    #[test]
    fn created_oauth_accounts_are_private_typed_records() {
        let root = tempfile::tempdir().unwrap();
        let mut vault = vault(&root);
        let ctx = ctx();
        let created = vault.set_oauth(&ctx, info(), payload(), None).unwrap();
        assert_eq!(created.revision, 1);
        let listed = vault.oauth_accounts(&ctx).unwrap();
        assert_eq!(listed, vec![created.clone()]);
        assert!(!serde_json::to_string(&listed).unwrap().contains("canary"));
        vault
            .with_oauth(&ctx, "google-gemini-cli", "acct_1", |info, raw| {
                assert_eq!(info, &created);
                assert!(raw == PAYLOAD);
            })
            .unwrap();
        assert!(vault.items(&ctx).unwrap().is_empty());
        assert!(vault.item_ids().unwrap().is_empty());
        let site = "oauth:google-gemini-cli:acct_1";
        assert_eq!(
            vault.export_item(&ctx, site).unwrap_err(),
            Error::UnknownItem
        );
        vault.remove_item(&ctx, site).unwrap();
        assert_eq!(vault.oauth_accounts(&ctx).unwrap().len(), 1);
        assert!(vault.logins_for("https://example.test").unwrap().is_empty());
        let redacted = vault
            .redactor()
            .unwrap()
            .redact("fake-access-canary fake-refresh-canary");
        assert!(!redacted.contains("canary"));
        for file in ["vault", "audit"] {
            let bytes = std::fs::read(root.path().join(file)).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("canary"));
        }
        // A synced browser item cannot replace the typed record.
        let browser = format!(
            r#"{{"site":"{site}","origins":["https://example.test"],"username":"u","password":"p","totp_seed":null,"policy":null,"passkeys":[]}}"#
        );
        let prepared = crate::PreparedImport::parse(site, browser.as_bytes()).unwrap();
        assert_eq!(
            vault.commit_import(&ctx, prepared).unwrap_err(),
            Error::InvalidEntry
        );
    }

    #[test]
    fn oauth_entries_survive_reopen() {
        let root = tempfile::tempdir().unwrap();
        let mut vault = vault(&root);
        let ctx = ctx();
        let created = vault.set_oauth(&ctx, info(), payload(), None).unwrap();
        drop(vault);
        let vault = Vault::open(
            root.path().join("vault"),
            root.path().join("audit"),
            &Secret::new("test-oauth-passphrase".into()),
            PassphraseLock::default(),
        )
        .unwrap();
        assert_eq!(vault.oauth_accounts(&ctx).unwrap(), vec![created]);
    }

    #[test]
    fn invalid_oauth_records_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let mut vault = vault(&root);
        let ctx = ctx();
        let mut cases: Vec<(OAuthInfo, &str)> = Vec::new();
        for label in [
            "has fake-access-canary",
            "fake-refresh-canary",
            " ",
            "bad\nlabel",
            &"x".repeat(81),
        ] {
            cases.push((
                OAuthInfo {
                    label: label.to_owned(),
                    ..info()
                },
                PAYLOAD,
            ));
        }
        for provider in ["", "Upper", "bad_provider", &"a".repeat(65)] {
            cases.push((
                OAuthInfo {
                    provider: provider.to_owned(),
                    ..info()
                },
                PAYLOAD,
            ));
        }
        for account_id in ["", "bad:id", "bad id", &"a".repeat(65)] {
            cases.push((
                OAuthInfo {
                    account_id: account_id.to_owned(),
                    ..info()
                },
                PAYLOAD,
            ));
        }
        cases.push((
            OAuthInfo {
                reserve_percent: 100,
                ..info()
            },
            PAYLOAD,
        ));
        let oversized = format!(
            r#"{{"access":"a","refresh":"r","expires":1,"api_endpoint":"{}"}}"#,
            "x".repeat(65_536)
        );
        for raw in [
            r#"{"access":"a","refresh":"r","expires":1,"email":"x"}"#,
            r#"{"access":"","refresh":"r","expires":1}"#,
            r#"{"refresh":"r","expires":1}"#,
            r#"{"access":"a","refresh":"r","expires":"soon"}"#,
            r#"["a"]"#,
            "not json",
            oversized.as_str(),
        ] {
            cases.push((info(), raw));
        }
        for (info, raw) in cases {
            assert_eq!(
                vault
                    .set_oauth(&ctx, info, Secret::new(raw.to_owned()), None)
                    .unwrap_err(),
                Error::InvalidEntry
            );
        }
        assert!(vault.oauth_accounts(&ctx).unwrap().is_empty());
        let minimal = r#"{"access":"a","refresh":"","expires":1,"account_id":"u","enterprise_url":"e","api_endpoint":"p"}"#;
        vault
            .set_oauth(&ctx, info(), Secret::new(minimal.into()), None)
            .unwrap();
    }

    #[test]
    fn oauth_revisions_are_compare_and_swap() {
        let root = tempfile::tempdir().unwrap();
        let mut vault = vault(&root);
        let ctx = ctx();
        let first = vault.set_oauth(&ctx, info(), payload(), None).unwrap();
        assert_eq!(
            vault.set_oauth(&ctx, info(), payload(), None).unwrap_err(),
            Error::ItemExists
        );
        let second = vault
            .set_oauth(&ctx, first.clone(), payload(), Some(first.revision))
            .unwrap();
        assert_eq!(second.revision, 2);
        assert_eq!(
            vault
                .set_oauth(&ctx, first.clone(), payload(), Some(first.revision))
                .unwrap_err(),
            Error::UnknownItem
        );
        let refreshed =
            r#"{"access":"new-access-canary","refresh":"new-refresh-canary","expires":2}"#;
        let third = vault
            .set_oauth(
                &ctx,
                second.clone(),
                Secret::new(refreshed.into()),
                Some(second.revision),
            )
            .unwrap();
        let updated = vault
            .update_oauth_info(
                &ctx,
                OAuthInfo {
                    label: "Personal".into(),
                    enabled: false,
                    reserve_percent: 20,
                    auth_invalid: true,
                    ..third.clone()
                },
                third.revision,
            )
            .unwrap();
        assert_eq!(updated.revision, 4);
        assert_eq!(updated.label, "Personal");
        vault
            .with_oauth(&ctx, "google-gemini-cli", "acct_1", |info, raw| {
                assert_eq!(info, &updated);
                assert!(raw == refreshed);
            })
            .unwrap();
        assert_eq!(
            vault
                .update_oauth_info(&ctx, third.clone(), third.revision)
                .unwrap_err(),
            Error::UnknownItem
        );
        assert_eq!(
            vault
                .update_oauth_info(
                    &ctx,
                    OAuthInfo {
                        created_at_ms: 999,
                        ..updated.clone()
                    },
                    updated.revision
                )
                .unwrap_err(),
            Error::InvalidEntry
        );
        assert_eq!(
            vault
                .update_oauth_info(
                    &ctx,
                    OAuthInfo {
                        label: "new-access-canary".into(),
                        ..updated.clone()
                    },
                    updated.revision
                )
                .unwrap_err(),
            Error::InvalidEntry
        );
        assert_eq!(
            vault
                .update_oauth_info(
                    &ctx,
                    OAuthInfo {
                        account_id: "missing".into(),
                        ..updated.clone()
                    },
                    1
                )
                .unwrap_err(),
            Error::UnknownItem
        );
        assert_eq!(
            vault
                .delete_oauth(&ctx, "google-gemini-cli", "acct_1", third.revision)
                .unwrap_err(),
            Error::UnknownItem
        );
        vault
            .delete_oauth(&ctx, "google-gemini-cli", "acct_1", updated.revision)
            .unwrap();
        assert_eq!(
            vault
                .delete_oauth(&ctx, "google-gemini-cli", "acct_1", updated.revision)
                .unwrap_err(),
            Error::UnknownItem
        );
        assert_eq!(
            vault
                .set_oauth(&ctx, updated.clone(), payload(), Some(updated.revision))
                .unwrap_err(),
            Error::UnknownItem
        );
        assert!(vault.oauth_accounts(&ctx).unwrap().is_empty());
    }

    #[test]
    fn locked_vault_refuses_oauth_operations() {
        let root = tempfile::tempdir().unwrap();
        let mut vault = vault(&root);
        let ctx = ctx();
        let created = vault.set_oauth(&ctx, info(), payload(), None).unwrap();
        vault.lock();
        assert_eq!(vault.oauth_accounts(&ctx).unwrap_err(), Error::Locked);
        assert_eq!(
            vault
                .with_oauth(&ctx, "google-gemini-cli", "acct_1", |_, _| ())
                .unwrap_err(),
            Error::Locked
        );
        assert_eq!(
            vault
                .set_oauth(&ctx, created.clone(), payload(), Some(created.revision))
                .unwrap_err(),
            Error::Locked
        );
        assert_eq!(
            vault
                .update_oauth_info(&ctx, created.clone(), created.revision)
                .unwrap_err(),
            Error::Locked
        );
        assert_eq!(
            vault
                .delete_oauth(&ctx, "google-gemini-cli", "acct_1", created.revision)
                .unwrap_err(),
            Error::Locked
        );
    }
}
