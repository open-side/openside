/*
 * Messages between the web UI and the openside MV3 extension for desktop notifications. Shared by
 * both builds (the extension imports this file), so the two sides cannot drift.
 */

/**
 * The extension's manifest pins this public key, so its id is the same on every machine and the
 * page can address it with `chrome.runtime.sendMessage` without discovery. Only the public half
 * exists; nothing is ever signed with it.
 */
export const EXTENSION_ID = "jneddlbpgmlheplolfonafkbkbkjhcfj";
export const EXTENSION_KEY =
  "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAyu+JYqfYd7cUG1RmR/GPlNE6bIdempP10mWGtlwDMjYS5uLnTUmI4UH1xzHvvEtSKCGq/rczsenhT59VPa6M9S7CKN040XDIV72QhDSVRMMpG4evnmKi6F8gsoqwr5pyAjMbqsuwapkWiAYbQ6Qj3/ecXQDbSu9i0DEQHR8nadPZqhOOG/UB6pllj93yfW7MxJ1Kb0/A0icpQ9smruVHd7p7A6qYZEFiWA1UbaGbhO8Ca7ln+KgsPTnf1ckjg6h2Xg9hOCyH438nwppgdBrKcsuR1FC9uDAoZuMuBLych3QQ/C4ALB7BWIH8gKiA2V2ET9yqW0xNV0NkoJIuUU7SAQIDAQAB";

/** Page origins the manifest lets talk to the extension: the daemon UI on this computer. */
export const EXTENSION_PAGE_MATCHES = ["http://127.0.0.1/*", "http://localhost/*"];

export type DesktopRequest =
  | { type: "openside.desktop.status" }
  /** `code` is a one-time pairing code; omit it when the extension already holds a pairing for this origin. */
  | { type: "openside.desktop.enable"; code?: string }
  | { type: "openside.desktop.disable" }
  /** Pairs the extension for imports without changing the notification setting; `code` is needed unless it already holds an import pairing. */
  | { type: "openside.desktop.pair"; code?: string };

export type DesktopFailure = "needs_pairing" | "pairing_conflict" | "permission" | "exchange_failed" | "refused";

export type DesktopReply =
  | {
      ok: true;
      /** The extension can reach this daemon with its own device token. */
      paired: boolean;
      enabled: boolean;
      /** Device id of the token the extension minted for itself from a pairing code, so Disable can revoke it; null when it must keep the token for imports. */
      device: string | null;
      /** The extension holds an import secret for its current token. The secret itself never leaves the extension. */
      importing: boolean;
      /** Pair only: the extension's previous minted token, now replaced and safe for the page to revoke. */
      replaced?: string | null;
    }
  | { ok: false; reason: DesktopFailure; message: string };
