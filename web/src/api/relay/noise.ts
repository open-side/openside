/*
 * Noise_NK_25519_AESGCM_SHA256 initiator, interoperable with the daemon's `snow` responder
 * (crates/openside-relay/src/noise.rs). Honest client code pins the daemon key from pairing,
 * excluding a forwarding relay from plaintext access. The browser must still trust the
 * operator distributing this application; that operator can change the client code.
 * Only WebCrypto is used: X25519, AES-GCM and HKDF-SHA256 (empty HKDF info matches Noise).
 */

export const PROTOCOL = "Noise_NK_25519_AESGCM_SHA256";
export const PROLOGUE = "openside-relay/1\n";
export const MAX_MESSAGE = 65535;
export const TAG_LEN = 16;

const subtle = (): SubtleCrypto => globalThis.crypto.subtle;
const encoder = new TextEncoder();

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

async function sha256(data: Uint8Array): Promise<Uint8Array> {
  return new Uint8Array(await subtle().digest("SHA-256", data as BufferSource));
}

async function hkdf2(chainingKey: Uint8Array, input: Uint8Array): Promise<[Uint8Array, Uint8Array]> {
  const key = await subtle().importKey("raw", input as BufferSource, "HKDF", false, ["deriveBits"]);
  const bits = new Uint8Array(
    await subtle().deriveBits(
      { name: "HKDF", hash: "SHA-256", salt: chainingKey as BufferSource, info: new Uint8Array() },
      key,
      512,
    ),
  );
  return [bits.slice(0, 32), bits.slice(32)];
}

function nonce(counter: bigint): Uint8Array {
  if (counter < 0n || counter >= (1n << 64n) - 1n) throw new Error("Noise nonce exhausted");
  const out = new Uint8Array(12);
  new DataView(out.buffer).setBigUint64(4, counter, false);
  return out;
}

/** One direction of the transport: AES-256-GCM with Noise's 64-bit big-endian counter nonce. */
export class CipherState {
  private counter = 0n;
  private constructor(private readonly key: CryptoKey) {}

  static async create(raw: Uint8Array): Promise<CipherState> {
    const key = await subtle().importKey("raw", raw as BufferSource, "AES-GCM", false, ["encrypt", "decrypt"]);
    return new CipherState(key);
  }

  /** The nonce is taken synchronously, so callers must await messages in send order. */
  async encrypt(plaintext: Uint8Array, ad: Uint8Array = new Uint8Array()): Promise<Uint8Array> {
    const iv = nonce(this.counter);
    this.counter++;
    return new Uint8Array(
      await subtle().encrypt({ name: "AES-GCM", iv: iv as BufferSource, additionalData: ad as BufferSource }, this.key, plaintext as BufferSource),
    );
  }

  async decrypt(ciphertext: Uint8Array, ad: Uint8Array = new Uint8Array()): Promise<Uint8Array> {
    const iv = nonce(this.counter);
    this.counter++;
    return new Uint8Array(
      await subtle().decrypt({ name: "AES-GCM", iv: iv as BufferSource, additionalData: ad as BufferSource }, this.key, ciphertext as BufferSource),
    );
  }
}

export interface Transport {
  send: CipherState;
  receive: CipherState;
}

export interface Handshake {
  /** First handshake message (`-> e, es`), sent as one binary WebSocket message. */
  message: Uint8Array;
  /** Completes the handshake with the daemon's reply (`<- e, ee`). */
  finish(reply: Uint8Array): Promise<Transport>;
}

export interface EphemeralKey {
  privateKey: CryptoKey;
  publicKey: Uint8Array;
}

export async function generateEphemeral(): Promise<EphemeralKey> {
  const pair = (await subtle().generateKey({ name: "X25519" }, true, ["deriveBits"])) as CryptoKeyPair;
  return { privateKey: pair.privateKey, publicKey: new Uint8Array(await subtle().exportKey("raw", pair.publicKey)) };
}

async function dh(privateKey: CryptoKey, publicKey: Uint8Array): Promise<Uint8Array> {
  const peer = await subtle().importKey("raw", publicKey as BufferSource, { name: "X25519" }, false, []);
  return new Uint8Array(await subtle().deriveBits({ name: "X25519", public: peer }, privateKey, 256));
}

/** Start a handshake with the daemon whose routing id and static key came from the QR. */
export async function initiate(daemonId: string, daemonKey: Uint8Array, ephemeral?: EphemeralKey): Promise<Handshake> {
  if (daemonKey.length !== 32) throw new Error("daemon key must be 32 bytes");
  const name = encoder.encode(PROTOCOL);
  // Protocol names of at most 32 bytes are zero-padded instead of hashed.
  let h = name.length <= 32 ? concat(name, new Uint8Array(32 - name.length)) : await sha256(name);
  let ck = h;
  const mixHash = async (data: Uint8Array) => {
    h = await sha256(concat(h, data));
  };
  await mixHash(encoder.encode(PROLOGUE + daemonId));
  await mixHash(daemonKey);

  const e = ephemeral ?? (await generateEphemeral());
  await mixHash(e.publicKey);
  const [nextCk, k] = await hkdf2(ck, await dh(e.privateKey, daemonKey));
  ck = nextCk;
  const sendKey = await CipherState.create(k);
  const tag = await sendKey.encrypt(new Uint8Array(), h);
  await mixHash(tag);
  const message = concat(e.publicKey, tag);

  return {
    message,
    async finish(reply: Uint8Array): Promise<Transport> {
      if (reply.length !== 32 + TAG_LEN) throw new Error("unexpected handshake reply");
      const re = reply.slice(0, 32);
      await mixHash(re);
      let k2: Uint8Array;
      [ck, k2] = await hkdf2(ck, await dh(e.privateKey, re));
      const receiveKey = await CipherState.create(k2);
      // Throws when the peer does not hold the daemon key: the empty payload fails to verify.
      await receiveKey.decrypt(reply.slice(32), h);
      const [initiatorKey, responderKey] = await hkdf2(ck, new Uint8Array());
      return { send: await CipherState.create(initiatorKey), receive: await CipherState.create(responderKey) };
    },
  };
}

export function decodeKey(text: string): Uint8Array | null {
  try {
    const base64 = text.replace(/-/g, "+").replace(/_/g, "/");
    const binary = atob(base64 + "=".repeat((4 - (base64.length % 4)) % 4));
    const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
    return bytes.length === 32 ? bytes : null;
  } catch {
    return null;
  }
}

/** `daemon_id` = base64url-nopad(SHA-256(public key)[..16]), as on the daemon. */
export async function daemonIdFor(key: Uint8Array): Promise<string> {
  const digest = (await sha256(key)).slice(0, 16);
  return btoa(String.fromCharCode(...digest)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
