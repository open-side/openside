//! Item export/import for end-to-end encrypted device sync (`openside-sync`). The
//! exported bytes contain secrets: the caller must seal them immediately and drop them.

use zeroize::Zeroizing;

use crate::{
    AllowedOrigin, AuditAction, AuditContext, Entry, Error, ItemInfo, PasskeyCredential,
    PlatformLock, ReadEntry, Redactor, Vault, WriteEntry,
};

/// An item received from another device, fully validated and not yet stored. Parsing has
/// no side effects; register [`Self::redactor`] before [`Vault::commit_import`] so the new
/// secrets are masked before they can be used.
pub struct PreparedImport {
    entry: Entry,
}

impl PreparedImport {
    /// Validate [`Vault::export_item`] bytes like a loaded vault file, and require the item to
    /// be `expected_site` (the id the authenticated sync record names). Malformed input is
    /// [`Error::Format`]; any other validation failure, including an identity mismatch, is
    /// [`Error::InvalidEntry`] or a specific validation error.
    pub fn parse(expected_site: &str, bytes: &[u8]) -> Result<Self, Error> {
        let item: ReadEntry = serde_json::from_slice(bytes).map_err(|_| Error::Format)?;
        if item.site != expected_site {
            return Err(Error::InvalidEntry);
        }
        let origins = item
            .origins
            .iter()
            .map(|origin| AllowedOrigin::parse(origin))
            .collect::<Result<Vec<_>, _>>()?;
        let passkeys = item
            .passkeys
            .into_iter()
            .map(|passkey| {
                PasskeyCredential::new(
                    passkey.credential_id,
                    passkey.rp_id,
                    passkey.user_handle,
                    passkey.private_key.0,
                    passkey.sign_count,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let entry = Entry::build(
            item.site,
            origins,
            item.username.0,
            item.password.0,
            ReadEntry::totp(item.totp_seed, item.totp_params)?,
            item.policy,
            passkeys,
        )?;
        Ok(Self { entry })
    }

    /// Masking filter for every secret of the item.
    pub fn redactor(&self) -> Redactor {
        Redactor::new(self.entry.secrets())
    }
}

impl<L: PlatformLock> Vault<L> {
    /// Ids (site labels) of every item, in vault order. Value-free, so not audited.
    pub fn item_ids(&self) -> Result<Vec<String>, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        Ok(state
            .entries
            .iter()
            .map(|entry| entry.site.clone())
            .collect())
    }

    /// Serialize one item, secrets included, for sealing by the sync engine. Audited as a
    /// `get` with detail `sync_export` before any value leaves the vault.
    pub fn export_item(
        &self,
        ctx: &AuditContext<'_>,
        site: &str,
    ) -> Result<Zeroizing<Vec<u8>>, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let entry = state
            .entries
            .iter()
            .find(|entry| entry.site == site)
            .ok_or(Error::UnknownItem)?;
        self.audit(
            ctx,
            AuditAction::Get,
            Some(site),
            None,
            "allowed",
            Some("sync_export"),
        )?;
        let mut bytes = Zeroizing::new(Vec::new());
        let mut wire = WriteEntry::from(entry);
        // The registration marker names a local operation; other devices have no use for it.
        wire.registration = None;
        serde_json::to_writer(&mut *bytes, &wire).map_err(|_| Error::Format)?;
        Ok(bytes)
    }

    /// Insert or replace an item validated by [`PreparedImport::parse`]. The import is
    /// audited (`get`, detail `sync_import`) before anything changes, so an unwritable audit
    /// log leaves the vault untouched; then the item is saved encrypted.
    pub fn commit_import(
        &mut self,
        ctx: &AuditContext<'_>,
        prepared: PreparedImport,
    ) -> Result<ItemInfo, Error> {
        let site = prepared.entry.site.clone();
        self.audit(
            ctx,
            AuditAction::Get,
            Some(&site),
            None,
            "allowed",
            Some("sync_import"),
        )?;
        // The registration marker is local evidence that an interrupted registration saved
        // this item; a synced replacement (e.g. a policy change on another device) must not
        // erase it. It is never taken from the payload, which `parse` builds without one.
        let mut entry = prepared.entry;
        entry.registration = self
            .state
            .as_ref()
            .ok_or(Error::Locked)?
            .entries
            .iter()
            .find(|existing| existing.site == site)
            .and_then(|existing| existing.registration.clone());
        self.put(entry)?;
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let entry = state
            .entries
            .iter()
            .find(|entry| entry.site == site)
            .ok_or(Error::UnknownItem)?;
        Ok(self.info(entry))
    }

    /// Delete one item (a synced deletion), saving before changing in-memory state.
    /// Deleting an unknown id succeeds without writing.
    pub fn remove_item(&mut self, ctx: &AuditContext<'_>, site: &str) -> Result<(), Error> {
        let state = self.state.as_mut().ok_or(Error::Locked)?;
        let Some(index) = state.entries.iter().position(|entry| entry.site == site) else {
            return Ok(());
        };
        self.audit(
            ctx,
            AuditAction::Get,
            Some(site),
            None,
            "allowed",
            Some("sync_delete"),
        )?;
        let state = self.state.as_mut().ok_or(Error::Locked)?;
        let removed = state.entries.remove(index);
        if let Err(error) = self.persist(None, false) {
            if let Some(state) = self.state.as_mut() {
                state.entries.insert(index, removed);
            }
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PreparedImport;
    use crate::{AccessPolicy, AllowedOrigin, AuditContext, Entry, PassphraseLock, Secret, Vault};

    const CTX: AuditContext<'static> = AuditContext {
        who: "sync",
        session_id: None,
        tool_call_id: None,
    };

    fn vault(dir: &std::path::Path, name: &str) -> Vault {
        Vault::create(
            dir.join(format!("{name}.vault")),
            dir.join(format!("{name}.audit")),
            &Secret::new("sync-test-passphrase".into()),
            PassphraseLock::default(),
        )
        .unwrap()
    }

    #[test]
    fn exported_item_imports_on_another_vault_and_deletes_durably() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = vault(dir.path(), "a");
        let mut b = vault(dir.path(), "b");
        a.put(
            Entry::new(
                "bank".into(),
                vec![AllowedOrigin::parse("https://bank.test").unwrap()],
                Secret::new("alice".into()),
                Secret::new("SYNC-PASSWORD-CANARY".into()),
                None,
                AccessPolicy::Never,
            )
            .unwrap(),
        )
        .unwrap();
        let exported = a.export_item(&CTX, "bank").unwrap();
        let info = b
            .commit_import(&CTX, PreparedImport::parse("bank", &exported).unwrap())
            .unwrap();
        assert_eq!(info.origins.len(), 1);
        assert_eq!(info.policy, Some(AccessPolicy::Never));
        assert_eq!(*b.export_item(&CTX, "bank").unwrap(), *exported);
        assert!(!b
            .redactor()
            .unwrap()
            .redact("x SYNC-PASSWORD-CANARY")
            .contains("CANARY"));
        assert!(matches!(
            PreparedImport::parse("bank", b"{\"site\":1}"),
            Err(crate::Error::Format)
        ));

        b.remove_item(&CTX, "bank").unwrap();
        b.remove_item(&CTX, "bank").unwrap();
        drop(b);
        let reopened = Vault::<PassphraseLock>::open(
            dir.path().join("b.vault"),
            dir.path().join("b.audit"),
            &Secret::new("sync-test-passphrase".into()),
            PassphraseLock::default(),
        )
        .unwrap();
        assert!(reopened.item_ids().unwrap().is_empty());
        let audit = std::fs::read_to_string(dir.path().join("b.audit")).unwrap();
        assert!(audit.contains("sync_import") && audit.contains("sync_delete"));
        assert!(!audit.contains("SYNC-PASSWORD-CANARY"));
    }

    fn item(site: &str, password: &str) -> Entry {
        Entry::new(
            site.into(),
            vec![AllowedOrigin::parse(&format!("https://{site}.test")).unwrap()],
            Secret::new("user".into()),
            Secret::new(password.into()),
            None,
            AccessPolicy::Always,
        )
        .unwrap()
    }

    #[test]
    fn mismatched_identity_is_rejected_before_touching_an_existing_item() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = vault(dir.path(), "a");
        let mut b = vault(dir.path(), "b");
        a.put(item("other", "OTHER-REMOTE-PASSWORD")).unwrap();
        b.put(item("other", "OTHER-LOCAL-PASSWORD")).unwrap();
        let before = b.export_item(&CTX, "other").unwrap();
        let exported = a.export_item(&CTX, "other").unwrap();
        // A record for id "bank" whose body is item "other": nothing may change.
        assert!(matches!(
            PreparedImport::parse("bank", &exported),
            Err(crate::Error::InvalidEntry)
        ));
        assert_eq!(*b.export_item(&CTX, "other").unwrap(), *before);
        assert_eq!(b.item_ids().unwrap(), vec!["other".to_owned()]);
    }

    #[test]
    fn audit_failure_aborts_the_import_without_storing_the_item() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = vault(dir.path(), "a");
        let mut b = vault(dir.path(), "b");
        a.put(item("bank", "IMPORT-AUDIT-CANARY")).unwrap();
        let exported = a.export_item(&CTX, "bank").unwrap();
        let prepared = PreparedImport::parse("bank", &exported).unwrap();
        // The redactor exists before the item is stored, so the secret is masked first.
        assert!(!prepared
            .redactor()
            .redact("IMPORT-AUDIT-CANARY")
            .contains("CANARY"));
        let audit = dir.path().join("b.audit");
        let _ = std::fs::remove_file(&audit);
        std::fs::create_dir(&audit).unwrap();
        assert!(b.commit_import(&CTX, prepared).is_err());
        assert!(b.item_ids().unwrap().is_empty());
        drop(b);
        let reopened = Vault::<PassphraseLock>::open(
            dir.path().join("b.vault"),
            dir.path().join("b2.audit"),
            &Secret::new("sync-test-passphrase".into()),
            PassphraseLock::default(),
        )
        .unwrap();
        assert!(reopened.item_ids().unwrap().is_empty());
    }
}
