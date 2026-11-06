//! Person-requested bulk operations on saved sign-ins: CSV export and clearing every login,
//! authenticator and passkey. Typed records (provider keys, channel/MCP secrets,
//! subscriptions, OAuth accounts) are never part of either: they are not passwords and other
//! features own their lifecycle.

use zeroize::Zeroizing;

use crate::{AuditAction, AuditContext, Entry, Error, TotpAlgorithm, Vault};

/// What [`Vault::export_csv`] produced. `csv` holds every exported value; the host must hand
/// it only to the person who asked and drop it right after.
pub struct CsvExport {
    /// `name,url,username,password,otpauth` rows, header first.
    pub csv: Zeroizing<String>,
    /// Items written (one row per allowed origin).
    pub exported: usize,
    /// Password-manager items with nothing a CSV can carry (passkey-only).
    pub skipped: usize,
}

impl std::fmt::Debug for CsvExport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CsvExport")
            .field("exported", &self.exported)
            .field("skipped", &self.skipped)
            .finish_non_exhaustive()
    }
}

impl Entry {
    /// A login, authenticator or passkey item, as opposed to a typed record.
    pub(crate) fn is_login_item(&self) -> bool {
        self.provider_key.is_none()
            && self.channel_secret.is_none()
            && self.mcp_secret.is_none()
            && self.subscription.is_none()
            && self.oauth.is_none()
    }
}

fn push_field(out: &mut String, value: &str) {
    if value.contains([',', '"', '\n', '\r']) || value.trim() != value {
        out.push('"');
        for ch in value.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
    } else {
        out.push_str(value);
    }
}

/// The `name` column with spreadsheet formulas defused: a label starting with `=`, `+`, `-`,
/// `@`, tab or CR gets a leading apostrophe. `url` and `otpauth` always start with their
/// scheme. Usernames and passwords stay byte-exact on purpose: the file exists to be
/// imported by another password manager, and an altered password would no longer sign in.
fn push_label(out: &mut String, value: &str) {
    if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        let mut defused = String::with_capacity(value.len() + 1);
        defused.push('\'');
        defused.push_str(value);
        push_field(out, &defused);
    } else {
        push_field(out, value);
    }
}

fn otpauth(entry: &Entry) -> Zeroizing<String> {
    let Some(totp) = entry.totp.as_ref() else {
        return Zeroizing::new(String::new());
    };
    let params = totp.params();
    let algorithm = match params.algorithm() {
        TotpAlgorithm::Sha1 => "SHA1",
        TotpAlgorithm::Sha256 => "SHA256",
        TotpAlgorithm::Sha512 => "SHA512",
    };
    let seed = totp.seed.0.to_ascii_uppercase().replace([' ', '='], "");
    Zeroizing::new(format!(
        "otpauth://totp/{label}?secret={seed}&digits={}&period={}&algorithm={algorithm}",
        params.digits(),
        params.period(),
        label = url::form_urlencoded::byte_serialize(entry.site.as_bytes()).collect::<String>(),
    ))
}

impl<L: crate::PlatformLock> Vault<L> {
    /// Every saved sign-in and authenticator as CSV that browsers and password managers
    /// import (`name,url,username,password,otpauth`; one row per allowed origin, a wildcard
    /// origin written as its base host). Passkeys cannot travel in CSV and are counted in
    /// `skipped`. Audited as `export`: the `allowed` record is written before any value is
    /// read and `exported` after; a locked vault is recorded as `denied`. The decision to
    /// release is the host's (fresh authentication, confirmation); this only records it.
    pub fn export_csv(&self, ctx: &AuditContext<'_>) -> Result<CsvExport, Error> {
        let action = AuditAction::Export;
        let Some(state) = self.state.as_ref() else {
            self.audit(ctx, action, None, None, "denied", Some("locked"))?;
            return Err(Error::Locked);
        };
        self.audit(ctx, action, None, None, "allowed", Some("csv"))?;
        let mut csv = Zeroizing::new(String::from("name,url,username,password,otpauth\n"));
        let (mut exported, mut skipped) = (0, 0);
        for entry in state.entries.iter().filter(|entry| entry.is_login_item()) {
            if entry.password.0.is_empty() && entry.totp.is_none() {
                skipped += 1;
                continue;
            }
            let otpauth = otpauth(entry);
            for origin in &entry.origins {
                push_label(&mut csv, &entry.site);
                csv.push(',');
                push_field(&mut csv, &origin.export_url());
                csv.push(',');
                push_field(&mut csv, &entry.username.0);
                csv.push(',');
                push_field(&mut csv, &entry.password.0);
                csv.push(',');
                push_field(&mut csv, &otpauth);
                csv.push('\n');
            }
            exported += 1;
        }
        self.audit(ctx, action, None, None, "exported", Some("csv"))
            .map_err(|_| Error::AuditIncomplete)?;
        Ok(CsvExport {
            csv,
            exported,
            skipped,
        })
    }

    /// Delete every saved sign-in, authenticator and passkey in one write, keeping typed
    /// records. Audited as `clear` (`allowed` before the write, `cleared` with the count
    /// after); an unwritable decision record leaves everything in place, and a failed write
    /// restores the in-memory items. [`Error::AuditIncomplete`] means the items *are* gone.
    pub fn clear_logins(&mut self, ctx: &AuditContext<'_>) -> Result<usize, Error> {
        let action = AuditAction::Clear;
        let Some(state) = self.state.as_ref() else {
            self.audit(ctx, action, None, None, "denied", Some("locked"))?;
            return Err(Error::Locked);
        };
        let count = state
            .entries
            .iter()
            .filter(|entry| entry.is_login_item())
            .count();
        self.audit(ctx, action, None, None, "allowed", None)?;
        let state = self.state.as_mut().ok_or(Error::Locked)?;
        let (removed, kept): (Vec<Entry>, Vec<Entry>) = std::mem::take(&mut state.entries)
            .into_iter()
            .partition(Entry::is_login_item);
        state.entries = kept;
        if let Err(error) = self.persist(None, false) {
            if let Some(state) = self.state.as_mut() {
                state.entries.extend(removed);
            }
            let _ = self.audit(ctx, action, None, None, "failed", None);
            return Err(error);
        }
        drop(removed);
        let detail = count.to_string();
        self.audit(ctx, action, None, None, "cleared", Some(&detail))
            .map_err(|_| Error::AuditIncomplete)?;
        Ok(count)
    }
}
