use super::*;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

/// Host-only cookie metadata. There is intentionally no value field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieKey {
    /// CDP domain, including a leading dot for domain cookies.
    pub domain: String,
    /// Case-sensitive cookie key; never the corresponding value.
    pub name: String,
    /// Cookie path used with domain and name for exact deletion.
    pub path: String,
    /// Unix seconds; nonpositive values denote session cookies.
    pub expires: f64,
    /// Whether renderer scripts are excluded from reading the value.
    pub http_only: bool,
    /// Whether Chromium restricts sending the cookie to secure transports.
    pub secure: bool,
}

type CookieMetadata = (CookieKey, Option<Zeroizing<[u8; 32]>>);

/// Opaque, transient comparison state. Never serialized or formatted for diagnostics.
pub struct CookieBaseline {
    key: Zeroizing<[u8; 32]>,
    cookies: Vec<CookieMetadata>,
}

struct SecretReply(Value);
impl Drop for SecretReply {
    fn drop(&mut self) {
        fn wipe(value: &mut Value) {
            match value {
                Value::String(value) => value.zeroize(),
                Value::Array(values) => values.iter_mut().for_each(wipe),
                Value::Object(values) => values.values_mut().for_each(wipe),
                _ => {}
            }
        }
        wipe(&mut self.0);
    }
}

fn cookie_metadata(
    mut reply: SecretReply,
    key: Option<&[u8]>,
) -> Result<Vec<CookieMetadata>, BrowserError> {
    let cookies = reply.0["cookies"]
        .as_array_mut()
        .ok_or(BrowserError::InvalidResponse("invalid cookie metadata"))?;
    let mut result = Vec::with_capacity(cookies.len());
    for cookie in cookies {
        let Value::String(value) = cookie["value"].take() else {
            return Err(BrowserError::InvalidResponse("invalid cookie value"));
        };
        let value = Zeroizing::new(value);
        let digest = key
            .map(|key| {
                let mut mac = Hmac::<Sha256>::new_from_slice(key)
                    .map_err(|_| BrowserError::InvalidResponse("invalid baseline key"))?;
                mac.update(value.as_bytes());
                Ok::<_, BrowserError>(Zeroizing::new(mac.finalize().into_bytes().into()))
            })
            .transpose()?;
        drop(value);
        let metadata = serde_json::from_value(cookie.take())
            .map_err(|_| BrowserError::InvalidResponse("invalid cookie metadata"))?;
        result.push((metadata, digest));
    }
    Ok(result)
}

impl Browser {
    /// Visit all browser-context cookie values for output redaction, wiping each after use.
    pub async fn visit_tab_cookie_values(
        &self,
        owner: &str,
        tab: &str,
        mut visit: impl FnMut(&str),
    ) -> Result<(), BrowserError> {
        self.owned_session(owner, tab).await?;
        // Script can fetch paths and origins absent from the current frame tree.
        let mut reply = SecretReply(
            self.inner
                .cdp
                .call_sensitive(None, "Storage.getCookies", json!({}))
                .await
                .map_err(|_| BrowserError::InvalidResponse("cookie output guard unavailable"))?,
        );
        let cookies = reply.0["cookies"]
            .as_array_mut()
            .ok_or(BrowserError::InvalidResponse("invalid cookie output guard"))?;
        for cookie in cookies {
            let Value::String(value) = cookie["value"].take() else {
                return Err(BrowserError::InvalidResponse("invalid cookie output guard"));
            };
            let value = Zeroizing::new(value);
            if !value.is_empty() {
                visit(&value);
            }
        }
        Ok(())
    }

    /// Read only shared-profile cookie keys. Untrusted page CDP cannot call this method.
    pub async fn site_cookie_keys(&self) -> Result<Vec<CookieKey>, BrowserError> {
        let reply = self
            .inner
            .cdp
            .call(None, "Storage.getCookies", json!({}))
            .await
            .map_err(|_| BrowserError::InvalidResponse("cookie metadata unavailable"))?;
        Ok(cookie_metadata(SecretReply(reply), None)?
            .into_iter()
            .map(|(key, _)| key)
            .collect())
    }

    /// A fresh per-login key prevents correlating transient digests across login attempts.
    pub async fn cookie_baseline(&self) -> Result<CookieBaseline, BrowserError> {
        let mut key = Zeroizing::new([0; 32]);
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, key.as_mut());
        let reply = self
            .inner
            .cdp
            .call(None, "Storage.getCookies", json!({}))
            .await
            .map_err(|_| BrowserError::InvalidResponse("cookie baseline unavailable"))?;
        let cookies = cookie_metadata(SecretReply(reply), Some(key.as_ref()))?;
        Ok(CookieBaseline { key, cookies })
    }

    /// Only changed key metadata crosses the host boundary; digests die with the baseline.
    pub async fn changed_cookie_keys(
        &self,
        before: &CookieBaseline,
    ) -> Result<Vec<CookieKey>, BrowserError> {
        let reply = self
            .inner
            .cdp
            .call(None, "Storage.getCookies", json!({}))
            .await
            .map_err(|_| BrowserError::InvalidResponse("cookie changes unavailable"))?;
        let after = cookie_metadata(SecretReply(reply), Some(before.key.as_ref()))?;
        Ok(after
            .into_iter()
            .filter(|(key, digest)| {
                !before.cookies.iter().any(|(old, old_digest)| {
                    old == key && old_digest.as_deref() == digest.as_deref()
                })
            })
            .map(|(key, _)| key)
            .collect())
    }

    /// Host-only revocation closes shared-profile pages embedding any matching origin,
    /// including unowned pages, so a surviving frame cannot repopulate cleared storage.
    pub async fn quiesce_storage_origins(
        &self,
        matches: impl Fn(&str) -> bool,
    ) -> Result<Vec<String>, BrowserError> {
        let mut origins: std::collections::BTreeSet<_> = self
            .inner
            .state
            .lock()
            .await
            .committed_origins
            .iter()
            .filter(|origin| matches(origin))
            .cloned()
            .collect();
        for tab in self
            .tabs()
            .await
            .into_iter()
            .filter(|tab| tab.browser_context_id.is_none())
        {
            let session = self.session(&tab.id).await?;
            let frames = self.frames_of(&session, &tab.id).await?;
            let mut affected = false;
            for frame in frames {
                if matches(&frame.url) {
                    if let Some(origin) = committed_origin(&frame.url) {
                        origins.insert(origin);
                    }
                    affected = true;
                }
            }
            if affected {
                if let Some(owner) = &tab.owner_session_id {
                    self.close_agent_tab(owner, &tab.id).await?;
                } else {
                    let reply = self
                        .inner
                        .cdp
                        .call(None, "Target.closeTarget", json!({"targetId":tab.id}))
                        .await?;
                    if reply["success"] != true {
                        return Err(BrowserError::TabNotReady(tab.id));
                    }
                }
            }
        }
        let targets = self
            .inner
            .cdp
            .call(None, "Target.getTargets", json!({}))
            .await?;
        let isolated = self.inner.state.lock().await.isolated_contexts.clone();
        for target in targets["targetInfos"].as_array().into_iter().flatten() {
            if target["browserContextId"]
                .as_str()
                .is_some_and(|context| isolated.contains(context))
                || !matches!(
                    target["type"].as_str(),
                    Some("worker" | "shared_worker" | "service_worker")
                )
            {
                continue;
            }
            let Some(url) = target["url"].as_str().filter(|url| matches(url)) else {
                continue;
            };
            if let Some(origin) = committed_origin(url) {
                origins.insert(origin);
            }
            if target["type"] != "service_worker" {
                let reply = self
                    .inner
                    .cdp
                    .call(
                        None,
                        "Target.closeTarget",
                        json!({"targetId":target["targetId"]}),
                    )
                    .await?;
                if reply["success"] != true {
                    return Err(BrowserError::InvalidResponse("worker could not be stopped"));
                }
            }
        }
        Ok(origins.into_iter().collect())
    }

    /// The daemon supplies PSL-checked keys and origins while holding its browser/site gates.
    pub async fn sign_out_site(
        &self,
        keys: &[CookieKey],
        origins: &[String],
    ) -> Result<(), BrowserError> {
        let target = self
            .inner
            .cdp
            .call(
                None,
                "Target.createTarget",
                json!({"url":"about:blank","background":true}),
            )
            .await?;
        let target = required_string(&target, "targetId")?;
        let result = async {
            let attached = self
                .inner
                .cdp
                .call(
                    None,
                    "Target.attachToTarget",
                    json!({"targetId":target,"flatten":true}),
                )
                .await?;
            let session = required_string(&attached, "sessionId")?;
            // Unregister and terminate origin workers before deleting their writable stores.
            for origin in origins {
                self.inner
                    .cdp
                    .call(
                        Some(session),
                        "Storage.clearDataForOrigin",
                        json!({"origin":origin,"storageTypes":"service_workers"}),
                    )
                    .await?;
            }
            for key in keys {
                self.inner
                    .cdp
                    .call(
                        Some(session),
                        "Network.deleteCookies",
                        json!({"name":key.name,"domain":key.domain,"path":key.path}),
                    )
                    .await?;
            }
            // Origin storage commands need a page's storage partition, not the browser target.
            for origin in origins {
                self.inner
                    .cdp
                    .call(
                        Some(session),
                        "Storage.clearDataForOrigin",
                        json!({"origin":origin,"storageTypes":"all"}),
                    )
                    .await?;
            }
            Ok::<_, BrowserError>(())
        }
        .await;
        let closed = self
            .inner
            .cdp
            .call(None, "Target.closeTarget", json!({"targetId":target}))
            .await;
        result.map_err(|_| BrowserError::InvalidResponse("site data could not be cleared"))?;
        closed
            .map_err(|_| BrowserError::InvalidResponse("cookie cleanup tab could not be closed"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(value: &str) -> SecretReply {
        SecretReply(json!({"cookies":[{
            "domain":".example.com","name":"session","path":"/",
            "expires":4102444800.0,"httpOnly":true,"secure":true,"value":value
        }]}))
    }

    #[test]
    fn connections_cookie_metadata_never_contains_value_or_digest() {
        const CANARY: &str = "COOKIE_VALUE_CANARY_928ca";
        let keys = cookie_metadata(response(CANARY), None).unwrap();
        assert!(keys[0].1.is_none());
        let wire = serde_json::to_string(&keys[0].0).unwrap();
        assert!(!wire.contains(CANARY));
        assert!(!wire.contains("value"));
        assert!(!wire.contains("digest"));
    }

    #[test]
    fn connections_ephemeral_digest_detects_same_key_rotation() {
        let before = cookie_metadata(response("COOKIE_BEFORE_CANARY"), Some(&[7; 32])).unwrap();
        let after = cookie_metadata(response("COOKIE_AFTER_CANARY"), Some(&[7; 32])).unwrap();
        let other_baseline =
            cookie_metadata(response("COOKIE_BEFORE_CANARY"), Some(&[8; 32])).unwrap();
        assert_eq!(before[0].0, after[0].0);
        assert_ne!(before[0].1.as_deref(), after[0].1.as_deref());
        assert_ne!(before[0].1.as_deref(), other_baseline[0].1.as_deref());
    }
}
