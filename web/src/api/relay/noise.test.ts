import { describe, expect, it } from "vitest";
import vector from "./noise-vector.json";
import { daemonIdFor, decodeKey, initiate, type EphemeralKey } from "./noise";

const hex = (bytes: Uint8Array) => Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
const unhex = (text: string) => Uint8Array.from(text.match(/../g) ?? [], (pair) => parseInt(pair, 16));

async function fixedEphemeral(): Promise<EphemeralKey> {
  const privateKey = await crypto.subtle.importKey(
    "jwk",
    { kty: "OKP", crv: "X25519", d: vector.ephemeral_secret, x: vector.ephemeral_public },
    { name: "X25519" },
    true,
    ["deriveBits"],
  );
  return { privateKey, publicKey: decodeKey(vector.ephemeral_public)! };
}

describe("Noise NK initiator", () => {
  it("matches the snow vector byte for byte", async () => {
    const key = decodeKey(vector.daemon_key)!;
    expect(await daemonIdFor(key)).toBe(vector.daemon_id);
    const handshake = await initiate(vector.daemon_id, key, await fixedEphemeral());
    expect(hex(handshake.message)).toBe(vector.message1);
    const transport = await handshake.finish(unhex(vector.message2));
    for (const [i, ciphertext] of vector.from_daemon.entries()) {
      expect(new TextDecoder().decode(await transport.receive.decrypt(unhex(ciphertext)))).toBe(vector.from_daemon_plain[i]);
    }
    for (const [i, plain] of vector.from_device_plain.entries()) {
      expect(hex(await transport.send.encrypt(new TextEncoder().encode(plain)))).toBe(vector.from_device[i]);
    }
  });

  it("refuses a reply from a peer without the daemon key", async () => {
    const key = decodeKey(vector.daemon_key)!;
    const handshake = await initiate(vector.daemon_id, key, await fixedEphemeral());
    const forged = unhex(vector.message2);
    forged[40] = (forged[40] ?? 0) ^ 1;
    await expect(handshake.finish(forged)).rejects.toThrow();
    // The routing id is bound into the prologue: the same key under another id fails too.
    const other = await initiate("A".repeat(22), key, await fixedEphemeral());
    expect(hex(other.message)).not.toBe(vector.message1);
  });

  it("decodes only 32-byte base64url keys", () => {
    expect(decodeKey(vector.daemon_key)?.length).toBe(32);
    expect(decodeKey("short")).toBeNull();
    expect(decodeKey("!!!")).toBeNull();
  });
});
