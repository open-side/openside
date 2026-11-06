import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { applyTheme, forgetRetiredThemeKeys } from "./theme";

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
});

describe("theme-color metas", () => {
  it("keeps each scheme's own canvas on its meta under the system preference", () => {
    applyTheme("system");
    const [light, dark] = metas();
    expect(light?.content).toBe("#f9f9f9");
    expect(dark?.content).toBe("#1f1f1f");
  });

  it("pins both metas to the chosen scheme when Light or Dark is explicit", () => {
    applyTheme("dark");
    expect(metas().map((meta) => meta.content)).toEqual(["#1f1f1f", "#1f1f1f"]);
    applyTheme("light");
    expect(metas().map((meta) => meta.content)).toEqual(["#f9f9f9", "#f9f9f9"]);
  });
});

describe("forgetRetiredThemeKeys", () => {
  it("drops the palette and accent keys the removed presets left behind and keeps the theme", () => {
    localStorage.setItem("openside.palette", "{}");
    localStorage.setItem("openside.accent", "#2f6fed");
    localStorage.setItem("openside.theme", "dark");
    forgetRetiredThemeKeys();
    expect(localStorage.getItem("openside.palette")).toBeNull();
    expect(localStorage.getItem("openside.accent")).toBeNull();
    expect(localStorage.getItem("openside.theme")).toBe("dark");
  });
});
