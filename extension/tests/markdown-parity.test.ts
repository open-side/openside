import { describe, expect, it } from "vitest";
import { factsOf, MARKDOWN_CASES } from "../../web/src/components/markdownCorpus";
import { renderMarkdown } from "../src/markdown";

describe("side panel Markdown reaches the same sanitised result as the web app", () => {
  it.each(MARKDOWN_CASES)("$name", ({ markdown, expected, textIncludes }) => {
    const target = document.createElement("div");
    renderMarkdown(target, markdown);
    const facts = factsOf(target);
    expect(facts).toMatchObject(expected);
    for (const fragment of textIncludes ?? []) expect(facts.text).toContain(fragment);
  });
});
