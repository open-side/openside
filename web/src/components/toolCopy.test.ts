import { afterEach, describe, expect, it } from "vitest";
import type { TimelineItem } from "../state/session";
import { collectToolFacts, describeTool, formatDuration, hasPhrase, safeFavicon, spanSeconds, technicalView, workLabel, type ToolItem } from "./toolCopy";

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
  "browser_downloads",
  "ask_user",
  "computer_use_act",
  "computer_use_apps",
  "computer_use_connect",
  "computer_use_screenshot",
  "computer_use_stop",
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
  "webfetch",
  "websearch",
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

  it("shows web searches with their query and web reads with the page the daemon read", () => {
    const search = call("websearch", { query: "tide tables" });
    expect(describeTool(search)).toMatchObject({ title: "Searched the web", detail: "tide tables" });
    expect(describeTool(call("websearch", { query: "q" }, false)).title).toBe("Searching the web");
    const icon = "data:image/png;base64,iVBORw0KGgo=";
    const fetch = call("webfetch", { url: "https://tides.example/a" }, true, {
      status: "fetched",
      url: "https://tides.example/guide",
      title: "Tide guide",
      favicon_ref: { type: "data", data_url: icon },
    });
    const facts = collectToolFacts([fetch] as TimelineItem[]);
    expect(facts.get(fetch.id)?.favicon).toBe(icon);
    expect(describeTool(fetch, facts.get(fetch.id))).toMatchObject({ title: "Read Tide guide", detail: "tides.example" });
    expect(describeTool(call("webfetch", { url: "https://tides.example/a" }, false)).title).toBe("Reading tides.example");
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

describe("work duration", () => {
  it("spans two event times in whole seconds, floored", () => {
    expect(spanSeconds("2026-01-01T00:00:00Z", "2026-01-01T00:00:12.4Z")).toBe(12);
    expect(spanSeconds("2026-01-01T00:00:00Z", Date.parse("2026-01-01T00:00:04Z"))).toBe(4);
    expect(workLabel(12, false)).toBe("Worked for 12s");
    expect(workLabel(4, true)).toBe("Working for 4s");
  });

  it("never estimates: an untimed or sub-second span just says Worked or Working", () => {
    expect(spanSeconds(undefined, "2026-01-01T00:00:09Z")).toBeNull();
    expect(spanSeconds("2026-01-01T00:00:09Z", "2026-01-01T00:00:00Z")).toBeNull();
    expect(workLabel(null, false)).toBe("Worked");
    expect(workLabel(spanSeconds("2026-01-01T00:00:00Z", "2026-01-01T00:00:00.600Z"), false)).toBe("Worked");
    expect(workLabel(0, true)).toBe("Working");
    expect(workLabel(spanSeconds("2026-01-01T00:00:00Z", "2026-01-01T00:00:01.900Z"), false)).toBe("Worked for 1s");
  });

  it("reads minutes and hours the short way", () => {
    expect(formatDuration(45)).toBe("45s");
    expect(formatDuration(125)).toBe("2m 5s");
    expect(formatDuration(180)).toBe("3m");
    expect(formatDuration(3720)).toBe("1h 2m");
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
