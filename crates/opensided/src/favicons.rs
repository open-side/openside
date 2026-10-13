//! One normalized favicon boundary for browser observations, tickets and presentation cards.
use crate::{api::HttpError, engine::Shared, tools::ToolEnv};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use base64::Engine as _;
use openside_browser::Browser;
use openside_proto::{Block, Card, CardSite, EventKind, FaviconRef, SessionId};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    future::Future,
    time::{Duration, Instant},
};

const PREFIX: &str = "data:image/png;base64,";
const MAX_ENTRIES: usize = 256;
const MAX_BYTES: usize = 2 * 1024 * 1024;
const SUCCESS_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const NEGATIVE_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    origin: String,
    session: SessionId,
}
struct Entry {
    pixels: Option<Bytes>,
    expires: Instant,
    used: u64,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<Key, Entry>,
    clock: u64,
    bytes: usize,
}
impl Cache {
    fn get(&mut self, key: &Key, now: Instant) -> Option<Option<Bytes>> {
        if self
            .entries
            .get(key)
            .is_some_and(|entry| entry.expires <= now)
        {
            self.remove(key);
        }
        let entry = self.entries.get_mut(key)?;
        self.clock = self.clock.wrapping_add(1);
        entry.used = self.clock;
        Some(entry.pixels.clone())
    }
    fn remove(&mut self, key: &Key) {
        if let Some(entry) = self.entries.remove(key) {
            self.bytes -= entry.pixels.as_ref().map_or(0, |v| v.len());
        }
    }
    fn insert(&mut self, key: Key, pixels: Option<Vec<u8>>, now: Instant) {
        self.remove(&key);
        let size = pixels.as_ref().map_or(0, Vec::len);
        if size > 8192 {
            return;
        }
        while self.entries.len() >= MAX_ENTRIES || self.bytes + size > MAX_BYTES {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, v)| v.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.remove(&oldest);
        }
        self.clock = self.clock.wrapping_add(1);
        self.bytes += size;
        let ttl = if pixels.is_some() {
            SUCCESS_TTL
        } else {
            NEGATIVE_TTL
        };
        self.entries.insert(
            key,
            Entry {
                pixels: pixels.map(Bytes::from),
                expires: now + ttl,
                used: self.clock,
            },
        );
    }
}

#[derive(Default)]
pub(crate) struct Service {
    cache: Mutex<Cache>,
    // Serial admission also bounds in-flight key bookkeeping; repeated callers recheck the
    // cache after taking this gate, so a burst can never start duplicate work for a key.
    fetch_gate: tokio::sync::Mutex<()>,
}

impl Service {
    #[cfg(test)]
    pub(crate) fn seed(&self, session: SessionId, origin: &str, data: &str) {
        let pixels = Browser::validated_favicon(data).expect("test supplies a validated icon");
        self.cache.lock().insert(
            Key {
                session,
                origin: origin.into(),
            },
            Some(pixels),
            Instant::now(),
        );
    }

    async fn reference(
        &self,
        key: Key,
        private: bool,
        fetch: impl Future<Output = Option<Vec<u8>>>,
    ) -> Option<FaviconRef> {
        if private {
            let _gate = self.fetch_gate.lock().await;
            return fetch.await.map(|bytes| data_ref(&bytes));
        }
        let cached = self.cache.lock().get(&key, Instant::now());
        let available = match cached {
            Some(bytes) => bytes.is_some(),
            None => {
                let _gate = self.fetch_gate.lock().await;
                let cached = self.cache.lock().get(&key, Instant::now());
                if let Some(bytes) = cached {
                    bytes.is_some()
                } else {
                    let bytes = fetch.await;
                    let available = bytes.is_some();
                    self.cache.lock().insert(key.clone(), bytes, Instant::now());
                    available
                }
            }
        };
        available.then_some(FaviconRef::Origin { origin: key.origin })
    }
}

pub(crate) fn validated_ref(data_url: &str) -> Option<FaviconRef> {
    Browser::validated_favicon(data_url)?;
    Some(FaviconRef::Data {
        data_url: data_url.into(),
    })
}

pub(crate) async fn for_tab(
    browser: &Browser,
    owner: &str,
    tab: &str,
    origin: &str,
) -> Option<String> {
    let origin_matches = |url: &str| {
        url::Url::parse(url).is_ok_and(|url| url.origin().ascii_serialization() == origin)
    };
    browser.check_owned(owner, tab).await.ok()?;
    if !browser
        .tabs()
        .await
        .iter()
        .any(|info| info.id == tab && origin_matches(&info.url))
    {
        return None;
    }
    let data = browser.favicon(owner, tab).await.ok().flatten()?;
    if !browser
        .tabs()
        .await
        .iter()
        .any(|info| info.id == tab && origin_matches(&info.url))
    {
        return None;
    }
    Some(data)
}

fn canonical(origin: &str) -> bool {
    url::Url::parse(origin).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
            && url.origin().ascii_serialization() == origin
    })
}
fn data_ref(bytes: &[u8]) -> FaviconRef {
    FaviconRef::Data {
        data_url: format!(
            "{PREFIX}{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ),
    }
}

async fn browser_pixels(origin: &str, ctx: &ToolEnv) -> Option<Vec<u8>> {
    let browser = ctx
        .shared
        .registry
        .browser
        .session_browser(&ctx.session)
        .await?;
    let tabs = browser.tabs().await;
    for tab in tabs {
        if !url::Url::parse(&tab.url).is_ok_and(|u| u.origin().ascii_serialization() == origin)
            || browser
                .check_owned(ctx.session.as_str(), &tab.id)
                .await
                .is_err()
        {
            continue;
        }
        if let Ok(events) = ctx.shared.db.events(&ctx.session, 0).await {
            for event in events.iter().rev() {
                if let EventKind::BrowserState(state) = &event.kind {
                    if state.tab_id != tab.id {
                        continue;
                    }
                    if url::Url::parse(&state.url)
                        .is_ok_and(|u| u.origin().ascii_serialization() == origin)
                    {
                        if let Some(bytes) = state
                            .favicon
                            .as_deref()
                            .and_then(Browser::validated_favicon)
                        {
                            return Some(bytes);
                        }
                    }
                    break;
                }
            }
        }
        if let Some(data) = for_tab(&browser, ctx.session.as_str(), &tab.id, origin).await {
            return Browser::validated_favicon(&data);
        }
    }
    None
}

async fn pixels(origin: &str, ctx: &ToolEnv) -> Option<Vec<u8>> {
    if let Some(bytes) = browser_pixels(origin, ctx).await {
        return Some(bytes);
    }
    if !origin.starts_with("https://") {
        return None;
    }
    crate::card_media::fetch_normalized(&format!("{origin}/favicon.ico"), 65536, true, ctx)
        .await
        .ok()
        .map(|(pixels, _)| pixels)
}

async fn populate_site(site: &mut CardSite, ctx: &ToolEnv, private: bool) {
    site.favicon_ref = None;
    let Some(origin) = site.origin.as_deref().filter(|origin| canonical(origin)) else {
        return;
    };
    let key = Key {
        origin: origin.into(),
        session: ctx.session.clone(),
    };
    site.favicon_ref = ctx
        .shared
        .favicons
        .reference(key, private, pixels(origin, ctx))
        .await;
}

pub(crate) async fn populate(card: &mut Card, ctx: &ToolEnv) {
    let Ok(state) = ctx.shared.db.state(&ctx.session).await else {
        return;
    };
    // Isolated spaces use session-local pixels too: the origin-only public route cannot
    // encode which of two browser partitions supplied a site's icon.
    let private = state.incognito || state.browser_space == openside_proto::BrowserSpace::Isolated;
    populate_site(&mut card.site, ctx, private).await;
    for block in &mut card.blocks {
        match block {
            Block::ListItems { items, .. } => {
                for item in items {
                    if let Some(site) = &mut item.site {
                        populate_site(site, ctx, private).await;
                    }
                }
            }
            Block::Table {
                row_sites: Some(sites),
                ..
            } => {
                for site in sites.iter_mut().flatten() {
                    populate_site(site, ctx, private).await;
                }
            }
            _ => {}
        }
    }
}

fn references(card: &Card, origin: &str) -> bool {
    let site = |site: &CardSite| {
        matches!(&site.favicon_ref,Some(FaviconRef::Origin {origin: found}) if found == origin)
            && site.origin.as_deref() == Some(origin)
    };
    site(&card.site)
        || card.blocks.iter().any(|block| match block {
            Block::ListItems { items, .. } => {
                items.iter().filter_map(|item| item.site.as_ref()).any(site)
            }
            Block::Table {
                row_sites: Some(sites),
                ..
            } => sites.iter().flatten().any(site),
            _ => false,
        })
}

pub(crate) async fn get(
    State(shared): State<Shared>,
    Path(origin): Path<String>,
) -> Result<Response, HttpError> {
    let miss = || HttpError(StatusCode::NOT_FOUND, "favicon_unavailable");
    if !canonical(&origin) {
        return Err(miss());
    }
    let candidates: Vec<_> = shared
        .favicons
        .cache
        .lock()
        .entries
        .keys()
        .filter(|key| key.origin == origin)
        .cloned()
        .collect();
    for key in candidates {
        let Ok(state) = shared.db.state(&key.session).await else {
            continue;
        };
        if state.deleted
            || state.incognito
            || state.browser_space == openside_proto::BrowserSpace::Isolated
        {
            continue;
        }
        let referenced = state
            .cards
            .values()
            .chain(
                state
                    .tickets
                    .values()
                    .filter_map(|ticket| ticket.card.as_ref()),
            )
            .any(|card| references(card, &origin));
        if !referenced {
            continue;
        }
        let Some(Some(bytes)) = shared.favicons.cache.lock().get(&key, Instant::now()) else {
            continue;
        };
        let mut response = bytes.into_response();
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("image/png"),
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            header::HeaderValue::from_static("nosniff"),
        );
        headers.insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("private,max-age=300"),
        );
        return Ok(response);
    }
    Err(miss())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(n: usize) -> Key {
        Key {
            origin: format!("https://{n}.example.test"),
            session: SessionId::new("test"),
        }
    }
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&vec![0; width as usize * height as usize * 4])
                .unwrap();
        }
        bytes
    }
    #[test]
    fn favicon_validation_decodes_not_just_the_signature() {
        let make = |bytes: &[u8]| {
            format!(
                "{PREFIX}{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            )
        };
        assert!(validated_ref(&make(&png(32, 32))).is_some());
        assert!(validated_ref(&make(&png(33, 32))).is_none());
        assert!(validated_ref(&make(&vec![0; 8193])).is_none());
        let mut truncated = png(32, 32);
        truncated.truncate(24);
        assert!(validated_ref(&make(&truncated)).is_none());
        assert!(validated_ref("data:image/svg+xml;base64,PHN2Zy8+").is_none());
    }
    #[test]
    fn cache_is_lru_bounded_and_expires_positive_and_negative_entries() {
        let mut cache = Cache::default();
        let now = Instant::now();
        for n in 0..MAX_ENTRIES {
            cache.insert(key(n), Some(vec![0; 8192]), now);
        }
        assert_eq!(cache.bytes, MAX_BYTES);
        cache.get(&key(0), now).unwrap();
        cache.insert(key(MAX_ENTRIES), Some(vec![0; 8192]), now);
        assert!(cache.get(&key(1), now).is_none());
        assert!(cache.get(&key(0), now).is_some());
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert!(cache.get(&key(0), now + SUCCESS_TTL).is_none());
        cache.insert(key(999), None, now);
        assert!(matches!(cache.get(&key(999), now), Some(None)));
        assert!(cache.get(&key(999), now + NEGATIVE_TTL).is_none());
        assert!(cache.bytes <= MAX_BYTES);
    }
    #[test]
    fn origin_identity_cannot_be_a_path_or_credentials() {
        assert!(canonical("https://xn--bcher-kva.example:8443"));
        for origin in [
            "https://example.test/",
            "https://user@example.test",
            "http://example.test/path",
            "data:image/png;base64,a",
        ] {
            assert!(!canonical(origin));
        }
    }

    #[tokio::test]
    async fn concurrent_same_key_fetches_are_coalesced_including_negative_results() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for result in [Some(png(16, 16)), None] {
            let service = Service::default();
            let calls = AtomicUsize::new(0);
            let fetch = || async {
                calls.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
                result.clone()
            };
            let (first, second) = tokio::join!(
                service.reference(key(1), false, fetch()),
                service.reference(key(1), false, fetch())
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(first, second);
            assert_eq!(first.is_some(), result.is_some());
        }
    }

    #[tokio::test]
    async fn private_icons_never_enter_or_read_the_global_origin_cache() {
        let service = Service::default();
        let public = png(16, 16);
        let private = png(8, 8);
        let public_ref = service
            .reference(key(1), false, async { Some(public.clone()) })
            .await;
        assert!(matches!(public_ref, Some(FaviconRef::Origin { .. })));
        let private_ref = service
            .reference(key(1), true, async { Some(private.clone()) })
            .await;
        assert_eq!(private_ref, Some(data_ref(&private)));
        {
            let cache = service.cache.lock();
            assert_eq!(cache.entries.len(), 1);
            assert_eq!(
                cache.entries[&key(1)].pixels.as_deref(),
                Some(public.as_slice())
            );
        }
        let mut separate = key(1);
        separate.session = SessionId::new("private-partition");
        service
            .reference(separate.clone(), true, async { Some(private) })
            .await;
        assert!(!service.cache.lock().entries.contains_key(&separate));
    }
}
