import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { applyPalettes, applyTheme } from "./theme";

const metas = () => Array.from(document.querySelectorAll<HTMLMetaElement>('meta[name="theme-color"]'));

beforeEach(() => {
  document.head.insertAdjacentHTML(
    "beforeend",
    '<meta name="theme-color" content="#000001" media="(prefers-color-scheme: light)">' +
      '<meta name="theme-color" content="#000002" media="(prefers-color-scheme: dark)">',
  );
});

afterEach(() => {
  metas().forEach((meta) => meta.remove());
  localStorage.clear();
  applyTheme("system");
  applyPalettes({ light: "openside", dark: "openside" });
});

describe("theme-color metas", () => {
  it("keeps each scheme's own canvas on its meta under the system preference", () => {
    applyTheme("system");
    const [light, dark] = metas();
    expect(light?.content).toMatch(/^#[0-9a-f]{6}$/i);
    expect(dark?.content).toMatch(/^#[0-9a-f]{6}$/i);
    expect(light?.content).not.toBe(dark?.content);
  });

  it("pins both metas to the chosen scheme when Light or Dark is explicit", () => {
    applyTheme("dark");
    const [light, dark] = metas();
    expect(light?.content).toBe(dark?.content);
    const darkCanvas = dark?.content;
    applyTheme("light");
    expect(metas()[0]?.content).toBe(metas()[1]?.content);
    expect(metas()[0]?.content).not.toBe(darkCanvas);
  });
});
