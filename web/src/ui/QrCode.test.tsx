import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { QrCode } from "./QrCode";

describe("QrCode", () => {
  it("draws the code locally as a labelled black-on-white SVG with a quiet zone", () => {
    const { container } = render(<QrCode text="https://desk.example/pair#abc" label="Pairing QR code" />);
    const svg = screen.getByRole("img", { name: "Pairing QR code" });
    expect(svg.tagName.toLowerCase()).toBe("svg");
    expect(container.querySelector("rect")).toHaveAttribute("fill", "#fff");
    expect(container.querySelector("path")).toHaveAttribute("fill", "#000");
    // A QR symbol is 17 + 4 × version modules wide; the quiet zone adds 4 on each side.
    const extent = Number(svg.getAttribute("viewBox")!.split(" ")[2]);
    expect((extent - 8 - 17) % 4).toBe(0);
    expect(extent).toBeGreaterThanOrEqual(21 + 8);
    expect(container.querySelector("path")!.getAttribute("d")!.startsWith("M4 4")).toBe(true);
  });

  it("grows for longer links", () => {
    const short = render(<QrCode text="a" label="short" />).container.querySelector("svg")!;
    const long = render(<QrCode text={"https://relay.example.com/pair#c=" + "x".repeat(150)} label="long" />).container.querySelector("svg")!;
    const side = (svg: SVGElement) => Number(svg.getAttribute("viewBox")!.split(" ")[2]);
    expect(side(long)).toBeGreaterThan(side(short));
  });
});
