import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Markdown } from "./Markdown";
import { factsOf, MARKDOWN_CASES } from "./markdownCorpus";
import { imageChip, safeUrl } from "./markdownPolicy";

describe("Markdown sanitisation corpus (shared with the extension)", () => {
  it.each(MARKDOWN_CASES)("$name", ({ markdown, expected, textIncludes }) => {
    const { container } = render(<Markdown text={markdown} />);
    const facts = factsOf(container);
    expect(facts).toMatchObject(expected);
    for (const fragment of textIncludes ?? []) expect(facts.text).toContain(fragment);
  });
});

describe("markdownPolicy", () => {
  it("empties schemes react-markdown would not allow and keeps relative references", () => {
    expect(safeUrl("javascript:alert(1)")).toBe("");
    expect(safeUrl("JaVaScRiPt:alert(1)")).toBe("");
    expect(safeUrl("data:text/html,x")).toBe("");
    expect(safeUrl("https://a.test")).toBe("https://a.test");
    expect(safeUrl("/v1/x?next=a:b")).toBe("/v1/x?next=a:b");
  });

  it("offers a link only for an absolute http(s) image on another origin", () => {
    expect(imageChip("https://x.test/a.png", " Cat ", "https://app.test")).toEqual({ label: "Cat", href: "https://x.test/a.png" });
    expect(imageChip("https://app.test/a.png", undefined, "https://app.test")).toEqual({ label: "Image", href: null });
    expect(imageChip("javascript:alert(1)", "x", "https://app.test")).toEqual({ label: "x", href: null });
  });
});
