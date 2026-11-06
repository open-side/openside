import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { artifactPath, Markdown } from "./Markdown";

describe("Markdown images", () => {
  it("never renders an <img>: external, relative and daemon-API sources stay non-fetching", () => {
    const { container } = render(
      <Markdown
        text={[
          "![Chart](https://example.com/chart.png)",
          "![Diagram](./diagram.png)",
          "![Artifact](/v1/sessions/s1/artifacts/a.png)",
          `![Local](${window.location.origin}/v1/sessions/s1/artifacts/b.png)`,
          "![](https://example.com/untitled.png)",
        ].join("\n\n")}
      />,
    );
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("[src]")).toBeNull();

    const external = screen.getByRole("link", { name: "Chart" });
    expect(external).toHaveAttribute("href", "https://example.com/chart.png");
    expect(external).toHaveAttribute("rel", "noopener noreferrer");
    expect(screen.getByRole("link", { name: "Image" })).toHaveAttribute("href", "https://example.com/untitled.png");

    // Relative and same-origin URLs would reach the daemon without the API client's token.
    for (const name of ["Diagram", "Artifact", "Local"]) {
      expect(screen.getByRole("img", { name })).not.toHaveAttribute("href");
      expect(screen.queryByRole("link", { name })).toBeNull();
    }
  });
});

describe("Markdown tables", () => {
  it("marks cells that are only a number so they align right, unless the author aligned the column", () => {
    const { container } = render(<Markdown text={["| Plan | Price | Share | Note |", "| --- | --- | :-: | --- |", "| Team | $1,200 | 42% | 3 seats left |"].join("\n")} />);
    const cells = [...container.querySelectorAll("td")];
    expect(cells.map((cell) => cell.hasAttribute("data-numeric"))).toEqual([false, true, false, false]);
  });
});

describe("artifactPath", () => {
  it("accepts plain relative files and decodes them", () => {
    expect(artifactPath("chart.png")).toBe("chart.png");
    expect(artifactPath("./out/report.md")).toBe("out/report.md");
    expect(artifactPath("out/my%20report.md?x=1#top")).toBe("out/my report.md");
  });

  it("rejects schemes, rooted paths and any way out of the chat's folder, encoded or not", () => {
    for (const href of [
      undefined,
      "",
      "#top",
      "https://example.com/a.png",
      "javascript:alert(1)",
      "/etc/passwd",
      "../secret",
      "out/../../secret",
      "%2e%2e/secret",
      "foo%2f..%2fsecret",
      "%2fetc%2fpasswd",
      "..%5csecret",
      "a\\b",
      "a//b",
      "a/./b",
      "%E0%A4%A",
    ]) {
      expect(artifactPath(href), String(href)).toBeNull();
    }
  });
});
