import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { t, type MessageKey } from "../i18n";
import { SETTINGS_NAV } from "../settings/nav";

const root = resolve(__dirname, "../../..");
const help = readFileSync(resolve(root, "docs/specs/app-help.md"), "utf8");
const source = (path: string) => readFileSync(resolve(root, "web/src", path), "utf8");

describe("static openside app help", () => {
  it("names every real route and no invented route", () => {
    const screenRoutes = [...source("App.tsx").matchAll(/<Route path="([^"*]+)"/g)].map((match) => `/${match[1]}`);
    expect(source("App.tsx")).toContain('window.location.pathname === "/pair"');
    const routes = ["/", "/pair", ...screenRoutes];
    const documented = [...help.matchAll(/(?<![\w/])(\/(?:[a-z]+(?:\/:[a-z]+)?)?)(?=[\s):,;])/g)].map((match) => match[1]);
    expect([...new Set(documented)].sort()).toEqual([...new Set(routes)].sort());
  });

  it("uses actual Chats filter and phone connection labels", () => {
    const chats = source("pages/ChatsPage.tsx");
    const filters = [...chats.matchAll(/"(shell\.chats\.show[A-Za-z]+)"/g)].map(([, key]) => t(key as MessageKey));
    expect(filters).toEqual(["All chats", "Unread", "Pinned", "Archived"]);
    for (const label of filters) expect(help).toContain(label);
    expect(source("remote/RemoteAccess.tsx")).toContain('title={t("connections.remote.connectPhone")}');
    expect(t("connections.remote.connectPhone")).toBe("Connect your phone");
    expect(help).toContain("Connect your phone");
    expect(help).toContain("Home threads offer neither Delete conversation nor Clear.");
  });

  it("names every Settings group and page", () => {
    for (const group of SETTINGS_NAV) {
      expect(help).toContain(`${t(group.heading)}:`);
      for (const page of group.pages) {
        expect(help).toContain(t(page.title));
        for (const child of page.children ?? []) expect(help).toContain(t(child.title));
      }
    }
    expect(help).toContain("Sign out");
  });

  it("stays within the front-only help budget", () => {
    const prompt = readFileSync(resolve(root, "crates/opensided/src/prompt.rs"), "utf8");
    expect(prompt).not.toContain("app-help.md");
    expect(Buffer.byteLength(help)).toBeLessThanOrEqual(1500);
    expect(help).toContain("Possibly signed in is unverified");
  });
});
