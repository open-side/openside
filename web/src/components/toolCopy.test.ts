import { afterEach, describe, expect, it } from "vitest";
import type { TimelineItem } from "../state/session";
import { collectToolFacts, describeTool, groupSummary, hasPhrase, safeFavicon, technicalView, type ToolItem } from "./toolCopy";
import { visits } from "./ToolActivity";

/** Every tool the daemon registers for the agent (crates/opensided/src/{tools,tasks,subagents,skills}). */
const DAEMON_TOOLS = [
  "bash",
  "browser_act",
  "browser_close_tab",
  "browser_navigate",
  "browser_open_tab",
  "browser_pointer",
  "browser_repl",
  "browser_screenshot",
  "browser_snapshot",
  "browser_space_reset",
  "browser_tabs",
  "ask_user",
  "delegate",
  "edit",
  "memory_search",
  "memory_write",
  "read",
  "register_authenticator",
  "request_choice",
  "request_human_step",
  "request_login",
  "request_otp",
  "skill",
  "cancel_task",
  "create_task",
  "get_tasks",
  "send_message",
  "steer_task",
  "vault_login",
  "vault_otp",
  "vault_passkey",
  "wait_for_approval",
  "write",
];

let counter = 0;
const call = (name: string, args: unknown, done = true, result?: unknown, isError = false): ToolItem => ({
  type: "tool",
  id: `c${(counter += 1)}`,
  name,
  args,
  updates: [],
  done,
  ...(done ? { result, isError } : {}),
});

afterEach(() => localStorage.removeItem("openside.debug"));

describe("describeTool", () => {
  it("has a phrase for every daemon tool, in both tenses, with no identifiers", () => {
    for (const name of DAEMON_TOOLS) {
      expect(hasPhrase(name), name).toBe(true);
      for (const done of [true, false]) {
        const { title } = describeTool(call(name, { path: "src/a.rs", url: "https://acme.com/p", site: "acme.com", action: "click", name: "x" }, done));
        expect(title, name).toMatch(/^[A-Z]/);
        expect(title, name).not.toMatch(/_|tab_id|[0-9a-f]{12}/);
      }
    }
  });

  it("speaks past tense once done and present tense while running", () => {
    const args = { tab_id: "t", url: "https://naver.com/" };
    expect(describeTool(call("browser_navigate", args)).title).toBe("Opened naver.com");
    expect(describeTool(call("browser_navigate", args, false)).title).toBe("Opening naver.com");
    expect(describeTool(call("read", { path: "/repo/README.md" })).title).toBe("Read README.md");
    expect(describeTool(call("edit", { path: "src/lib.rs" }, false)).title).toBe("Editing lib.rs");
  });

  it("names the page, the clicked element and the filled field from what the page said", () => {
    const open = call("browser_open_tab", { url: "https://acme.com/pricing" }, true, { tab_id: "tab1" });
    const snapshot = call("browser_snapshot", { tab_id: "tab1" }, true, '[snapshot_id s1]\n[e1] RootWebArea "Pricing · Acme Cloud"\n [e2] button "Sign in"\n [e3] textbox "Email"');
    const click = call("browser_act", { tab_id: "tab1", target: "e2", action: "click" });
    const fill = call("browser_act", { tab_id: "tab1", target: "e3", action: "type", text: "me@acme.com" });
    const facts = collectToolFacts([open, snapshot, click, fill] as TimelineItem[]);
    expect(describeTool(open, facts.get(open.id))).toMatchObject({ title: "Opened Pricing · Acme Cloud", detail: "acme.com" });
    expect(describeTool(click, facts.get(click.id)).title).toBe("Clicked “Sign in”");
    expect(describeTool(fill, facts.get(fill.id)).title).toBe("Filled the email field");
    expect(describeTool(call("browser_snapshot", { tab_id: "tab1" })).title).toBe("Read the page");
  });

  it("never shows what was typed", () => {
    const type = call("browser_act", { tab_id: "t", target: "e3", action: "type", text: "hunter2-secret" });
    expect(describeTool(type).title).not.toContain("hunter2");
    expect(JSON.stringify(technicalView(type, type.args))).not.toContain("hunter2");
    const pointer = call("browser_pointer", { tab_id: "t", action: "type", x: 1, y: 2, text: "hunter2-secret" });
    expect(describeTool(pointer).title).toBe("Typed on the page");
    expect(JSON.stringify(technicalView(pointer, pointer.args))).not.toContain("hunter2");
  });

  it("gives tools nobody wrote copy for a sane generic sentence", () => {
    expect(describeTool(call("mcp__acme__fetch_invoice_list", {})).title).toBe("Used fetch invoice list");
    expect(describeTool(call("totally_new_thing", {}, false)).title).toBe("Using totally new thing");
    expect(describeTool(call("web_search", {})).title).toBe("Searched");
    expect(describeTool(call("???", {})).title).not.toMatch(/\?/);
  });
});

describe("group labels", () => {
  it("names the work, not the count", () => {
    const nav = call("browser_navigate", { tab_id: "t", url: "https://acme.com" });
    const snap = call("browser_snapshot", { tab_id: "t" });
    const reads = [call("read", { path: "a.md" }), call("read", { path: "b.md" })];
    const facts = collectToolFacts([nav, snap, ...reads] as TimelineItem[]);
    expect(groupSummary([nav, snap], facts)).toBe("Browsed acme.com · 2 steps");
    expect(groupSummary(reads, facts)).toBe("Read 2 files");
    expect(groupSummary([nav, snap, ...reads], facts)).toBe("Browsed acme.com and read 2 files");
    expect(groupSummary([nav, ...reads, call("bash", { command: "ls" }), call("memory_search", { query: "x" })], facts)).toMatch(/ \+2 more$/);
  });

  it("lets a running call name itself", () => {
    const running = call("browser_act", { tab_id: "t", target: "e1", action: "click" }, false);
    expect(groupSummary([call("read", { path: "a.md" }), running], new Map())).toBe("Clicking on the page");
  });
});

describe("page visits", () => {
  it("groups the steps after a navigation under it and starts a new visit per page", () => {
    const [a, aSnap, aClick, b, bSnap] = [
      call("browser_navigate", { tab_id: "t", url: "https://a.test" }),
      call("browser_snapshot", { tab_id: "t" }),
      call("browser_act", { tab_id: "t", target: "e1", action: "click" }),
      call("browser_navigate", { tab_id: "t", url: "https://b.test" }),
      call("browser_snapshot", { tab_id: "t" }),
    ] as const;
    const shell = call("bash", { command: "ls" });
    const result = visits([a, aSnap, aClick, b, bSnap, shell]);
    expect(result.map((visit) => [visit.head.id, visit.steps.map((step) => step.id)])).toEqual([
      [a.id, [aSnap.id, aClick.id]],
      [b.id, [bSnap.id]],
      [shell.id, []],
    ]);
  });
});

describe("favicons and technical details", () => {
  it("accepts only daemon-fetched data URLs", () => {
    expect(safeFavicon("data:image/png;base64,iVBORw0KGgo=")).not.toBeNull();
    expect(safeFavicon("https://evil.test/x.png")).toBeNull();
    expect(safeFavicon("data:image/svg+xml;base64,PHN2Zz4=")).toBeNull();
  });

  it("takes the favicon and title for a navigation from the daemon's browser state", () => {
    const nav = call("browser_navigate", { tab_id: "tab1", url: "https://acme.com/p" });
    const states = {
      "front:tab1": { tab_id: "tab1", url: "https://acme.com/p", host: "acme.com", title: "Plans", phase: "reading", page_kind: "other", facts: {}, seq: 3, favicon: "data:image/png;base64,AAAA" },
    };
    const facts = collectToolFacts([nav] as TimelineItem[], states as never);
    expect(facts.get(nav.id)).toMatchObject({ host: "acme.com", title: "Plans", favicon: "data:image/png;base64,AAAA" });
  });

  it("hides ids unless the debug flag is set", () => {
    const tool = call("browser_navigate", { tab_id: "70F7982D", url: "https://a.test" }, true, { ticket_id: "c752", status: "ok" });
    expect(technicalView(tool, tool.args)).toEqual({ url: "https://a.test" });
    expect(technicalView(tool, tool.result)).toEqual({ status: "ok" });
    localStorage.setItem("openside.debug", "1");
    expect(technicalView(tool, tool.args)).toEqual({ tab_id: "70F7982D", url: "https://a.test" });
  });
});
