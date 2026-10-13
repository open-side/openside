// RFC 6238 codes for a setup key the person pasted but chose not to keep: the key stays in
// this page for one computation and never reaches the daemon.

const BASE32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const HASHES: Record<string, "SHA-1" | "SHA-256" | "SHA-512"> = { SHA1: "SHA-1", SHA256: "SHA-256", SHA512: "SHA-512" };

export interface TotpParams {
  key: Uint8Array<ArrayBuffer>;
  hash: "SHA-1" | "SHA-256" | "SHA-512";
  digits: number;
  period: number;
}

function base32(text: string): Uint8Array<ArrayBuffer> | null {
  const clean = text.replace(/[\s-]/g, "").replace(/=+$/, "").toUpperCase();
  if (!clean) return null;
  const bytes = new Uint8Array(new ArrayBuffer(Math.floor((clean.length * 5) / 8)));
  let buffer = 0;
  let bits = 0;
  let index = 0;
  for (const char of clean) {
    const value = BASE32.indexOf(char);
    if (value < 0) return null;
    buffer = (buffer << 5) | value;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      bytes[index++] = (buffer >> bits) & 0xff;
    }
  }
  return index ? bytes : null;
}

/** An `otpauth://totp/…` link or a bare base32 setup key; `null` when it is neither. */
export function parseAuthenticator(input: string): TotpParams | null {
  const text = input.trim();
  if (!/^otpauth:/i.test(text)) {
    const key = base32(text);
    return key ? { key, hash: "SHA-1", digits: 6, period: 30 } : null;
  }
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    return null;
  }
  if (url.host.toLowerCase() !== "totp") return null;
  const hash = HASHES[(url.searchParams.get("algorithm") ?? "SHA1").toUpperCase().replace("-", "")];
  const digits = Number(url.searchParams.get("digits") ?? 6);
  const period = Number(url.searchParams.get("period") ?? 30);
  const key = base32(url.searchParams.get("secret") ?? "");
  if (!hash || !key || ![6, 7, 8].includes(digits) || !Number.isInteger(period) || period <= 0) return null;
  return { key, hash, digits, period };
}

export async function totpCode(params: TotpParams, now = Date.now()): Promise<string> {
  const counter = Math.floor(now / 1000 / params.period);
  const message = new DataView(new ArrayBuffer(8));
  message.setUint32(0, Math.floor(counter / 2 ** 32));
  message.setUint32(4, counter >>> 0);
  const key = await crypto.subtle.importKey("raw", params.key, { name: "HMAC", hash: params.hash }, false, ["sign"]);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, message.buffer));
  const offset = mac[mac.length - 1]! & 0x0f;
  const binary = ((mac[offset]! & 0x7f) << 24) | (mac[offset + 1]! << 16) | (mac[offset + 2]! << 8) | mac[offset + 3]!;
  return String(binary % 10 ** params.digits).padStart(params.digits, "0");
}
