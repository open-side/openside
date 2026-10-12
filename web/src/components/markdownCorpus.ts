// One corpus of hostile and ordinary Markdown with the sanitised result both renderers must
// reach: web/src/components/Markdown.tsx (React) and extension/src/markdown.ts (plain DOM). Both
// parity tests (markdownParity.test.tsx, extension/tests/markdown-parity.test.ts) run it, so the
// two cannot drift apart on what model output may become.

export interface MarkdownFacts {
  /** `href` of every real link (not image chips), in order; "" when the URL was emptied. */
  links: string[];
  /** Image chips: label and the link they offer, if any. */
  chips: { label: string; href: string | null }[];
  /** Elements that must never come out of model text. */
  dangerous: number;
  /** Whether table markup is wrapped for horizontal scrolling. */
  scrollTables: number;
  /** Code blocks by language label. */
  codeLabels: string[];
  /** Visible text with whitespace collapsed (cell spacing differs between renderers). */
  text: string;
}

export interface MarkdownCase {
  name: string;
  markdown: string;
  expected: Partial<MarkdownFacts>;
  /** Fragments that must show up as visible text (not markup). */
  textIncludes?: string[];
}

export const MARKDOWN_CASES: MarkdownCase[] = [
  {
    name: "unsafe link schemes are emptied, safe and relative ones kept",
    markdown: "[a](https://ok.test/x) [b](mailto:a@b.test) [c](/rel) [d](javascript:alert(1)) [e](vbscript:x) [f](data:text/html,boo)",
    expected: { links: ["https://ok.test/x", "mailto:a@b.test", "/rel", "", "", ""], dangerous: 0 },
  },
  {
    name: "a reference-style definition goes through the same rule",
    markdown: "[x][r]\n\n[r]: javascript:alert(1)",
    expected: { links: [""] },
  },
  {
    name: "raw HTML is literal text, never markup",
    markdown: "before <script>alert(1)</script> <iframe src=\"https://evil.test\"></iframe> <img src=x onerror=alert(1)> after",
    expected: { dangerous: 0, chips: [], links: [] },
    textIncludes: ["<script>alert(1)</script>", "<iframe", "onerror=alert(1)"],
  },
  {
    name: "images never fetch: external stays a link, everything else is a plain chip",
    markdown: "![Chart](https://example.test/c.png)\n\n![Rel](./r.png)\n\n![](data:image/png;base64,AAAA)\n\n![Bad](javascript:alert(1))",
    expected: {
      chips: [
        { label: "Chart", href: "https://example.test/c.png" },
        { label: "Rel", href: null },
        { label: "Image", href: null },
        { label: "Bad", href: null },
      ],
      links: [],
      dangerous: 0,
    },
  },
  {
    name: "tables scroll inside their own wrapper",
    markdown: "| a | b |\n| - | - |\n| 1 | 2 |",
    expected: { scrollTables: 1 },
  },
  {
    name: "code blocks keep their language label",
    markdown: "```js\nconst x = 1;\n```\n\n```\nplain\n```",
    expected: { codeLabels: ["js", "text"] },
  },
];

/** Reads the facts out of a rendered container, whatever renderer made it. */
export function factsOf(root: Element): MarkdownFacts {
  const chipNodes = [...root.querySelectorAll(".md-image")];
  return {
    links: [...root.querySelectorAll("a")].filter((link) => !link.classList.contains("md-image")).map((link) => link.getAttribute("href") ?? ""),
    chips: chipNodes.map((chip) => ({
      label: chip.querySelector("span")?.textContent ?? "",
      href: chip.getAttribute("href"),
    })),
    dangerous: root.querySelectorAll("script, iframe, object, embed, style, form, img, [onerror], [onclick]").length,
    scrollTables: root.querySelectorAll(".table-scroll > table").length,
    codeLabels: [...root.querySelectorAll(".code-block-head > span")].map((label) => label.textContent ?? ""),
    text: (root.textContent ?? "").replace(/\s+/g, " ").trim(),
  };
}
