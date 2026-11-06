import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";
import { placeholders, type Message } from "../../web/src/i18n/core";
import { en } from "../src/i18n/en";
import { ko } from "../src/i18n/ko";

const english = en as Record<string, Message>;
const korean = ko as Record<string, Message>;

describe("extension catalogs", () => {
  it("translate every key with the same placeholders", () => {
    expect(Object.keys(korean).sort()).toEqual(Object.keys(english).sort());
    for (const [key, message] of Object.entries(english)) {
      expect([...placeholders(korean[key]!)].sort(), key).toEqual([...placeholders(message)].sort());
    }
  });

  it("know every key the static pages mark", () => {
    for (const page of ["panel.html", "options.html", "import-connect.html", "import-permissions.html", "ask.html", "lasso-access.html", "mic-access.html"]) {
      const html = readFileSync(`${process.cwd()}/static/${page}`, "utf8");
      for (const [, key] of html.matchAll(/data-i18n(?:-[a-z-]+)?="([^"]+)"/g)) expect(english, `${page}: ${key}`).toHaveProperty([key!]);
    }
  });

  // tsc checks keys passed straight to t(); this also catches keys kept in plain-string tables.
  it("know every key the sources name", () => {
    const src = join(process.cwd(), "src");
    const sources = (dir: string): string[] =>
      readdirSync(dir).flatMap((name) => {
        const path = join(dir, name);
        if (statSync(path).isDirectory()) return name === "i18n" ? [] : sources(path);
        return name.endsWith(".ts") ? [path] : [];
      });
    const missing = sources(src).flatMap((path) =>
      [...readFileSync(path, "utf8").matchAll(/["'`](ext\.[A-Za-z][\w-]*(?:\.[A-Za-z0-9_-]+)+)["'`]/g)]
        .map((match) => match[1]!)
        .filter((key) => !(key in english))
        .map((key) => `${relative(src, path)}: ${key}`),
    );
    expect(missing).toEqual([]);
  });
});
