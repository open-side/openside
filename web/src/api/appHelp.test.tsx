import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { SettingsPage } from "../pages/SettingsPage";
import { manageTarget, WORKSPACE_SECTIONS } from "../pages/manageRoutes";
import { fakeClient, json } from "../test/fakeDaemon";

const root = resolve(__dirname, "../../..");
const help = readFileSync(resolve(root, "docs/specs/app-help.md"), "utf8");
const source = (path: string) => readFileSync(resolve(root, "web/src", path), "utf8");

afterEach(cleanup);

describe("static openside app help", () => {
  it("names every real route and no invented route", () => {
    const screenRoutes = [...source("App.tsx").matchAll(/<Route path="([^"*]+)"/g)].map((match) => `/${match[1]}`);
    const dialogRoutes = [...source("pages/manageRoutes.ts").matchAll(/path === "(\/[^" ]+)"/g)].flatMap((match) => match[1] ?? []);
    dialogRoutes.push(...WORKSPACE_SECTIONS.map((section) => section.path));
    for (const route of dialogRoutes) expect(manageTarget(route)).not.toBeNull();
    expect(source("App.tsx")).toContain('window.location.pathname === "/pair"');
    const routes = ["/", "/pair", ...screenRoutes, ...dialogRoutes];
    const documented = [...help.matchAll(/(?<![\w/])(\/(?:[a-z]+(?:\/:[a-z]+)?)?)(?=[\s):,;])/g)].map((match) => match[1]);
    expect([...new Set(documented)].sort()).toEqual([...new Set(routes)].sort());
    for (const section of WORKSPACE_SECTIONS) expect(help).toContain(`${section.title} (${section.path}`);
  });

  it("uses actual sidebar, History and phone connection menu labels", () => {
    const sidebar = source("components/Sidebar.tsx");
    const navigation = sidebar.slice(sidebar.indexOf('<nav className="manage-nav"'), sidebar.indexOf("</nav>"));
    const links = [...navigation.matchAll(/<Link to="(\/[^" ]*)"[\s\S]*?<span>([^<]*)<\/span>/g)];
    expect(links.map((match) => match[2])).toEqual(["History", "Workspace", "Settings"]);
    for (const [, route, label] of links) {
      expect(help).toContain(`${label} (${route})`);
      expect(help).toContain(label);
    }
    const history = source("pages/HistoryPage.tsx");
    for (const label of ["Earlier", "Archived"]) {
      expect(history).toContain(`label: "${label}"`);
      expect(help).toContain(label);
    }
    expect(source("remote/RemoteAccess.tsx")).toContain('title="Connect your phone"');
    expect(help).toContain("Connect your phone");
    expect(help).toContain("Home threads offer neither Delete conversation nor Clear.");
  });

  it("covers the rendered Settings entries and sections", () => {
    const { client } = fakeClient(() => json(200, { ok: true, api_version: "v1" }));
    const noop = vi.fn();
    const { container } = render(
      <ClientContext.Provider value={client}>
        <SettingsPage onModels={noop} onChannels={noop} onRemote={noop} onWorkspace={noop} onGuide={noop} onSignOut={noop} />
      </ClientContext.Provider>,
    );
    const entries = [...container.querySelectorAll(".workspace-entry strong")].map((entry) => entry.textContent);
    expect(entries).toEqual(["Models & accounts", "Channels", "Remote access", "Workspace", "Setup guide"]);
    for (const entry of entries) expect(help).toContain(entry);
    for (const heading of ["Connection", "Notifications", "Appearance"]) {
      expect(screen.getByRole("heading", { name: heading })).toBeInTheDocument();
      expect(help).toContain(heading);
    }
    expect(screen.getByRole("button", { name: "Sign out" })).toBeInTheDocument();
    expect(help).toContain("Sign out");
  });

  it("stays within the front-only help budget", () => {
    const prompt = readFileSync(resolve(root, "crates/opensided/src/prompt.rs"), "utf8");
    expect(prompt).not.toContain("app-help.md");
    expect(Buffer.byteLength(help)).toBeLessThanOrEqual(1500);
    expect(help).toContain("Possibly signed in is unverified");
  });
});
