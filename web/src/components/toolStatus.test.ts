import { describe, expect, it } from "vitest";
import type { TimelineItem } from "../state/session";
import { hostOf, lastHost, outcomeNote, statusPhrase, toolPhrase } from "./toolStatus";

type ToolItem = Extract<TimelineItem, { type: "tool" }>;

const tool = (name: string, args: unknown, done = false, result?: unknown): ToolItem => ({
  type: "tool",
  id: name,
  name,
  args,
  updates: [],
  done,
  ...(done ? { result } : {}),
});

describe("tool status phrases", () => {
  it("names the site the browser acts on", () => {
    expect(hostOf("https://www.example.com/a")).toBe("example.com");
    expect(hostOf("about:blank")).toBeNull();
    expect(toolPhrase(tool("browser_navigate", { url: "https://nid.naver.com/login" }), null)).toBe("Opening nid.naver.com");
    expect(toolPhrase(tool("browser_act", { action: "click", target: "e1" }), "nid.naver.com")).toBe("Clicking on nid.naver.com");
    expect(toolPhrase(tool("browser_act", { action: "type", target: "e1" }), null)).toBe("Typing on the page");
    expect(toolPhrase(tool("browser_snapshot", { tab_id: "a" }), "x.com")).toBe("Reading a page");
  });

  it("describes files, commands and requests for the person", () => {
    expect(toolPhrase(tool("read", { path: "src/lib.rs" }), null)).toBe("Reading lib.rs");
    expect(toolPhrase(tool("edit", { path: "README.md" }), null)).toBe("Editing README.md");
    expect(toolPhrase(tool("bash", { command: "ls" }), null)).toBe("Running a command");
    expect(toolPhrase(tool("request_otp", {}), null)).toBe("Waiting for you");
  });

  it("follows the latest url and falls back once calls have returned", () => {
    const items = [
      tool("browser_navigate", { url: "https://a.com" }, true, { url: "https://b.com/next" }),
      tool("browser_snapshot", { tab_id: "t" }, true, "…"),
    ];
    expect(lastHost(items)).toBe("b.com");
    expect(statusPhrase(items, "b.com")).toBe("Reading a page");
    expect(statusPhrase([tool("read", { path: "a" }, true, "")], null)).toBe("Thinking");
  });
});

describe("outcomeNote", () => {
  it("words the daemon's reason codes as sentences and keeps free-text reasons", () => {
    expect(outcomeNote("failed", "interrupted")).toBe("Interrupted");
    expect(outcomeNote("aborted", "stopped")).toBe("Stopped");
    expect(outcomeNote("failed", "provider said no")).toBe("Failed: provider said no");
    expect(outcomeNote("failed", undefined)).toBe("Failed");
  });
});
