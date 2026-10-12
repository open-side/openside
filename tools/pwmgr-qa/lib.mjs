// Shared helpers for tools/pwmgr-qa: a hermetic opensided and a minimal CDP client.
// Runs under Bun (fetch over a Unix socket, global WebSocket); Node needs `ws` for the socket part.
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync, readFileSync, existsSync, openSync } from "node:fs";
import { join } from "node:path";

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function until(label, fn, timeoutMs = 30000, stepMs = 200) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    try {
      const value = await fn();
      if (value) return value;
    } catch (error) {
      last = error;
    }
    await sleep(stepMs);
  }
  throw new Error(`timeout waiting for ${label}${last ? `: ${last.message}` : ""}`);
}

/** Hermetic daemon: private HOME/XDG tree under `root`, same env contract as crates/openside-e2e. */
export class Daemon {
  constructor({ root, repo, executable, extensions, env = {}, home }) {
    this.home = home ?? join(root, "home");
    this.root = root;
    this.repo = repo;
    this.executable = executable;
    this.extensions = extensions;
    this.extraEnv = env;
    this.dataDir = join(root, "data/openside");
    this.socket = join(root, "run/opensided.sock");
    this.profile = join(this.dataDir, "browser-profile");
  }

  get env() {
    const r = this.root;
    return {
      PATH: process.env.PATH,
      HOME: this.home,
      XDG_CONFIG_HOME: join(r, "config"),
      XDG_DATA_HOME: join(r, "data"),
      XDG_STATE_HOME: join(r, "state"),
      XDG_CACHE_HOME: join(r, "cache"),
      XDG_RUNTIME_DIR: join(r, "run"),
      TMPDIR: join(r, "tmp"),
      OPENSIDE_AGENT_SCRIPT: join(r, "script.json"),
      ...this.extraEnv,
    };
  }

  async start() {
    for (const dir of ["home", "config/openside", "data", "state", "cache", "run", "tmp", "workspace"]) {
      mkdirSync(join(this.root, dir), { recursive: true });
    }
    Bun.spawnSync(["chmod", "700", join(this.root, "run")]);
    writeFileSync(join(this.root, "script.json"), JSON.stringify({ responses: [{ parts: [{ type: "text", text: "ok" }] }] }));
    const python = join(this.repo, "agent/.venv/bin/python");
    const config = [
      `[agent]\ncommand = ${JSON.stringify([python, "-m", "openside_agent"])}`,
      `[models]\ndefault = "script"`,
      `[policy]\nmode = "full"`,
      `[browser]\nexecutable = ${JSON.stringify(this.executable)}\nextensions = ${JSON.stringify(this.extensions)}\ngateway = "auto"`,
    ].join("\n") + "\n";
    writeFileSync(join(this.root, "config/openside/config.toml"), config);
    this.child = spawn(join(this.repo, "target/debug/opensided"), ["--data-dir", this.dataDir, "--socket", this.socket, "--listen", "127.0.0.1:0"], {
      env: this.env,
      cwd: join(this.root, "workspace"),
      // A file, not a pipe: a closed pipe makes the daemon panic in eprintln! while shutting down.
      stdio: ["ignore", "ignore", openSync(join(this.root, "daemon.stderr"), "a")],
      detached: true,
    });
    this.child.on("exit", (code, signal) => { this.exited = `exit code=${code} signal=${signal}`; });
    await until("daemon health", async () => { if (this.exited) throw new Error(`daemon ${this.exited}`); return (await this.api("GET", "/v1/health")).status === 200; }, 120000);
  }

  async api(method, path, body) {
    const response = await fetch(`http://localhost${path}`, {
      method,
      unix: this.socket,
      headers: body ? { "content-type": "application/json" } : {},
      body: body ? JSON.stringify(body) : undefined,
    });
    const text = await response.text();
    let json = null;
    try { json = JSON.parse(text); } catch { /* non-JSON body */ }
    return { status: response.status, body: json ?? text };
  }

  async newSession() {
    const created = await this.api("POST", "/v1/sessions", { title: "pwmgr-qa", mode: "full" });
    if (created.status !== 201) throw new Error(`session ${created.status} ${JSON.stringify(created.body)}`);
    return created.body.id;
  }

  /** Playwright-syntax REPL in the session's agent tab (docs: `openside repl`). */
  async repl(session, code) {
    const result = await this.api("POST", `/v1/sessions/${session}/repl`, { code });
    return result;
  }

  async stop() {
    if (this.child?.pid) {
      try { process.kill(-this.child.pid, "SIGTERM"); } catch { /* already gone */ }
    }
  }

  debugEndpoint() {
    const text = readFileSync(join(this.profile, "DevToolsActivePort"), "utf8").split("\n");
    return `ws://127.0.0.1:${text[0]}${text[1]}`;
  }
}

/** Minimal flat-protocol CDP client over one browser WebSocket. */
export class Cdp {
  static async connect(url) {
    const ws = new WebSocket(url);
    await new Promise((resolve, reject) => {
      ws.onopen = resolve;
      ws.onerror = () => reject(new Error(`cdp connect failed ${url}`));
    });
    return new Cdp(ws);
  }

  constructor(ws) {
    this.ws = ws;
    this.next = 0;
    this.pending = new Map();
    this.events = [];
    ws.onmessage = (message) => {
      const data = JSON.parse(message.data);
      if (data.id !== undefined) {
        const entry = this.pending.get(data.id);
        if (!entry) return; // answer arrived after the call timed out
        this.pending.delete(data.id);
        if (data.error) entry.reject(new Error(`${entry.method}: ${data.error.message}`));
        else entry.resolve(data.result);
      } else {
        this.events.push(data);
      }
    };
  }

  call(method, params = {}, sessionId) {
    const id = ++this.next;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method}: no response in 45s`));
      }, 45000);
      this.pending.set(id, { resolve: (v) => (clearTimeout(timer), resolve(v)), reject: (e) => (clearTimeout(timer), reject(e)), method });
      this.ws.send(JSON.stringify({ id, method, params, sessionId }));
    });
  }

  async targets() {
    return (await this.call("Target.getTargets")).targetInfos;
  }

  async attach(targetId) {
    const { sessionId } = await this.call("Target.attachToTarget", { targetId, flatten: true });
    return sessionId;
  }

  async evaluate(sessionId, expression, awaitPromise = true) {
    const result = await this.call("Runtime.evaluate", { expression, awaitPromise, returnByValue: true }, sessionId);
    if (result.exceptionDetails) throw new Error(`evaluate: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
    return result.result.value;
  }

  async page(url) {
    const { targetId } = await this.call("Target.createTarget", { url });
    return { targetId, sessionId: await this.attach(targetId) };
  }

  async screenshot(sessionId, file) {
    const { data } = await this.call("Page.captureScreenshot", { format: "png" }, sessionId);
    writeFileSync(file, Buffer.from(data, "base64"));
  }

  async click(sessionId, x, y) {
    for (const type of ["mousePressed", "mouseReleased"]) {
      await this.call("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1 }, sessionId);
    }
  }

  async type(sessionId, text) {
    for (const ch of text) {
      await this.call("Input.dispatchKeyEvent", { type: "keyDown", text: ch, key: ch }, sessionId);
      await this.call("Input.dispatchKeyEvent", { type: "keyUp", key: ch }, sessionId);
    }
  }

  close() {
    this.ws.close();
  }
}
