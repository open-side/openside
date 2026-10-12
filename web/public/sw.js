// Offline shell only: the API (/v1/*) is never cached or intercepted, so session
// data and tokens stay out of Cache Storage.
const CACHE = "openside-shell-v3";
// Unhashed files that change with a build: fetched network-first, cached for offline.
const FRESH = ["/", "/theme-boot.js"];
const SHELL = [...FRESH, "/manifest.webmanifest", "/icons/icon.svg", "/icons/icon-192.png", "/icons/icon-512.png"];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll(SHELL))
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

self.addEventListener("fetch", (event) => {
  const request = event.request;
  const url = new URL(request.url);
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

  if (url.pathname.startsWith("/assets/") || SHELL.includes(url.pathname)) {
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

self.addEventListener("push", (event) => {
  let message = {};
  try {
    message = event.data ? event.data.json() : {};
  } catch {
    message = {};
  }
  const url = sameOriginPath(message.url);
  event.waitUntil(
    self.registration.showNotification(message.title || "openside", {
      body: message.body || "",
      // One notification per ticket or session: a newer push replaces the older one.
      tag: message.ticket_id || message.session_id || "openside",
      icon: "/icons/icon-192.png",
      data: { url, kind: message.kind, session_id: message.session_id, ticket_id: message.ticket_id },
    }),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const url = sameOriginPath(event.notification.data && event.notification.data.url);
  event.waitUntil(
    self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((windows) => {
      const open = windows.find((client) => new URL(client.url).origin === self.location.origin);
      if (open) return open.focus().then((client) => (client.navigate ? client.navigate(url) : client));
      return self.clients.openWindow(url);
    }),
  );
});
