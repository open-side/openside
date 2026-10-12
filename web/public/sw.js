// Offline shell only: the API (/v1/*) is never cached or intercepted, so session
// data and tokens stay out of Cache Storage.
const CACHE = "openside-shell-v5";
// Every unhashed shell file: fetched network-first so a new build (manifest, icons, theme boot)
// is picked up without a cache rename, and cached for offline. Only hashed /assets/ are cache-first.
const FRESH = [
  "/",
  "/theme-boot.js",
  "/manifest.webmanifest",
  "/icons/icon.svg",
  "/icons/icon-192.png",
  "/icons/icon-512.png",
  "/icons/icon-maskable-192.png",
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll(FRESH))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((key) => key !== CACHE).map((key) => caches.delete(key))))
      .then(() => self.clients.claim()),
  );
});

// Web Share Target (manifest share_target): the phone's share sheet POSTs here. A service
// worker is the only place that can read the multipart body, so it parks the payload in
// IndexedDB and redirects into the app, which turns it into a draft (src/pwa/share.ts). The
// database name, version and stores mirror src/pwa/idb.ts.
const SHARE_PATH = "/share-target";
const SHARE_STORE = "share";
const MAX_SHARED_FILES = 4;

function openShareDatabase() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open("openside-pwa", 1);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains("outbox")) db.createObjectStore("outbox", { keyPath: "message_id" });
      if (!db.objectStoreNames.contains(SHARE_STORE)) db.createObjectStore(SHARE_STORE, { keyPath: "id" });
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function storeShare(item) {
  const db = await openShareDatabase();
  try {
    await new Promise((resolve, reject) => {
      const tx = db.transaction(SHARE_STORE, "readwrite");
      tx.objectStore(SHARE_STORE).put(item);
      tx.oncomplete = resolve;
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  } finally {
    db.close();
  }
}

function formText(form, name) {
  const value = form.get(name);
  return typeof value === "string" ? value : "";
}

// 303 turns the POST into a GET of the app; the absolute URL is what Response.redirect requires.
function redirectTo(path) {
  return Response.redirect(new URL(path, self.location.origin).href, 303);
}

async function receiveShare(request) {
  try {
    const form = await request.formData();
    // Bytes, not File objects: Safari has historically lost File metadata in IndexedDB.
    const files = await Promise.all(
      form
        .getAll("files")
        .filter((entry) => typeof entry !== "string" && entry.size > 0)
        .slice(0, MAX_SHARED_FILES)
        .map(async (file) => ({ name: file.name, type: file.type, data: await file.arrayBuffer() })),
    );
    const id = crypto.randomUUID();
    await storeShare({
      id,
      title: formText(form, "title"),
      text: formText(form, "text"),
      url: formText(form, "url"),
      files,
      received_at: Date.now(),
    });
    return redirectTo(`/?share=${id}`);
  } catch {
    return redirectTo("/?share=failed");
  }
}

self.addEventListener("fetch", (event) => {
  const request = event.request;
  const url = new URL(request.url);
  if (request.method === "POST" && url.origin === self.location.origin && url.pathname === SHARE_PATH) {
    event.respondWith(receiveShare(request));
    return;
  }
  const api = url.pathname === "/v1" || url.pathname.startsWith("/v1/");
  if (request.method !== "GET" || url.origin !== self.location.origin || api) return;

  const fresh = request.mode === "navigate" ? "/" : FRESH.includes(url.pathname) ? url.pathname : null;
  if (fresh) {
    // Network first so a new build is picked up; the cached copy covers offline.
    event.respondWith(
      fetch(request)
        .then((response) => {
          const copy = response.clone();
          if (response.ok) caches.open(CACHE).then((cache) => cache.put(fresh, copy));
          return response;
        })
        .catch(() => caches.match(fresh).then((cached) => cached ?? Response.error())),
    );
    return;
  }

  if (url.pathname.startsWith("/assets/")) {
    // Hashed assets are immutable, so cache-first is safe.
    event.respondWith(
      caches.match(request).then(
        (cached) =>
          cached ??
          fetch(request).then((response) => {
            const copy = response.clone();
            if (response.ok) caches.open(CACHE).then((cache) => cache.put(request, copy));
            return response;
          }),
      ),
    );
  }
});

// Web Push from opensided (docs/REMOTE.md): the payload is end-to-end encrypted to this
// subscription and carries only a title, a short body and an in-app link.
function sameOriginPath(value) {
  try {
    const url = new URL(typeof value === "string" ? value : "/", self.location.origin);
    return url.origin === self.location.origin ? url.pathname + url.search + url.hash : "/";
  } catch {
    return "/";
  }
}

// The installed icon's badge counts the "needs you" notifications still on screen; the open app
// replaces this with the exact pending-ticket count (src/pwa/badge.ts).
async function syncBadge() {
  if (!self.navigator || !self.navigator.setAppBadge) return;
  const shown = await self.registration.getNotifications();
  const waiting = shown.filter((notification) => notification.data && notification.data.kind === "ticket").length;
  if (waiting > 0) await self.navigator.setAppBadge(waiting);
  else if (self.navigator.clearAppBadge) await self.navigator.clearAppBadge();
}

self.addEventListener("push", (event) => {
  let message = {};
  try {
    message = event.data ? event.data.json() : {};
  } catch {
    message = {};
  }
  const url = sameOriginPath(message.url);
  event.waitUntil(
    self.registration
      .showNotification(message.title || "openside", {
        body: message.body || "",
        // One notification per ticket or session: a newer push replaces the older one.
        tag: message.ticket_id || message.session_id || "openside",
        icon: "/icons/icon-192.png",
        data: { url, kind: message.kind, session_id: message.session_id, ticket_id: message.ticket_id },
      })
      .then(syncBadge)
      .catch(() => undefined),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const url = sameOriginPath(event.notification.data && event.notification.data.url);
  event.waitUntil(
    Promise.all([
      self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((windows) => {
        const open = windows.find((client) => new URL(client.url).origin === self.location.origin);
        if (open) return open.focus().then((client) => (client.navigate ? client.navigate(url) : client));
        return self.clients.openWindow(url);
      }),
      syncBadge().catch(() => undefined),
    ]),
  );
});

self.addEventListener("notificationclose", (event) => {
  event.waitUntil(syncBadge().catch(() => undefined));
});
