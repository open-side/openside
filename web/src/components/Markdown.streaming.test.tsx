import { render } from "@testing-library/react";
import type { ComponentProps } from "react";
import type * as ReactMarkdownModule from "react-markdown";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { Markdown } from "./Markdown";
import { BlockScanner, STREAM_CODE_MIN } from "./markdownBlocks";

// The parser is called through react-markdown's default export; recording its input shows
// exactly what was (re-)parsed, independent of timing.
const parsed: string[] = vi.hoisted(() => []);
vi.mock("react-markdown", async () => {
  const actual =
    await vi.importActual<typeof ReactMarkdownModule>("react-markdown");
  const Real = actual.default;
  return {
    ...actual,
    default: (props: ComponentProps<typeof Real>) => {
      parsed.push(String(props.children));
      return <Real {...props} />;
    },
  };
});

beforeEach(async () => {
  parsed.length = 0;
  // Dynamic import on purpose: a static import only loads the module, while this await settles
  // the exact promise Markdown's lazy `import("rehype-highlight")` created. Otherwise it can
  // resolve mid-test, swap the plugins array and re-render every memoized chunk, inflating the
  // parse log the assertions count.
  await import("rehype-highlight");
});

const paragraph = (i: number) =>
  `Paragraph ${i}: **bold** and \`code\` and [a link](https://example.com/${i}) plus a few more words so each block has some length to it.`;

function prose(count: number) {
  return Array.from({ length: count }, (_, i) => paragraph(i)).join("\n\n");
}

function stream(text: string, step: number) {
  const view = render(<Markdown text="" />);
  for (let end = step; end < text.length + step; end += step) {
    view.rerender(
      <Markdown text={text.slice(0, Math.min(end, text.length))} />,
    );
  }
  return view;
}

describe("Markdown streaming", () => {
  it("parses each finished segment once while the tail keeps growing", () => {
    const text = prose(60);
    // Delta size only sets the parser's workload; the spy on the parser, not wall-clock, proves
    // each segment is parsed exactly once and the total volume stays linear.
    const step = 256;
    const steps = Math.ceil(text.length / step);
    stream(text, step);

    const { segments } = new BlockScanner().view(text);
    expect(segments.length).toBeGreaterThan(2);
    for (const segment of segments) {
      expect(
        parsed.filter((call) => call === segment),
        segment.slice(0, 30),
      ).toHaveLength(1);
    }
    // Quadratic parsing would feed ~steps * text.length/2 characters to the parser.
    const fed = parsed.reduce((sum, call) => sum + call.length, 0);
    expect(fed).toBeLessThan(steps * 5000);
    expect(fed).toBeLessThan((steps * text.length) / 4);
  });

  it("streams a large open code fence without parsing its body, then parses it once on close", () => {
    const body = Array.from(
      { length: 1200 },
      (_, i) => `    let value_${i} = compute(${i});`,
    ).join("\n");
    expect(body.length).toBeGreaterThan(STREAM_CODE_MIN * 1.5);
    const open = `Intro paragraph.\n\n\`\`\`rust\n${body}\n`;
    // A few chunk-sized deltas, not hundreds: the parse log below proves the streamed body is
    // never parsed, so the test needs no wall-clock pressure. Small deltas would still be
    // correct but would re-parse the growing pre-threshold tail hundreds of times.
    const view = stream(open, STREAM_CODE_MIN / 4);

    const bodyParsed = parsed.filter((call) => call.includes("let value_900"));
    expect(bodyParsed).toHaveLength(0);
    expect(
      view.container.querySelector(".code-block-head")?.textContent,
    ).toContain("rust");
    expect(
      view.container.querySelector(".code-block pre")?.textContent,
    ).toContain("let value_1199");

    parsed.length = 0;
    view.rerender(<Markdown text={`${open}\`\`\`\n`} />);
    view.rerender(
      <Markdown text={`${open}\`\`\`\n\nAfter the code, more text`} />,
    );
    view.rerender(
      <Markdown
        text={`${open}\`\`\`\n\nAfter the code, more text, and more`}
      />,
    );
    expect(
      parsed.filter((call) => call.includes("let value_900")),
    ).toHaveLength(1);
    expect(view.container.querySelectorAll(".code-block")).toHaveLength(1);
    expect(
      view.container.querySelector(".code-block pre code")?.textContent,
    ).toContain("let value_0 =");
  });

  it("renders the same document as a single parse", () => {
    const sections = [
      "# Title\n\nIntro with a [ref link](https://example.com).",
      "1. one\n\n   continued under one\n\n2. two\n\n3. three",
      "- loose\n\n- items\n\n- here",
      "```js\nconst a = 1;\n\nconst b = 2;\n```",
      "~~~\ntilde fence\n\n```\nstill inside\n~~~",
      "| a | b |\n| - | - |\n| 1 | 2 |",
      "> quote\n>\n> more quote",
      "    indented code\n\n    still code",
      "Setext\n======\n\ntext after",
    ];
    const text = Array.from({ length: 40 }, () => sections.join("\n\n")).join(
      "\n\n",
    );
    expect(new BlockScanner().view(text).segments.length).toBeGreaterThan(3);

    const chunked = render(<Markdown text={text} />).container.innerHTML;
    // An unused link definition forces the single-parse fallback and renders nothing itself.
    const whole = render(
      <Markdown text={`${text}\n\n[unused]: https://example.com`} />,
    ).container.innerHTML;
    expect(chunked).toBe(whole);
  });

  it("falls back to one parse when a reference definition can resolve earlier text", () => {
    const text = `${prose(40)}\n\nSee [the docs][docs].\n\n[docs]: https://example.com/docs\n`;
    const view = new BlockScanner().view(text);
    expect(view.segments).toEqual([]);
    expect(view.tail).toBe(text);
  });
});
