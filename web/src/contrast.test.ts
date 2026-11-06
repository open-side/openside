import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { contrastRatio } from "./theme";

const tokens = readFileSync(`${__dirname}/tokens.css`, "utf8");

const LIGHT = ":root";
const DARK = ':root[data-theme="dark"]';
const SYSTEM_DARK = ':root[data-theme="system"],\n  :root:not([data-theme])';

/** Every `--name: value;` declaration inside the first rule that starts with `selector {`. */
function block(selector: string): Map<string, string> {
  const start = tokens.indexOf(`${selector} {`);
  expect(start, selector).toBeGreaterThanOrEqual(0);
  const body = tokens.slice(start, tokens.indexOf("}", start));
  return new Map([...body.matchAll(/(--[a-z0-9-]+):\s*([^;]+);/g)].map((match) => [match[1]!, match[2]!.trim()]));
}

function hex(selector: string, name: string): string {
  const value = block(selector).get(name);
  expect(value, `${selector} ${name}`).toMatch(/^#[0-9a-f]{6}$/i);
  return value!;
}

describe("text contrast (WCAG AA, 4.5:1)", () => {
  it.each([
    ["light", LIGHT],
    ["dark", DARK],
  ])("the %s palette keeps --muted and --text readable on canvas, surface, raised and overlay", (_, selector) => {
    for (const background of ["--canvas", "--surface", "--surface-raised", "--overlay"]) {
      for (const foreground of ["--muted", "--text"]) {
        expect(contrastRatio(hex(selector, foreground), hex(selector, background)), `${foreground} on ${background}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it.each([
    ["light", LIGHT],
    ["dark", DARK],
  ])("the %s settings rows read at AA on their cards", (_, selector) => {
    for (const foreground of ["--row-title", "--row-desc"]) {
      expect(contrastRatio(hex(selector, foreground), hex(selector, "--card-bg")), foreground).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("the primary button label reads on the primary fill in both schemes", () => {
    for (const selector of [LIGHT, DARK]) {
      expect(contrastRatio(hex(selector, "--primary-foreground"), hex(selector, "--primary"))).toBeGreaterThanOrEqual(4.5);
    }
  });
});

const AA = 4.5;
/** WCAG 1.4.11 / large-text level: Aside's own secondary labels, links and control glyphs sit here. */
const UI = 3;

/**
 * Every foreground the token set paints on a token background, checked in both schemes. Values are
 * Aside's measured colours, so a few secondary labels only reach 3:1; the last group lists the
 * pairs that sit below 3:1 in Aside itself, held at their measured floor so they never fall further.
 */
const PAIRS: [foreground: string, background: string, floor: number][] = [
  ["--sidebar-text", "--sidebar-bg", AA],
  ["--sidebar-text-selected", "--sidebar-row-selected", AA],
  ["--text", "--badge-bg", AA],
  ["--tooltip-text", "--tooltip-bg", AA],
  ["--tooltip-strong-text", "--tooltip-strong-bg", AA],
  ["--text", "--menu-hover", AA],
  ["--menu-label", "--overlay", AA],
  ["--menu-label", "--menu-bg-solid", AA],
  ["--text", "--palette-bg", AA],
  ["--text", "--palette-row-hover", AA],
  ["--seg-text-active", "--seg-active", AA],
  ["--settings-nav-text-selected", "--settings-nav-selected", AA],
  ["--button-secondary-text", "--button-secondary-bg", AA],
  ["--text", "--button-outline-bg", AA],
  ["--send-fg", "--send-bg", AA],
  ["--danger", "--canvas", AA],
  ["--danger", "--card-bg", AA],
  ["--danger", "--overlay", AA],
  ["--success", "--canvas", AA],
  ["--warn", "--canvas", AA],
  ["--avatar-fg", "--avatar-bg", AA],
  ["--muted", "--sidebar-bg", UI],
  ["--sidebar-secondary", "--sidebar-bg", UI],
  ["--link", "--canvas", UI],
  ["--link", "--surface", UI],
  ["--link", "--surface-raised", UI],
  ["--link", "--overlay", UI],
  ["--badge-new-text", "--badge-new-bg", UI],
  ["--banner-text", "--banner-bg", UI],
  ["--info-banner-text", "--info-banner-bg", UI],
  ["--danger", "--danger-surface", UI],
  ["--menu-icon", "--overlay", UI],
  ["--menu-icon", "--menu-bg-solid", UI],
  ["--seg-text", "--seg-track", UI],
  ["--settings-nav-text", "--settings-nav-bg", UI],
  ["--section-label", "--settings-sheet-bg", UI],
  ["--placeholder", "--control-bg", UI],
  ["--placeholder", "--omnibox-bg", UI],
  ["--timestamp", "--canvas", UI],
  ["--task-card-desc", "--task-card-bg", UI],
  ["--breadcrumb", "--canvas", UI],
  ["--toolbar-icon", "--canvas", UI],
  // Below 3:1 in Aside's light palette; the switch also shows its state by the knob's side.
  ["--sidebar-heading", "--sidebar-bg", 2.9],
  ["--toggle-on", "--canvas", 2.3],
  ["--toggle-on", "--card-bg", 2.4],
  ["--toggle-knob", "--toggle-on", 2.4],
  ["--toggle-off", "--card-bg", 1.2],
];

describe("semantic pairs", () => {
  it.each([
    ["light", LIGHT],
    ["dark", DARK],
  ])("every %s foreground keeps its contrast floor on its background", (_, selector) => {
    for (const [foreground, background, floor] of PAIRS) {
      expect(contrastRatio(hex(selector, foreground), hex(selector, background)), `${foreground} on ${background}`).toBeGreaterThanOrEqual(floor);
    }
  });
});

describe("token blocks", () => {
  it("the system dark block repeats every dark value", () => {
    expect(Object.fromEntries(block(SYSTEM_DARK))).toEqual(Object.fromEntries(block(DARK)));
  });

  it("every dark colour overrides a light one", () => {
    const light = block(LIGHT);
    for (const name of block(DARK).keys()) expect(light.has(name), name).toBe(true);
  });

  it("no blue primary is left", () => {
    for (const selector of [LIGHT, DARK]) {
      const primary = hex(selector, "--primary");
      const [r, g, b] = [1, 3, 5].map((offset) => parseInt(primary.slice(offset, offset + 2), 16));
      expect(Math.max(r!, g!, b!) - Math.min(r!, g!, b!), primary).toBeLessThanOrEqual(2);
    }
  });
});
