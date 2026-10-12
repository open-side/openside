//! Storage backends behind [`Backend`]. They only ever handle opaque keys, sealed blobs and
//! entity tags; none of them is trusted with plaintext.

use std::{
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{crypto::hex, AccountId, Error};

/// One stored record as listed by the backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    /// Opaque record key (hex HMAC of kind and id).
    pub key: String,
    /// Entity tag of the stored blob; changes whenever the blob changes.
    pub etag: String,
}

/// Result of a conditional write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pushed {
    /// Stored; the new entity tag, if the backend reported one.
    Stored(Option<String>),
    /// The record changed since the entity tag the device last saw; pull and merge first.
    Conflict,
}

/// Dumb storage for sealed records, with conditional writes. Writes name the entity tag the
/// device last saw (`None` = the record must not exist yet), like HTTP `If-Match` /
/// `If-None-Match: *`.
pub trait Backend: Send + Sync {
    /// Every stored record with its entity tag.
    fn list(&self) -> impl Future<Output = Result<Vec<Listed>, Error>> + Send;
    /// One blob and its entity tag; `None` if it does not exist.
    fn get(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<(Vec<u8>, String)>, Error>> + Send;
    /// Store `blob` under `key` if its current entity tag is still `if_match`.
    fn put(
        &self,
        key: &str,
        if_match: Option<&str>,
        blob: &[u8],
    ) -> impl Future<Output = Result<Pushed, Error>> + Send;
}

pub(crate) fn valid_key(key: &str) -> bool {
    key.len() == 32 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Strong entity tag of a blob (its truncated SHA-256), shared by both backends and the fake
/// WebDAV server.
pub(crate) fn etag(blob: &[u8]) -> String {
    hex(&Sha256::digest(blob)[..16])
}

/// Serverless backend: a directory the user syncs by other means (Syncthing, a network
/// share, removable media). Each record is `<dir>/<account>/<key>.blob`. The conditional
/// write is best effort: check and rename are separate steps with no lock, so a concurrent
/// writer (another process, or another machine via the sync tool) can overwrite a version
/// in between. The lost version is not lost for good: its device still holds it with a
/// newer stamp than what it reads back and uploads it again on its next run.
#[derive(Clone, Debug)]
pub struct DirBackend {
    root: PathBuf,
}

impl DirBackend {
    /// Account directory under `dir`, created with owner-only permissions.
    pub fn open(dir: &Path, account: AccountId) -> Result<Self, Error> {
        let root = dir.join(account.hex());
        std::fs::create_dir_all(&root).map_err(|_| Error::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| Error::Io)?;
        }
        Ok(Self { root })
    }

    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        match std::fs::read(self.root.join(format!("{key}.blob"))) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(Error::Io),
        }
    }
}

impl Backend for DirBackend {
    async fn list(&self) -> Result<Vec<Listed>, Error> {
        let mut listed = Vec::new();
        for entry in std::fs::read_dir(&self.root).map_err(|_| Error::Io)? {
            let path = entry.map_err(|_| Error::Io)?.path();
            let Some(key) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".blob"))
                .filter(|key| valid_key(key))
            else {
                continue;
            };
            // A file vanishing mid-listing (another machine's sync tool) is skipped.
            if let Ok(bytes) = std::fs::read(&path) {
                listed.push(Listed {
                    key: key.to_owned(),
                    etag: etag(&bytes),
                });
            }
        }
        Ok(listed)
    }

    async fn get(&self, key: &str) -> Result<Option<(Vec<u8>, String)>, Error> {
        if !valid_key(key) {
            return Err(Error::Format);
        }
        Ok(self.read(key)?.map(|bytes| {
            let tag = etag(&bytes);
            (bytes, tag)
        }))
    }

    async fn put(&self, key: &str, if_match: Option<&str>, blob: &[u8]) -> Result<Pushed, Error> {
        if !valid_key(key) {
            return Err(Error::Format);
        }
        let current = self.read(key)?.map(|bytes| etag(&bytes));
        if current.as_deref() != if_match {
            return Ok(Pushed::Conflict);
        }
        write_atomic(&self.root.join(format!("{key}.blob")), blob)?;
        Ok(Pushed::Stored(Some(etag(blob))))
    }
}

/// Write via a temporary file in the same directory and rename, mode 0600.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    use std::io::Write;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| Error::Io)?;
    file.write_all(bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|_| Error::Io)?;
    file.persist(path).map_err(|_| Error::Io)?;
    Ok(())
}

/// WebDAV-style HTTP backend: one collection per account (`<base>/<account>/`), records as
/// resources `<key>.blob`, `PROPFIND` (Depth 1) for listing with `getetag`, `GET`, and
/// `PUT` with `If-Match` / `If-None-Match: *`. Credentials, when given, go as HTTP Basic.
#[derive(Clone)]
pub struct WebDavBackend {
    client: reqwest::Client,
    collection: String,
    credentials: Option<(String, Zeroizing<String>)>,
}

impl std::fmt::Debug for WebDavBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebDavBackend")
            .field("collection", &self.collection)
            .finish_non_exhaustive()
    }
}

fn transport(error: &reqwest::Error) -> Error {
    Error::Backend(if error.is_timeout() {
        "timeout".into()
    } else if error.is_connect() {
        "connection failed".into()
    } else {
        "request failed".into()
    })
}

fn status_error(status: reqwest::StatusCode) -> Error {
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        Error::Unauthorized
    } else {
        Error::Backend(format!("HTTP {}", status.as_u16()))
    }
}

fn quoted(tag: &str) -> String {
    format!("\"{tag}\"")
}

/// Strong entity tag value without quotes. Weak tags (`W/"…"`) cannot be used in a strong
/// `If-Match` comparison, so a server that only offers them is unsupported.
fn strong_tag(tag: &str) -> Result<String, Error> {
    let tag = tag.trim();
    if tag.starts_with("W/") {
        return Err(Error::Backend("weak entity tag".into()));
    }
    Ok(tag.trim_matches('"').to_owned())
}

impl WebDavBackend {
    /// Backend for `account` under `base`; `credentials` are `(username, password)`.
    pub fn new(
        base: &str,
        account: AccountId,
        credentials: Option<(String, Zeroizing<String>)>,
    ) -> Result<Self, Error> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::Backend("http client".into()))?;
        Ok(Self {
            client,
            collection: format!("{}/{}/", base.trim_end_matches('/'), account.hex()),
            credentials,
        })
    }

    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        let request = self.client.request(method, url);
        match &self.credentials {
            Some((user, password)) => request.basic_auth(user, Some(password.as_str())),
            None => request,
        }
    }

    /// Create the account collection (`MKCOL`); an existing collection is fine.
    pub async fn ensure(&self) -> Result<(), Error> {
        let method = reqwest::Method::from_bytes(b"MKCOL").map_err(|_| Error::Format)?;
        let response = self
            .request(method, &self.collection)
            .send()
            .await
            .map_err(|e| transport(&e))?;
        let status = response.status();
        // 405: the collection already exists.
        if status.is_success() || status == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        Err(status_error(status))
    }
}

/// Contents of the first element with local name `name` (any namespace prefix, attributes
/// allowed) and the offset just past its closing tag.
fn element<'a>(xml: &'a str, name: &str) -> Option<(&'a str, usize)> {
    let mut from = 0;
    while let Some(found) = xml[from..].find('<') {
        let start = from + found + 1;
        let tag_end = start + xml[start..].find('>')?;
        let tag = &xml[start..tag_end];
        let full = tag
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or_default();
        let local = full.rsplit(':').next().unwrap_or_default();
        if local == name && !tag.starts_with('/') && !tag.ends_with('/') {
            let close_tag = format!("</{full}>");
            let close = tag_end + 1 + xml[tag_end + 1..].find(&close_tag)?;
            return Some((&xml[tag_end + 1..close], close + close_tag.len()));
        }
        from = tag_end + 1;
    }
    None
}

/// Every element body with local name `name`, in order.
fn elements<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some((body, after)) = element(rest, name) {
        found.push(body);
        rest = &rest[after..];
    }
    found
}

/// `(href, getetag)` pairs from a `207 Multi-Status` body. Only a `getetag` inside a
/// `propstat` whose status is 200 counts; other propstats report missing properties.
pub(crate) fn multistatus(body: &str) -> Result<Vec<(String, String)>, Error> {
    let mut pairs = Vec::new();
    for response in elements(body, "response") {
        let Some((href, _)) = element(response, "href") else {
            continue;
        };
        for propstat in elements(response, "propstat") {
            let ok = element(propstat, "status")
                .is_some_and(|(s, _)| s.split_whitespace().nth(1) == Some("200"));
            if let (true, Some((tag, _))) = (ok, element(propstat, "getetag")) {
                pairs.push((
                    href.trim().to_owned(),
                    strong_tag(&tag.replace("&quot;", "\""))?,
                ));
            }
        }
    }
    Ok(pairs)
}

impl Backend for WebDavBackend {
    async fn list(&self) -> Result<Vec<Listed>, Error> {
        let method = reqwest::Method::from_bytes(b"PROPFIND").map_err(|_| Error::Format)?;
        let response = self
            .request(method, &self.collection)
            .header("Depth", "1")
            .header("Content-Type", "application/xml")
            .body(r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:getetag/></d:prop></d:propfind>"#)
            .send()
            .await
            .map_err(|e| transport(&e))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if response.status().as_u16() != 207 {
            return Err(status_error(response.status()));
        }
        let body = response.text().await.map_err(|e| transport(&e))?;
        Ok(multistatus(&body)?
            .into_iter()
            .filter_map(|(href, etag)| {
                let name = href.trim_end_matches('/').rsplit('/').next()?;
                let key = name.strip_suffix(".blob").filter(|key| valid_key(key))?;
                Some(Listed {
                    key: key.to_owned(),
                    etag,
                })
            })
            .collect())
    }

    async fn get(&self, key: &str) -> Result<Option<(Vec<u8>, String)>, Error> {
        if !valid_key(key) {
            return Err(Error::Format);
        }
        let response = self
            .request(
                reqwest::Method::GET,
                &format!("{}{key}.blob", self.collection),
            )
            .send()
            .await
            .map_err(|e| transport(&e))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        let tag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Error::Backend("missing ETag".into()))
            .and_then(strong_tag)?;
        let bytes = response.bytes().await.map_err(|e| transport(&e))?;
        Ok(Some((bytes.to_vec(), tag)))
    }

    async fn put(&self, key: &str, if_match: Option<&str>, blob: &[u8]) -> Result<Pushed, Error> {
        if !valid_key(key) {
            return Err(Error::Format);
        }
        let mut request = self
            .request(
                reqwest::Method::PUT,
                &format!("{}{key}.blob", self.collection),
            )
            .header("Content-Type", "application/octet-stream")
            .body(blob.to_vec());
        request = match if_match {
            Some(tag) => request.header("If-Match", quoted(tag)),
            None => request.header("If-None-Match", "*"),
        };
        let response = request.send().await.map_err(|e| transport(&e))?;
        if response.status() == reqwest::StatusCode::PRECONDITION_FAILED {
            return Ok(Pushed::Conflict);
        }
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        // A server may omit the ETag on PUT; the next PROPFIND then reports it.
        let tag = response.headers().get("etag").and_then(|v| v.to_str().ok());
        Ok(Pushed::Stored(tag.map(strong_tag).transpose()?))
    }
}
