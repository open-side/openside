import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { contrastRatio, presetsFor, readableMuted, readableText } from "./theme";

const tokens = readFileSync(`${__dirname}/tokens.css`, "utf8");

/** The hex value of `--name` inside the first rule that starts with `selector {`. */
function token(selector: string, name: string): string {
  const start = tokens.indexOf(`${selector} {`);
  expect(start, selector).toBeGreaterThanOrEqual(0);
  const block = tokens.slice(start, tokens.indexOf("}", start));
  const value = new RegExp(`${name}:\\s*(#[0-9a-f]{6});`, "i").exec(block)?.[1];
  expect(value, `${selector} ${name}`).toBeDefined();
  return value!;
}

describe("text contrast (WCAG AA, 4.5:1)", () => {
  it.each([
    ["light", ":root"],
    ["dark", ':root[data-theme="dark"]'],
  ])("the default %s palette keeps --muted and --text readable on canvas, surface and raised", (_, selector) => {
    for (const background of ["--canvas", "--surface", "--surface-raised", "--overlay"]) {
      for (const foreground of ["--muted", "--text"]) {
        expect(contrastRatio(token(selector, foreground), token(selector, background)), `${foreground} on ${background}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it("the system dark block repeats the dark values", () => {
    for (const name of ["--canvas", "--surface", "--surface-raised", "--muted", "--text"]) {
      expect(token(':root[data-theme="system"],\n  :root:not([data-theme])', name)).toBe(token(':root[data-theme="dark"]', name));
    }
  });

  it.each([...presetsFor("light"), ...presetsFor("dark")].map((preset) => [preset.id, preset] as const))("preset %s derives a readable --text and --muted", (_, preset) => {
    const text = readableText(preset.background, preset.foreground);
    const muted = readableMuted(preset.background, text);
    // tokens.css derives surface (94% canvas) and raised (90%) from the canvas and the applied text.
    for (const share of [1, 0.94, 0.9]) {
      const surface = `#${[1, 3, 5]
        .map((offset) =>
          Math.round(parseInt(preset.background.slice(offset, offset + 2), 16) * share + parseInt(text.slice(offset, offset + 2), 16) * (1 - share))
            .toString(16)
            .padStart(2, "0"),
        )
        .join("")}`;
      expect(contrastRatio(text, surface), `${preset.id} text on ${surface}`).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(muted, surface), `${preset.id} muted on ${surface}`).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("nudges Solarized light's foreground, which is under 4.5:1, toward black", () => {
    const solarized = presetsFor("light").find((preset) => preset.id === "solarized-light")!;
    expect(contrastRatio(solarized.foreground, solarized.background)).toBeLessThan(4.5);
    const text = readableText(solarized.background, solarized.foreground);
    expect(text).not.toBe(solarized.foreground.toLowerCase());
    expect(contrastRatio(text, solarized.background)).toBeGreaterThanOrEqual(4.5);
  });
});
