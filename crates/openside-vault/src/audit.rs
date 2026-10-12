//! Value-free Vault audit records (JSON lines) and their query.

use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

use serde::{Deserialize, Serialize};

use crate::Error;

/// Who asked and on behalf of which agent task. Identifiers are not secret; values that do
/// not look like identifiers are recorded as `invalid`.
#[derive(Clone, Copy, Debug)]
pub struct AuditContext<'a> {
    /// Non-secret caller label `[a-z0-9:_-]{1,64}`, e.g. `opensided` or `api`.
    pub who: &'a str,
    /// Agent session (task) id, when the call came from a session.
    pub session_id: Option<&'a str>,
    /// Model tool-call id, when the call came from a tool.
    pub tool_call_id: Option<&'a str>,
}

/// The operation recorded in the audit `what` field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditAction {
    /// Username/password fill (`vault_login`).
    Fill,
    /// TOTP generation and fill (`vault_otp`, `request_otp`).
    Otp,
    /// An authenticator a person handed over through a `request_otp` ticket was stored.
    OtpRegister,
    /// Passkey creation through a virtual authenticator (`vault_passkey` register).
    PasskeyRegister,
    /// Passkey assertion through a virtual authenticator (`vault_passkey` login).
    PasskeyLogin,
    /// A sign-in a person typed into a `login` ticket was kept (`request_login`).
    LoginSave,
    /// Item metadata read (names, origins, policy; never values).
    Get,
    /// Per-item access policy change.
    PolicyUpdate,
    /// A person added an item through the admin API or CLI.
    ItemAdd,
    /// A person deleted an item through the admin API or CLI.
    ItemDelete,
    /// Vault file creation ([`crate::audit_lifecycle`]).
    Setup,
    /// Opening (decrypting) the vault, including refused attempts.
    Unlock,
    /// Dropping the decrypted vault.
    Lock,
}

impl AuditAction {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Fill => "fill",
            Self::Otp => "otp",
            Self::OtpRegister => "otp_register",
            Self::PasskeyRegister => "passkey_register",
            Self::PasskeyLogin => "passkey_login",
            Self::LoginSave => "login_save",
            Self::Get => "get",
            Self::PolicyUpdate => "policy_update",
            Self::ItemAdd => "item_add",
            Self::ItemDelete => "item_delete",
            Self::Setup => "setup",
            Self::Unlock => "unlock",
            Self::Lock => "lock",
        }
    }
}

/// One audit line. `detail` carries a denial reason or the new policy, never a value.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct AuditRecord {
    /// Unix seconds.
    pub when: u64,
    /// Caller label.
    pub who: String,
    /// Operation (`fill`, `otp`, `otp_register`, `passkey_register`, `passkey_login`,
    /// `login_save`, `get`, `policy_update`, `item_add`, `item_delete`, `setup`, `unlock`, `lock`).
    pub what: String,
    /// Item id, or `unknown-site`.
    pub site_id: String,
    /// Normalized page origin, `-` when not applicable, or `invalid-origin`.
    pub origin: String,
    /// `allowed`, `denied`, `filled`, `fill-failed`, `completed`, `failed`, `updated`,
    /// `registered`, `added`, `deleted`, `unlocked` or `locked`.
    pub decision: String,
    /// Agent session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Tool-call id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Denial reason or new policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Filter for [`read_audit`]; every set field must match.
#[derive(Clone, Debug, Default)]
pub struct AuditQuery {
    /// Only records at or after this Unix second.
    pub since: Option<u64>,
    /// Only records for this item id.
    pub site_id: Option<String>,
    /// Only records for this session.
    pub session_id: Option<String>,
    /// Keep only the newest `limit` matches (oldest first).
    pub limit: Option<usize>,
}

/// Read matching records from an audit file; a missing file has no records. A torn or
/// foreign line (e.g. after a crash mid-append) is skipped rather than hiding the rest.
pub fn read_audit(path: impl AsRef<Path>, query: &AuditQuery) -> Result<Vec<AuditRecord>, Error> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(Error::Io),
    };
    let mut records = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|_| Error::Io)?;
        let Ok(record) = serde_json::from_str::<AuditRecord>(&line) else {
            continue;
        };
        if query.since.is_some_and(|since| record.when < since)
            || query
                .site_id
                .as_deref()
                .is_some_and(|site| record.site_id != site)
            || query
                .session_id
                .as_deref()
                .is_some_and(|session| record.session_id.as_deref() != Some(session))
        {
            continue;
        }
        records.push(record);
    }
    if let Some(limit) = query.limit {
        let skip = records.len().saturating_sub(limit);
        records.drain(..skip);
    }
    Ok(records)
}

pub(crate) fn identifier(value: Option<&str>) -> Option<&str> {
    value.map(|value| {
        if !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
        {
            value
        } else {
            "invalid"
        }
    })
}
