import { describe, expect, it } from "vitest";
import { exchange } from "./http1";
import { DATA, FIN, INITIAL_WINDOW, OPEN, RST, Session, WINDOW } from "./mux";
import { CipherState } from "./noise";
import { RelayWebSocket } from "./ws";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

interface Frame {
  kind: number;
  id: number;
  payload: Uint8Array;
}

/**
 * The daemon end of a Noise session, in memory: decrypts the device's frames in order and
 * sends frames back through the device's session, so mux, HTTP and WebSocket code run
 * against the same framing the Rust daemon speaks.
 */
async function harness(onFrame: (frame: Frame, daemon: Daemon) => void) {
  const k1 = crypto.getRandomValues(new Uint8Array(32));
  const k2 = crypto.getRandomValues(new Uint8Array(32));
  const daemonReceive = await CipherState.create(k1);
  const daemonSend = await CipherState.create(k2);
  let inbound = Promise.resolve();
  let outbound = Promise.resolve();
  const frames: Frame[] = [];
  const daemon: Daemon = {
    frames,
    send(kind, id, payload = new Uint8Array()) {
      const plain = new Uint8Array(5 + payload.length);
      plain[0] = kind;
      new DataView(plain.buffer).setUint32(1, id, false);
      plain.set(payload, 5);
      outbound = outbound.then(async () => session.receive(await daemonSend.encrypt(plain)));
    },
  };
  const session: Session = new Session(
    { send: await CipherState.create(k1), receive: await CipherState.create(k2) },
    {
      send(message) {
        inbound = inbound.then(async () => {
          const plain = await daemonReceive.decrypt(message);
          const frame = { kind: plain[0]!, id: new DataView(plain.buffer).getUint32(1, false), payload: plain.slice(5) };
          frames.push(frame);
          onFrame(frame, daemon);
        });
      },
      close() {},
    },
  );
  return { session, daemon };
}

interface Daemon {
  frames: Frame[];
  send(kind: number, id: number, payload?: Uint8Array): void;
}

/** Collects each stream's request bytes and calls `respond` once the head is complete. */
function http(respond: (request: string, id: number, daemon: Daemon) => void) {
  const requests = new Map<number, string>();
  return (frame: Frame, daemon: Daemon) => {
    if (frame.kind === OPEN) requests.set(frame.id, "");
    if (frame.kind !== DATA) return;
    const text = (requests.get(frame.id) ?? "") + decoder.decode(frame.payload);
    requests.set(frame.id, text);
    if (text.includes("\r\n\r\n") && !text.startsWith("DONE")) {
      requests.set(frame.id, "DONE" + text);
      respond(text, frame.id, daemon);
    }
  };
}

describe("relay HTTP over mux streams", () => {
  it("sends Host, token and Connection: close and streams a chunked SSE body", async () => {
    let seen = "";
    const { session } = await harness(
      http((request, id, daemon) => {
        seen = request;
        daemon.send(DATA, id, encoder.encode("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n"));
        daemon.send(DATA, id, encoder.encode("f\r\nid: 1\ndata: a\n\n\r\n"));
        daemon.send(DATA, id, encoder.encode("f\r\nid: 2\ndata: b\n\n\r\n0\r\n\r\n"));
        daemon.send(FIN, id);
        // The daemon drops its stream right after: the response must survive the reset.
        daemon.send(RST, id);
      }),
    );
    const response = await exchange(session.open(), {
      method: "GET",
      target: "/v1/sessions/s1/events",
      host: "relay.example.test",
      headers: new Headers({ Authorization: "Bearer tok", "Last-Event-ID": "0" }),
      body: null,
    });
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("text/event-stream");
    expect(await response.text()).toBe("id: 1\ndata: a\n\nid: 2\ndata: b\n\n");
    expect(seen).toMatch(/^GET \/v1\/sessions\/s1\/events HTTP\/1\.1\r\nHost: relay\.example\.test\r\n/);
    expect(seen).toContain("authorization: Bearer tok\r\n");
    expect(seen).toContain("Connection: close\r\n");
  });

  it("respects the credit window and grants more as the body is read", async () => {
    const size = INITIAL_WINDOW + 50_000;
    let pending: (() => void) | null = null;
    const { session, daemon } = await harness((frame, d) => {
      if (frame.kind === WINDOW) pending?.();
      http((_request, id) => {
        const head = encoder.encode(`HTTP/1.1 200 OK\r\ncontent-length: ${size}\r\n\r\n`);
        // Fill exactly the initial window, then wait for a WINDOW grant before the rest.
        const body = new Uint8Array(size).fill(7);
        const first = INITIAL_WINDOW - head.length;
        d.send(DATA, id, head);
        for (let at = 0; at < first; at += 16_384) d.send(DATA, id, body.subarray(at, Math.min(first, at + 16_384)));
        pending = () => {
          pending = null;
          for (let at = first; at < size; at += 16_384) d.send(DATA, id, body.subarray(at, Math.min(size, at + 16_384)));
          d.send(FIN, id);
        };
      })(frame, d);
    });
    const response = await exchange(session.open(), {
      method: "GET",
      target: "/big",
      host: "r",
      headers: new Headers(),
      body: null,
    });
    const body = new Uint8Array(await response.arrayBuffer());
    expect(body.length).toBe(size);
    expect(daemon.frames.some((f) => f.kind === WINDOW)).toBe(true);
  });

  it("splits request bodies into frames no larger than 16 KiB", async () => {
    const { session, daemon } = await harness(
      http((request, id, d) => {
        if (!request.includes("Content-Length: 40000")) return;
        d.send(DATA, id, encoder.encode("HTTP/1.1 204 No Content\r\n\r\n"));
        d.send(FIN, id);
      }),
    );
    const response = await exchange(session.open(), {
      method: "POST",
      target: "/v1/x",
      host: "r",
      headers: new Headers({ "Content-Type": "application/json" }),
      body: new Uint8Array(40_000),
    });
    expect(response.status).toBe(204);
    expect(Math.max(...daemon.frames.filter((f) => f.kind === DATA).map((f) => f.payload.length))).toBeLessThanOrEqual(16_384);
  });

  it("fails pending streams when the session ends", async () => {
    const { session } = await harness(() => {});
    const pending = exchange(session.open(), { method: "GET", target: "/", host: "r", headers: new Headers(), body: null });
    session.close();
    await expect(pending).rejects.toThrow(TypeError);
  });
});

describe("relay WebSocket", () => {
  it("upgrades on a stream, receives text frames and sends masked frames", async () => {
    const sent = Promise.withResolvers<string>();
    const { session } = await harness(
      http((request, id, d) => {
        const key = /sec-websocket-key: (.+)\r\n/i.exec(request)![1]!;
        void crypto.subtle
          .digest("SHA-1", encoder.encode(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))
          .then((digest) => {
            const accept = btoa(String.fromCharCode(...new Uint8Array(digest)));
            expect(request).toContain("Connection: Upgrade\r\n");
            expect(request).toContain("origin: https://relay.example.test\r\n");
            expect(request).toContain("sec-websocket-protocol: openside.v1, openside.bearer.tok\r\n");
            d.send(
              DATA,
              id,
              encoder.encode(
                `HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: upgrade\r\nsec-websocket-accept: ${accept}\r\nsec-websocket-protocol: openside.v1\r\n\r\n`,
              ),
            );
            const text = encoder.encode('{"type":"frame"}');
            d.send(DATA, id, new Uint8Array([0x81, text.length, ...text]));
          });
      }),
    );
    // Device → daemon frames after the upgrade arrive as raw DATA: unmask the first one.
    const socket = new RelayWebSocket("wss://relay.example.test/v1/tickets/t1/takeover", ["openside.v1", "openside.bearer.tok"], async () => {
      const stream = session.open();
      const write = stream.write.bind(stream);
      let upgraded = false;
      stream.write = async (data: Uint8Array) => {
        if (upgraded && data[0] === 0x81) {
          const length = data[1]! & 0x7f;
          const mask = data.subarray(2, 6);
          sent.resolve(decoder.decode(data.subarray(6, 6 + length).map((b, i) => b ^ mask[i % 4]!)));
        }
        upgraded = true;
        return write(data);
      };
      return stream;
    });
    const received = Promise.withResolvers<string>();
    socket.onmessage = (event) => received.resolve(String(event.data));
    const message = await received.promise;
    expect(message).toBe('{"type":"frame"}');
    expect(socket.readyState).toBe(1);
    expect(socket.protocol).toBe("openside.v1");
    socket.send('{"type":"key"}');
    expect(await sent.promise).toBe('{"type":"key"}');
  });
});
