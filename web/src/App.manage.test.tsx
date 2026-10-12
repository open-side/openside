import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { json } from "./test/fakeDaemon";

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
  window.history.pushState({}, "", "/");
});

type Route = (method: string, path: string, body: unknown) => Response | undefined;

function stubDaemon(route: Route) {
  const requests: { method: string; path: string; auth: string | null; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), "http://daemon.test");
      const method = init?.method ?? "GET";
      const body = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
      const auth = new Headers(init?.headers).get("Authorization");
      requests.push({ method, path: url.pathname + url.search, auth, body });
      return route(method, url.pathname, body) ?? json(404, { code: "not_found", message: "not found" });
    }),
  );
  return requests;
}

/** Answers every management screen with the smallest valid document. */
const EMPTY_ADMIN: Record<string, unknown> = {
  "/v1/sessions": { sessions: [{ id: "s1", title: "Hello", status: "idle", created_at: "2026-10-12T00:00:00Z", updated_at: "2026-10-12T00:00:00Z", pinned: false, archived: false }] },
  "/v1/skills": { skills: [{ name: "pdf-tools", description: "PDF helper", source: "user", enabled: true }], problems: [] },
  "/v1/routines": {
    routines: [
      { id: "r1", name: "Digest", schedule: { type: "interval", secs: 3600 }, action: { type: "new_session", prompt: "p" }, enabled: true, catch_up: false, created_at: "2026-10-12T00:00:00Z" },
    ],
  },
  "/v1/routines/suggestions": { suggestions: [] },
  "/v1/memory": { mode: "lexical_only", hits: [] },
  "/v1/memory/health": { mode: "lexical_only", count: 0 },
  "/v1/accounts": { sources: [], cached_for_ms: 0, truncated: false },
  "/v1/vault": { state: "unlocked", backend: "secret_service", reason: null, default_policy: "while_unlocked" },
  "/v1/vault/items": { available: true, default_policy: "while_unlocked", items: [{ id: "bank", origins: ["https://bank.example"], effective_policy: "while_unlocked", has_totp: false, passkey_rp_ids: [] }] },
  "/v1/vault/audit": { records: [] },
  "/v1/devices": { devices: [{ device_id: "d1", name: "Phone", scopes: ["read"] }] },
  "/v1/settings/config": {
    config: { tickets: { confirm_ttl_secs: 600 } },
    file_only: ["credentials"],
    restart_required: false,
    schema: { type: "object", properties: { tickets: { type: "object", properties: { confirm_ttl_secs: { type: "integer", minimum: 1 } } } } },
    remote_editable: ["tickets."],
    remote: false,
  },
};

describe("/pair deep link", () => {
  it("exchanges the one-time code once, stores the device token and opens the app", async () => {
    window.history.pushState({}, "", "/pair#code-123");
    const requests = stubDaemon((method, path) => {
      if (method === "POST" && path === "/v1/pairing/exchange") return json(201, { device_id: "d7", token: "tok-7", scopes: ["read", "approve"] });
      return path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined;
    });
    render(
      <StrictMode>
        <App />
      </StrictMode>,
    );
    expect(await screen.findByText("Hello")).toBeInTheDocument();
    const exchanges = requests.filter((r) => r.path === "/v1/pairing/exchange");
    expect(exchanges).toEqual([{ method: "POST", path: "/v1/pairing/exchange", auth: null, body: { code: "code-123" } }]);
    expect(localStorage.getItem("openside.token")).toBe("tok-7");
    expect(localStorage.getItem("openside.device")).toBe("d7");
    expect(window.location.pathname + window.location.hash).toBe("/");
    expect(requests.find((r) => r.path.startsWith("/v1/sessions"))?.auth).toBe("Bearer tok-7");
  });

  it("explains an expired code and offers token entry", async () => {
    window.history.pushState({}, "", "/pair#old-code");
    stubDaemon(() => json(410, { code: "pairing_expired", message: "expired" }));
    render(<App />);
    expect(await screen.findByRole("alert")).toHaveTextContent("This pairing code has expired. Create a new one with `openside pair`.");
    expect(window.location.hash).toBe("");
    expect(localStorage.getItem("openside.token")).toBeNull();
    expect(screen.getByRole("link", { name: "Enter a device token instead" })).toHaveAttribute("href", "/");
  });
});

describe("management screens (PP-15: no feature flags)", () => {
  it("every section opens from the nav with its controls usable and no gating", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "admin-token");
    stubDaemon((method, path) => (method === "GET" && path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined));
    render(<App />);
    const nav = await screen.findByRole("navigation", { name: "Sections" });
    expect(await screen.findByText("Hello")).toBeInTheDocument();

    const checks: Record<string, () => Promise<HTMLElement>> = {
      Skills: () => screen.findByRole("button", { name: "Disable" }),
      Routines: () => screen.findByRole("button", { name: "Run now" }),
      Memory: () => screen.findByPlaceholderText("Search memory"),
      Accounts: () => screen.findByRole("button", { name: "Refresh" }),
      Vault: () => screen.findByLabelText("Use policy for bank"),
      Devices: () => screen.findByRole("button", { name: "Revoke" }),
      Configuration: () => screen.findByRole("spinbutton"),
    };
    await user.click(within(nav).getByRole("link", { name: "Workspace" }));
    for (const [label, control] of Object.entries(checks)) {
      const workspace = await screen.findByRole("dialog", { name: "Workspace" });
      await user.click(within(workspace).getByRole("link", { name: new RegExp(`^${label}`) }));
      const dialog = await screen.findByRole("dialog", { name: label });
      expect(await control(), label).toBeEnabled();
      expect(screen.queryByRole("alert"), label).toBeNull();
      expect(document.body.textContent, label).not.toMatch(/upgrade|premium|feature flag|not available in this/i);
      // The conversation list stays underneath the dialog.
      expect(screen.getByText("Hello")).toBeInTheDocument();
      await user.click(within(dialog).getByRole("button", { name: "Back to workspace" }));
    }
    await user.click(within(await screen.findByRole("dialog", { name: "Workspace" })).getByRole("button", { name: "Close" }));
    await vi.waitFor(() => expect(window.location.pathname).toBe("/"));
  });

  it("deep links open their dialog over a new chat, and Close returns to it", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "admin-token");
    stubDaemon((method, path) => (method === "GET" && path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined));
    window.history.pushState({}, "", "/vault");
    render(<App />);
    const vault = await screen.findByRole("dialog", { name: "Vault" });
    expect(await within(vault).findByLabelText("Use policy for bank")).toBeEnabled();
    await user.click(within(vault).getByRole("button", { name: "Close" }));
    await vi.waitFor(() => expect(window.location.pathname).toBe("/"));
    await vi.waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await user.click(within(screen.getByRole("navigation", { name: "Sections" })).getByRole("link", { name: "Settings" }));
    const settings = await screen.findByRole("dialog", { name: "Settings" });
    await user.click(within(settings).getByRole("button", { name: /^Workspace/ }));
    expect(await screen.findByRole("dialog", { name: "Workspace" })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/workspace");
  });
});
