import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { agentSummary } from "./test/agents";
import { json } from "./test/fakeDaemon";
import { LEGACY_SETTINGS_PATHS, settingsPath } from "./settings/nav";

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
  "/v1/agents": { agents: [agentSummary("chief", "openside", { home_session_id: "s1" })] },
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
  "/v1/memory/files": { files: [{ name: "MEMORY.md", view: "memory", editable: true, items: 0, updated_at: null }] },
  "/v1/memory/files/MEMORY.md": { name: "MEMORY.md", view: "memory", editable: true, updated_at: null, revision: 1, markdown: "# MEMORY\n" },
  "/v1/accounts": { sources: [], cached_for_ms: 0, truncated: false },
  "/v1/vault": { state: "unlocked", backend: "secret_service", reason: null, default_policy: "while_unlocked" },
  "/v1/vault/items": { available: true, default_policy: "while_unlocked", items: [{ id: "bank", origins: ["https://bank.example"], effective_policy: "while_unlocked", has_totp: false, passkey_rp_ids: [] }] },
  "/v1/vault/settings": {
    auto_lock: "one_week",
    biometric_unlock: false,
    biometrics: { available: false, mechanism: null, reason: "unsupported_platform" },
    autofill: { enabled: true, auto_submit: true, allow_insecure: false, excluded_domains: [] },
    default_policy: "while_unlocked",
    disable_in_incognito: true,
    recovery: { configured: false, current: false, created_at: null },
  },
  "/v1/devices": { devices: [{ device_id: "d1", name: "Phone", scopes: ["read"] }] },
  "/v1/sync": {
    configured: false,
    set_up: false,
    unlocked: false,
    records: 0,
    pending: 0,
    exhausted: false,
    settings: false,
    memory_tags: [],
    passphrase_source: "entered",
    last_run: null,
    location: null,
    password_source: null,
    webdav_username: null,
  },
  "/v1/pinned-tabs": { tabs: [] },
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
    expect(await within(await screen.findByRole("region", { name: "Chats" })).findByRole("link", { name: "Hello" })).toBeInTheDocument();
    const exchanges = requests.filter((r) => r.path === "/v1/pairing/exchange");
    expect(exchanges).toEqual([{ method: "POST", path: "/v1/pairing/exchange", auth: null, body: { code: "code-123" } }]);
    expect(localStorage.getItem("openside.token")).toBe("tok-7");
    expect(localStorage.getItem("openside.device")).toBe("d7");
    expect(window.location.pathname + window.location.hash).toBe("/");
    expect(requests.find((r) => r.path === "/v1/agents")?.auth).toBe("Bearer tok-7");
  });

  it("explains an expired code and offers token entry", async () => {
    window.history.pushState({}, "", "/pair#old-code");
    stubDaemon(() => json(410, { code: "pairing_expired", message: "expired" }));
    render(<App />);
    expect(await screen.findByRole("alert")).toHaveTextContent("This pairing code has expired. Make a new one in openside on your computer (Settings → Developers).");
    expect(window.location.hash).toBe("");
    expect(localStorage.getItem("openside.token")).toBeNull();
    expect(screen.getByRole("link", { name: "enter a device token instead" })).toHaveAttribute("href", "/");
  });
});

describe("Settings tab (PP-15: no feature flags)", () => {
  it("every workspace page opens from the Settings nav with its controls usable and no gating", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "admin-token");
    stubDaemon((method, path) => (method === "GET" && path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined));
    window.history.pushState({}, "", "/settings");
    render(<App />);
    await vi.waitFor(() => expect(window.location.pathname).toBe("/settings/general"));
    const nav = await screen.findByRole("navigation", { name: "Settings sections" });

    const checks: Record<string, () => Promise<HTMLElement>> = {
      "Plugins & MCPs": () => screen.findByRole("switch", { name: /^Enable \// }),
      Routines: () => screen.findByRole("switch", { name: /^Enable / }),
      Memory: async () => within(await screen.findByRole("region", { name: "MEMORY.md" })).findByRole("button", { name: "Edit" }),
      // Item policies live in the Vaults sheet, as in Aside's Data management section.
      Passwords: async () => {
        await user.click(await screen.findByRole("button", { name: /^Vaults/ }));
        const policy = await screen.findByLabelText("Use policy for bank");
        expect(policy).toBeEnabled();
        // Close the sheet so the next nav link is reachable.
        await user.keyboard("{Escape}");
        return policy;
      },
      Account: () => screen.findByRole("button", { name: /^Options for / }),
    };
    for (const [label, control] of Object.entries(checks)) {
      await user.click(within(nav).getByRole("link", { name: label }));
      expect(await screen.findByRole("heading", { level: 1, name: label })).toBeInTheDocument();
      expect(await control(), label).toBeEnabled();
      expect(within(screen.getByRole("main", { name: label })).queryAllByRole("alert").map((alert) => alert.textContent), label).toEqual([]);
      expect(document.body.textContent, label).not.toMatch(/upgrade|premium|feature flag|not available in this/i);
    }
  });

  // Each old dialog address lands on the Settings page that now holds what it used to show.
  const LEGACY: Record<string, string> = {
    "/models": "/settings/models",
    "/channels": "/settings/channels",
    "/remote": "/settings/developers",
    "/workspace": "/settings/general",
    "/team": "/settings/general",
    "/skills": "/settings/plugins",
    "/routines": "/settings/routines",
    "/memory": "/settings/memory",
    "/vault": "/settings/passwords",
    "/connections": "/settings/security",
    "/devices": "/settings/account",
    "/agents": "/settings/developers",
    "/sync": "/settings/account",
    "/policy": "/settings/agents",
    "/diagnostics": "/settings/general",
    "/config": "/settings/general",
  };

  it("covers every legacy address the app redirects", () => {
    expect(LEGACY_SETTINGS_PATHS.map(([from, page]) => [from, settingsPath(page)])).toEqual(Object.entries(LEGACY));
  });

  it.each(Object.entries(LEGACY))("sends %s to %s", async (from, to) => {
    localStorage.setItem("openside.token", "admin-token");
    stubDaemon((method, path) => (method === "GET" && path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined));
    window.history.pushState({}, "", from);
    render(<App />);
    await vi.waitFor(() => expect(window.location.pathname).toBe(to));
    expect(await screen.findByRole("navigation", { name: "Settings sections" })).toBeInTheDocument();
  });
});

describe("phone drawer", () => {
  it("closes when the page already open is picked again", async () => {
    const user = userEvent.setup();
    localStorage.setItem("openside.token", "phone-token");
    stubDaemon((method, path) => (method === "GET" && path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined));
    window.history.pushState({}, "", "/chats");
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Open menu" }));
    const drawer = await screen.findByRole("dialog", { name: "Menu" });
    await user.click(await within(within(drawer).getByRole("region", { name: "Chats" })).findByRole("link", { name: "Show all" }));
    await vi.waitFor(() => expect(screen.queryByRole("dialog", { name: "Menu" })).toBeNull());
    expect(window.location.pathname).toBe("/chats");
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

  it("shows Home once the guide is done", async () => {
    localStorage.setItem("openside.token", "admin-token");
    stubDaemon((method, path) => {
      if (method === "GET" && path === "/v1/onboarding") return json(200, { state: "completed" });
      return method === "GET" && path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined;
    });
    render(<App />);
    expect(await screen.findByRole("heading", { name: "What should we do?" })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/");
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

describe("mini popup window", () => {
  it("shows only the message box, without the sidebar column or toolbar", async () => {
    localStorage.setItem("openside.token", "owner-token");
    stubDaemon((method, path) => (method === "GET" && path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined));
    window.history.pushState({}, "", "/mini");
    const { container } = render(<App />);
    const mini = await screen.findByRole("main", { name: "Mini popup" });
    expect(within(mini).getByRole("textbox", { name: "Message" })).toBeInTheDocument();
    expect(container.querySelector(".shell")).toHaveAttribute("data-mini", "true");
    expect(container.querySelector(".shell-toolbar")).toBeNull();
  });
});

describe("`#search` from the extension's palette command", () => {
  it("opens the search palette and drops the hash", async () => {
    localStorage.setItem("openside.token", "owner-token");
    stubDaemon((method, path) => (method === "GET" && path in EMPTY_ADMIN ? json(200, EMPTY_ADMIN[path]) : undefined));
    window.history.pushState({}, "", "/chats?view=all#search");
    render(<App />);
    expect(await screen.findByRole("dialog", { name: "Search" })).toBeInTheDocument();
    await vi.waitFor(() => expect(window.location.hash).toBe(""));
    expect(window.location.pathname + window.location.search).toBe("/chats?view=all");
  });
});
