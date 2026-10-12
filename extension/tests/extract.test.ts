import { readFileSync } from "node:fs";
import { describe, it, expect } from "vitest";
import { extractMessage } from "../src/extract";
function fixture(site: string): Document { return new DOMParser().parseFromString(readFileSync(`${process.cwd()}/tests/fixtures/${site}.html`, "utf8"), "text/html"); }
describe("Gmail and Slack message extraction", () => {
  it("extracts the open Gmail message with source URL and no thread chrome", () => {
    const url = "https://mail.google.com/mail/u/0/#inbox/mock";
    const context = extractMessage(fixture("gmail"), "gmail", url, null);
    expect(context?.selection).toBe("Please summarize the quarterly revenue report and flag the budget change."); expect(context?.url).toBe(url); expect(context?.title).toBe("Quarterly report · Gmail");
  });
  it("extracts the open Slack message with source URL", () => { const context = extractMessage(fixture("slack"), "slack", "https://app.slack.com/client/mock", null); expect(context?.selection).toContain("before Friday"); expect(context?.selection).not.toContain("Older Slack"); });
  it("uses a selection within the selected message and ignores unrelated selections", () => {
    const doc = fixture("gmail"); const node = doc.querySelector(".a3s")!.firstChild!;
    const selected = { anchorNode: node, toString: () => "Older email" } as unknown as Selection;
    expect(extractMessage(doc, "gmail", "https://mail.google.com/", selected)?.selection).toBe("Older email");
    const unrelated = { anchorNode: doc.querySelector("title")!.firstChild, toString: () => "Gmail" } as unknown as Selection;
    expect(extractMessage(doc, "gmail", "https://mail.google.com/", unrelated)?.selection).toContain("quarterly revenue");
  });
  it("never sends the whole page when no message is present", () => { expect(extractMessage(new DOMParser().parseFromString("<h1>Inbox</h1>", "text/html"), "gmail", "https://mail.google.com", null)).toBeNull(); });
});
