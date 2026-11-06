use crate::{AccessPolicy, AuditAction, AuditContext, Entry, Error, PlatformLock, Secret, Vault};
use serde::{Deserialize, Serialize};

/// Value-free metadata of one secret an external MCP server receives (an environment
/// variable of a stdio server or an HTTP header of a remote one).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpSecretInfo {
    /// Configured server id (`[mcp] servers[].id`).
    pub server_id: String,
    /// Environment variable or header name, exactly as configured.
    pub name: String,
    /// UTC creation or replacement time as Unix milliseconds.
    pub created_at_ms: u64,
}

fn valid_server(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn site(server_id: &str, name: &str) -> String {
    // Header names are case-insensitive and the config rejects case-insensitive duplicates,
    // so the lower-cased name is a stable, collision-free item id.
    format!("mcp:{server_id}:{}", name.to_ascii_lowercase())
}

impl Entry {
    /// Origins are empty: a server credential must never become a browser login.
    pub fn mcp_secret(
        server_id: &str,
        name: &str,
        value: Secret,
        created_at_ms: u64,
    ) -> Result<Self, Error> {
        // A value is written verbatim into a header or the environment, so line breaks and
        // NUL (header injection, truncated environment) are refused.
        if !valid_server(server_id)
            || !valid_name(name)
            || value.0.is_empty()
            || value.0.len() > 8192
            || value.0.chars().any(|c| matches!(c, '\0' | '\r' | '\n'))
        {
            return Err(Error::InvalidEntry);
        }
        Ok(Self {
            site: site(server_id, name),
            origins: Vec::new(),
            username: Secret::new(String::new()),
            password: value,
            totp: None,
            policy: Some(AccessPolicy::WhileUnlocked),
            passkeys: Vec::new(),
            registration: None,
            provider_key: None,
            channel_secret: None,
            mcp_secret: Some(McpSecretInfo {
                server_id: server_id.into(),
                name: name.into(),
                created_at_ms,
            }),
            subscription: None,
            subscription_tokens: Vec::new(),
            oauth: None,
            oauth_tokens: Vec::new(),
        })
    }
}

impl<L: PlatformLock> Vault<L> {
    /// Match by entry type, preserving ordinary browser items with similar names.
    pub fn is_mcp_secret(&self, site: &str) -> bool {
        self.state.as_ref().is_some_and(|s| {
            s.entries
                .iter()
                .any(|e| e.site == site && e.mcp_secret.is_some())
        })
    }

    /// Audited metadata listing of one server's secrets; values never leave the vault here.
    pub fn mcp_secrets(
        &self,
        ctx: &AuditContext<'_>,
        server_id: &str,
    ) -> Result<Vec<McpSecretInfo>, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        self.audit(
            ctx,
            AuditAction::Get,
            None,
            None,
            "allowed",
            Some("mcp_metadata"),
        )?;
        Ok(state
            .entries
            .iter()
            .filter_map(|e| e.mcp_secret.clone())
            .filter(|info| info.server_id == server_id)
            .collect())
    }

    /// Save only a typed MCP entry; reject collisions with other item types.
    pub fn set_mcp_secret(
        &mut self,
        ctx: &AuditContext<'_>,
        entry: Entry,
    ) -> Result<McpSecretInfo, Error> {
        let info = entry.mcp_secret.clone().ok_or(Error::InvalidEntry)?;
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        if state
            .entries
            .iter()
            .any(|e| e.site == entry.site && e.mcp_secret.is_none())
        {
            return Err(Error::ItemExists);
        }
        self.audit(
            ctx,
            AuditAction::ItemAdd,
            Some(&entry.site),
            None,
            "allowed",
            Some("mcp_secret"),
        )?;
        self.put(entry)?;
        self.audit(
            ctx,
            AuditAction::ItemAdd,
            None,
            None,
            "added",
            Some("mcp_secret"),
        )
        .map_err(|_| Error::AuditIncomplete)?;
        Ok(info)
    }

    /// Trusted connector callback only: `consume` receives `(name, value)` for every secret
    /// of the server and must never attach a value to an API response or log.
    pub fn with_mcp_secrets<T>(
        &self,
        ctx: &AuditContext<'_>,
        server_id: &str,
        consume: impl FnOnce(&[(&str, &str)]) -> T,
    ) -> Result<T, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let pairs: Vec<(&str, &str)> = state
            .entries
            .iter()
            .filter_map(|e| {
                e.mcp_secret
                    .as_ref()
                    .filter(|info| info.server_id == server_id)
                    .map(|info| (info.name.as_str(), e.password.0.as_str()))
            })
            .collect();
        self.audit(
            ctx,
            AuditAction::Get,
            None,
            None,
            "allowed",
            Some("mcp_session"),
        )?;
        Ok(consume(&pairs))
    }

    /// Revoke one secret of a server, never a same-named browser entry.
    pub fn delete_mcp_secret(
        &mut self,
        ctx: &AuditContext<'_>,
        server_id: &str,
        name: &str,
    ) -> Result<(), Error> {
        self.state.as_ref().ok_or(Error::Locked)?;
        let site = site(server_id, name);
        if !self.is_mcp_secret(&site) {
            return Err(Error::UnknownItem);
        }
        self.delete_item(ctx, &site)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_secret_validates_identity_and_value() {
        let ok = Entry::mcp_secret("files", "API_TOKEN", Secret::new("value-1".into()), 1);
        assert!(ok.is_ok_and(|e| e.site == "mcp:files:api_token" && e.origins.is_empty()));
        for (server, name, value) in [
            ("Files", "API_TOKEN", "v"),
            ("files", "", "v"),
            ("files", "bad name", "v"),
            ("files", "X", ""),
            ("files", "X", "a\r\nInjected: 1"),
            ("files", "X", "nul\0"),
        ] {
            assert!(
                Entry::mcp_secret(server, name, Secret::new(value.into()), 1).is_err(),
                "{server} {name}"
            );
        }
    }
}
