//! Software passkeys (WebAuthn credentials) kept inside the encrypted vault.

use std::fmt;

use zeroize::Zeroizing;

use crate::{Error, Secret};

/// A WebAuthn credential as exported by a CDP virtual authenticator. The private key
/// (base64 PKCS#8) is a secret; the credential id, RP ID and user handle are not.
pub struct PasskeyCredential {
    pub(crate) credential_id: String,
    pub(crate) rp_id: String,
    pub(crate) user_handle: Option<String>,
    pub(crate) private_key: Secret,
    pub(crate) sign_count: u32,
}

impl fmt::Debug for PasskeyCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl PasskeyCredential {
    /// Validate and wrap a captured credential. Ids are base64/base64url text as CDP uses.
    pub fn new(
        credential_id: String,
        rp_id: String,
        user_handle: Option<String>,
        private_key: Secret,
        sign_count: u32,
    ) -> Result<Self, Error> {
        let base64 = |value: &str| {
            !value.is_empty()
                && value.len() <= 2048
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+/=-_".contains(&b))
        };
        let rp_valid = !rp_id.is_empty()
            && rp_id.len() <= 253
            && rp_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-.".contains(&b));
        if !base64(&credential_id)
            || !rp_valid
            || user_handle.as_deref().is_some_and(|handle| !base64(handle))
            || private_key.0.is_empty()
        {
            return Err(Error::InvalidEntry);
        }
        Ok(Self {
            credential_id,
            rp_id,
            user_handle,
            private_key,
            sign_count,
        })
    }

    /// Credential id as sent to the relying party.
    pub fn credential_id(&self) -> &str {
        &self.credential_id
    }

    /// Relying-party id the credential is scoped to.
    pub fn rp_id(&self) -> &str {
        &self.rp_id
    }
}

/// A zeroizing copy of a stored passkey, lent only to trusted host code that installs it
/// into a browser virtual authenticator.
pub struct PasskeyRelease {
    /// Credential id.
    pub credential_id: String,
    /// Relying-party id.
    pub rp_id: String,
    /// User handle, when the credential is discoverable.
    pub user_handle: Option<String>,
    /// Base64 PKCS#8 private key.
    pub private_key: Zeroizing<String>,
    /// Signature counter last observed.
    pub sign_count: u32,
}

impl fmt::Debug for PasskeyRelease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Signature counter observed after a passkey login.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignCount {
    /// Credential id.
    pub credential_id: String,
    /// New counter value; lower values than the stored one are ignored.
    pub sign_count: u32,
}
