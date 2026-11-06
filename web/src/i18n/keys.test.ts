import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { en } from "./catalog";

// tsc checks keys passed straight to t(); this also catches keys kept in plain-string tables,
// tMaybe literals and tMaybe prefixes, which tsc cannot see.
const SRC = join(process.cwd(), "src");
const NAMESPACES = [...new Set(Object.keys(en).map((key) => key.split(".")[0]!))];
const KEY = new RegExp(`["'\`]((?:${NAMESPACES.join("|")})\\.[A-Za-z][\\w-]*(?:\\.[A-Za-z0-9_-]+)+)["'\`]`, "g");
const PREFIX = /tMaybe\(`((?:[A-Za-z0-9_-]+\.)+)\$\{/g;

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return name === "i18n" ? [] : sources(path);
    // The generated OpenAPI schema names config paths (`memory.embedding.model_dir`), not messages.
    if (path === join(SRC, "api/schema.ts")) return [];
    return /\.tsx?$/.test(name) && !/\.test\.tsx?$/.test(name) ? [path] : [];
  });
}

describe("message key references", () => {
  const files = sources(SRC).map((path) => ({ path: relative(SRC, path), text: readFileSync(path, "utf8") }));

  it("names only keys the English catalog has", () => {
    const missing = files.flatMap(({ path, text }) => [...text.matchAll(KEY)].map((match) => match[1]!).filter((key) => !(key in en)).map((key) => `${path}: ${key}`));
    expect(missing).toEqual([]);
  });

  it("looks up computed keys under prefixes the catalog uses", () => {
    const keys = Object.keys(en);
    const empty = files.flatMap(({ path, text }) => [...text.matchAll(PREFIX)].map((match) => match[1]!).filter((prefix) => !keys.some((key) => key.startsWith(prefix))).map((prefix) => `${path}: ${prefix}`));
    expect(empty).toEqual([]);
  });
});
