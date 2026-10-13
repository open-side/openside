import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { agentSummary } from "./test/agents";
import { json } from "./test/fakeDaemon";

/** The daemon's live stream: open, nothing to replay. */
const idleStream = () => new Response(new ReadableStream({ start() {} }), { status: 200, headers: { "Content-Type": "text/event-stream" } });

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
  sessionStorage.clear();
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
      if (method === "GET" && url.pathname.endsWith("/events")) return route(method, url.pathname, body) ?? idleStream();
      return route(method, url.pathname, body) ?? json(404, { code: "not_found", message: "not found" });
    }),
  );
  return requests;
}

/** Answers every management screen with the smallest valid document. */
const EMPTY_ADMIN: Record<string, unknown> = {
  "/v1/sessions": { sessions: [{ id: "s1", title: "Hello", status: "idle", activity: "", created_at: "2026-10-12T00:00:00Z", updated_at: "2026-10-12T00:00:00Z", pinned: false, archived: false }] },
  "/v1/agents": { agents: [agentSummary("chief", "Juno", { home_session_id: "s1" })] },
  "/v1/skills": { skills: [{ name: "pdf-tools", description: "PDF helper", source: "user", enabled: true }], problems: [] },
  "/v1/routines": {
    routines: [
      { id: "r1", name: "Digest", schedule: { type: "interval", secs: 3600 }, action: { type: "new_session", prompt: "p" }, enabled: true, catch_up: false, created_at: "2026-10-12T00:00:00Z" },
    ],
  },
  "/v1/routines/suggestions": { suggestions: [] },
  "/v1/memory": { mode: "lexical_only", hits: [] },
  "/v1/memory/health": { mode: "lexical_only", count: 0 },
  "/v1/memory/model": {
    state: "idle",
    installed: false,
    model_dir: "/data/models/potion",
    source: { endpoint: "https://huggingface.co", repo: "minishlab/potion-base-8M", revision: "main" },
    files_done: 0,
    files_total: 0,
    bytes_done: 0,
    bytes_total: 0,
    restart_required: false,
  },
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
    expect(await screen.findByRole("link", { name: /^Juno/ })).toBeInTheDocument();
    const exchanges = requests.filter((r) => r.path === "/v1/pairing/exchange");
    expect(exchanges).toEqual([{ method: "POST", path: "/v1/pairing/exchange", auth: null, body: { code: "code-123" } }]);
    expect(localStorage.getItem("openside.token")).toBe("tok-7");
    expect(localStorage.getItem("openside.device")).toBe("d7");
    expect(window.location.pathname + window.location.hash).toBe("/agents/chief");
    expect(requests.find((r) => r.path === "/v1/agents")?.auth).toBe("Bearer tok-7");
  });

  it("explains an expired code and offers token entry", async () => {
    window.history.pushState({}, "", "/pair#old-code");
    stubDaemon(() => json(410, { code: "pairing_expired", message: "expired" }));
    render(<App />);
    expect(await screen.findByRole("alert")).toHaveTextContent("This pairing code has expired. Make a new one in openside on your computer (Settings → Remote access).");
    expect(window.location.hash).toBe("");
    expect(localStorage.getItem("openside.token")).toBeNull();
    expect(screen.getByRole("link", { name: "enter a device token instead" })).toHaveAttribute("href", "/");
  });
});

describe("management screens (PP-15: no feature flags)", () => {
  it("every section opens from the nav with its controls usable and no gating", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "admin-token");
    stubDaemon((method, path) => (method === "GET" && path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined));
    render(<App />);
    const nav = await screen.findByRole("navigation", { name: "Sections" });
    expect(await screen.findByRole("link", { name: /^Juno/ })).toBeInTheDocument();

    const checks: Record<string, () => Promise<HTMLElement>> = {
      Skills: () => screen.findByRole("switch", { name: /^Enable \// }),
      Routines: () => screen.findByRole("switch", { name: /^Enable / }),
      Memory: () => screen.findByPlaceholderText("Search memory"),
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
      // The people list stays underneath the dialog.
      expect(screen.getByRole("link", { name: /^Juno/, hidden: true })).toBeInTheDocument();
      await user.click(within(dialog).getByRole("button", { name: "Back to workspace" }));
    }
    await user.click(within(await screen.findByRole("dialog", { name: "Workspace" })).getByRole("button", { name: "Close" }));
    await vi.waitFor(() => expect(window.location.pathname).toBe("/agents/chief"));
  });

  it("deep links open their dialog over the assistant's thread, and Close returns to it", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "admin-token");
    stubDaemon((method, path) => (method === "GET" && path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined));
    window.history.pushState({}, "", "/vault");
    render(<App />);
    const vault = await screen.findByRole("dialog", { name: "Vault" });
    expect(await within(vault).findByLabelText("Use policy for bank")).toBeEnabled();
    await user.click(within(vault).getByRole("button", { name: "Close" }));
    await vi.waitFor(() => expect(window.location.pathname).toBe("/agents/chief"));
    await vi.waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await user.click(within(screen.getByRole("navigation", { name: "Sections" })).getByRole("link", { name: "Settings" }));
    const settings = await screen.findByRole("dialog", { name: "Settings" });
    await user.click(within(settings).getByRole("button", { name: /^Workspace/ }));
    expect(await screen.findByRole("dialog", { name: "Workspace" })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/workspace");
  });
});

describe("phone team drawer", () => {
  it("closes when the person already open is picked again", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "phone-token");
    stubDaemon((method, path) => (method === "GET" && path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined));
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Open menu" }));
    const drawer = await screen.findByRole("dialog", { name: "Team menu" });
    expect(window.location.pathname).toBe("/agents/chief");
    await user.click(within(drawer).getByRole("link", { name: /^Juno/ }));
    await vi.waitFor(() => expect(screen.queryByRole("dialog", { name: "Team menu" })).toBeNull());
    expect(window.location.pathname).toBe("/agents/chief");
    expect(screen.getByRole("textbox", { name: "Message" })).toBeInTheDocument();
  });
});

describe("opening `/`", () => {
  it("goes to the guide without the assistant's thread ever being the address in between", async () => {
    localStorage.setItem("openside.token", "admin-token");
    const visited: string[] = [];
    const replace = window.history.replaceState.bind(window.history);
    vi.spyOn(window.history, "replaceState").mockImplementation((state, unused, url) => {
      visited.push(String(url));
      replace(state, unused, url);
    });
    stubDaemon((method, path) => {
      if (method === "GET" && path === "/v1/onboarding") return json(200, { state: "pending" });
      return method === "GET" && path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined;
    });
    render(<App />);
    await vi.waitFor(() => expect(window.location.pathname).toBe("/welcome"));
    expect(visited.filter((url) => url.startsWith("/agents/"))).toEqual([]);
    vi.restoreAllMocks();
  });

  it("goes to the assistant once the guide is done", async () => {
    localStorage.setItem("openside.token", "admin-token");
    stubDaemon((method, path) => {
      if (method === "GET" && path === "/v1/onboarding") return json(200, { state: "completed" });
      return method === "GET" && path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined;
    });
    render(<App />);
    await vi.waitFor(() => expect(window.location.pathname).toBe("/agents/chief"));
  });
});

describe("owner sign-out", () => {
  it("clears the persistent token and the tab's local boot proof", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "owner-token");
    sessionStorage.setItem("openside.local-secret", "boot-proof");
    stubDaemon((method, path) => (method === "GET" && path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined));
    window.history.pushState({}, "", "/settings");
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Sign out" }));
    expect(await screen.findByLabelText("Device token")).toBeInTheDocument();
    expect(localStorage.getItem("openside.token")).toBeNull();
    expect(sessionStorage.getItem("openside.local-secret")).toBeNull();
  });
});
