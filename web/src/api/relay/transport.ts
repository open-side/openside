/*
 * `fetch` and `WebSocket` replacements that reach the daemon through an openside relay.
 * One relay WebSocket carries one Noise session; every HTTP request and WebSocket gets its
 * own stream on it. The session is opened on first use and again after it drops (relay
 * restart, daemon reconnect, phone switching networks).
 */
import { exchange } from "./http1";
import { Session } from "./mux";
import { daemonIdFor, decodeKey, initiate } from "./noise";
import { RelayWebSocket } from "./ws";

/** How a relay-paired device reaches its daemon; stored next to the token. */
export interface RelayTarget {
  kind: "relay";
  /** Relay origin, e.g. `https://relay.example.com`. */
  relay: string;
  daemon_id: string;
  /** Daemon X25519 public key, base64url without padding. */
  daemon_key: string;
}

export interface RelayTransport {
  fetch: typeof fetch;
  webSocket: (url: string, protocols: string[]) => WebSocket;
  close(): void;
}

const HANDSHAKE_TIMEOUT_MS = 10_000;

function connectSession(target: RelayTarget, key: Uint8Array, signal: AbortSignal): Promise<Session> {
  const url = new URL(`/v1/relay/connect/${encodeURIComponent(target.daemon_id)}`, target.relay);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  const { promise, resolve, reject } = Promise.withResolvers<Session>();
  const socket = new WebSocket(url);
  socket.binaryType = "arraybuffer";
  let session: Session | null = null;
  let ended = false;
  let finishing = false;
  const cleanup = () => {
    clearTimeout(timer);
    signal.removeEventListener("abort", onAbort);
  };
  const fail = (error: unknown) => {
    if (ended) return;
    ended = true;
    cleanup();
    session?.close();
    socket.close();
    reject(error instanceof Error ? error : new Error("relay handshake failed"));
  };
  const onAbort = () => fail(new Error("relay transport closed"));
  const timer = setTimeout(() => fail(new Error("relay handshake timed out")), HANDSHAKE_TIMEOUT_MS);
  signal.addEventListener("abort", onAbort, { once: true });
  const handshake = initiate(target.daemon_id, key);
  void handshake.catch(fail);
  socket.onopen = () => {
    void handshake.then((h) => {
      if (!ended) socket.send(h.message as Uint8Array<ArrayBuffer>);
    }).catch(fail);
  };
  socket.onmessage = (event: MessageEvent) => {
    if (ended) return;
    if (!(event.data instanceof ArrayBuffer)) {
      fail(new Error("invalid relay message"));
      return;
    }
    const data = new Uint8Array(event.data);
    if (session) {
      session.receive(data);
      return;
    }
    if (finishing || data.length !== 48) {
      fail(new Error("unexpected relay handshake message"));
      return;
    }
    finishing = true;
    void handshake.then((h) => h.finish(data)).then((transport) => {
      if (ended) return;
      cleanup();
      session = new Session(transport, {
        send: (message) => socket.send(message as Uint8Array<ArrayBuffer>),
        close: () => socket.close(),
      });
      resolve(session);
    }).catch(fail);
  };
  socket.onerror = () => fail(new Error("relay connection failed"));
  socket.onclose = () => {
    if (session) {
      ended = true;
      session.close();
    } else {
      fail(new Error("the relay closed the connection (is the daemon online?)"));
    }
  };
  if (signal.aborted) onAbort();
  return promise;
}

export function relayTransport(target: RelayTarget): RelayTransport {
  const key = decodeKey(target.daemon_key);
  if (!key) throw new Error("invalid relay pairing data");
  // The routing id is derived from the key; a mismatch means a corrupted or forged link.
  const verified = daemonIdFor(key).then((id) => {
    if (id !== target.daemon_id) throw new Error("invalid relay pairing data");
  });
  const host = new URL(target.relay).host;
  let current: Promise<Session> | null = null;
  const lifetime = new AbortController();
  const session = (): Promise<Session> => {
    if (lifetime.signal.aborted) return Promise.reject(new Error("relay transport closed"));
    if (current) {
      const pending = current;
      return pending.then(
        (s) => (s.closed ? reconnect() : s),
        () => reconnect(),
      );
    }
    return reconnect();
  };
  const reconnect = (): Promise<Session> => {
    current = verified.then(() => connectSession(target, key, lifetime.signal));
    return current;
  };
  // A stale session can fail the first open after a network change; retry once on a fresh one.
  const openStream = async () => {
    const s = await session();
    try {
      return s.open();
    } catch {
      return (await reconnect()).open();
    }
  };

  const relayFetch: typeof fetch = async (input, init) => {
    const request = new Request(input instanceof URL ? input : new URL(String(input instanceof Request ? input.url : input), target.relay), init);
    const url = new URL(request.url);
    const body = ["GET", "HEAD"].includes(request.method) ? null : new Uint8Array(await request.arrayBuffer());
    let stream;
    try {
      stream = await openStream();
    } catch (error) {
      throw new TypeError(error instanceof Error ? error.message : "relay unavailable");
    }
    return exchange(
      stream,
      { method: request.method, target: url.pathname + url.search, host, headers: request.headers, body },
      init?.signal ?? request.signal,
    );
  };

  return {
    fetch: relayFetch,
    webSocket: (url, protocols) => new RelayWebSocket(url, protocols, openStream) as unknown as WebSocket,
    close() {
      lifetime.abort();
      void current?.then((s) => s.close(), () => undefined);
      current = null;
    },
  };
}
