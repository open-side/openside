import { describe, expect, it } from "vitest";
import { parseAuthenticator, totpCode } from "./totp";

// RFC 6238 appendix B: the ASCII key "12345678901234567890…" per hash, 8 digits.
const SHA1 = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
const SHA256 = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZA";

describe("totp", () => {
  it("matches the RFC 6238 test vectors", async () => {
    const sha1 = parseAuthenticator(`otpauth://totp/x?secret=${SHA1}&digits=8`)!;
    const sha256 = parseAuthenticator(`otpauth://totp/x?secret=${SHA256}&algorithm=SHA256&digits=8`)!;
    expect(await totpCode(sha1, 59_000)).toBe("94287082");
    expect(await totpCode(sha1, 1_111_111_109_000)).toBe("07081804");
    expect(await totpCode(sha256, 59_000)).toBe("46119246");
    expect(await totpCode(sha256, 20_000_000_000_000)).toBe("77737706");
  });

  it("reads a bare base32 key with spaces as SHA-1, 6 digits, 30 s", () => {
    expect(parseAuthenticator("jbsw y3dp ehpk 3pxp")).toMatchObject({ hash: "SHA-1", digits: 6, period: 30 });
  });

  it("refuses what is neither a key nor a TOTP link", () => {
    for (const value of ["", "not base32!", "otpauth://hotp/x?secret=JBSWY3DPEHPK3PXP", "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&algorithm=MD5"]) {
      expect(parseAuthenticator(value), value).toBeNull();
    }
  });
});
