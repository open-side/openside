#!/usr/bin/env node
// Regenerates bench/snapshot/agent-browser-baseline.json. Usage (see README.md):
//   AGENT_BROWSER=<path to agent-browser binary> CHROMIUM=<chromium executable> \
//     node bench/snapshot/agent-browser-baseline.mjs
// agent-browser keeps state under $HOME; this script points HOME at a fresh temporary
// directory so the user's own agent-browser/Chrome state is never read or written.
import { execFile } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
const here = new URL(".", import.meta.url).pathname;
const binary = process.env.AGENT_BROWSER;
const chromium = process.env.CHROMIUM;
if (!binary || !chromium) {
  console.error("set AGENT_BROWSER and CHROMIUM");
  process.exit(2);
}
const workflow = JSON.parse(readFileSync(join(here, "workflow.json"), "utf8"));
const pages = readdirSync(join(here, "pages")).filter((name) => name.endsWith(".html")).sort();
const server = createServer((request, response) => {
  const name = decodeURIComponent(request.url.slice(1));
  if (!pages.includes(name)) {
    response.writeHead(404).end();
    return;
  }
  response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
  response.end(readFileSync(join(here, "pages", name)));
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const home = mkdtempSync(join(tmpdir(), "agent-browser-home-"));
const env = { ...process.env, HOME: home, AGENT_BROWSER_EXECUTABLE_PATH: chromium };
// Asynchronous: a blocking call would stall the in-process page server it is fetching from.
const run = async (...args) =>
  (await promisify(execFile)(binary, ["--session", "openside-bench", ...args], { env, encoding: "utf8" })).stdout;
const version = (await run("--version")).trim();
const results = {};
try {
  for (const page of pages) {
    await run("open", `${origin}/${page}`);
    const snapshots = [Buffer.byteLength(await run("snapshot"))];
    for (const step of workflow.steps) {
      await run("eval", step.script);
      snapshots.push(Buffer.byteLength(await run("snapshot")));
    }
    results[page] = snapshots;
    console.log(page, snapshots.join(" "));
  }
} finally {
  try {
    await run("close", "--all");
  } catch {}
  server.close();
  rmSync(home, { recursive: true, force: true });
}
writeFileSync(
  join(here, "agent-browser-baseline.json"),
  JSON.stringify(
    {
      tool: "agent-browser (npm, vercel-labs/agent-browser)",
      version,
      command: "agent-browser --session openside-bench open <page>; agent-browser snapshot; then per workflow step: agent-browser eval <step.script>; agent-browser snapshot",
      environment: "HOME redirected to a fresh temporary directory; AGENT_BROWSER_EXECUTABLE_PATH set to the local Chromium; pages served from 127.0.0.1",
      unit: "UTF-8 bytes of each `agent-browser snapshot` stdout (default options), in order: after load, then after each workflow step",
      pages: results,
    },
    null,
    2,
  ) + "\n",
);
