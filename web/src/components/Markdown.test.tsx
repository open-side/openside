import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Markdown } from "./Markdown";

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
