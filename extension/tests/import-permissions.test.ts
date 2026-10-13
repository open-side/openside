import { afterEach, expect, it, vi } from "vitest";
// This entry point binds the DOM and Chrome APIs at import; each fixture needs a fresh module.

const id = "7df44135-84a6-44aa-9bac-8688269b9843";
afterEach(() => { vi.unstubAllGlobals(); document.body.innerHTML = ""; });

it("requests the exact saved hosts only from the Allow click and reports the result", async () => {
  vi.resetModules();
  document.body.innerHTML = '<ul id="sites"></ul><button id="allow" disabled>Allow</button><p id="status" role="status"></p>';
  vi.stubGlobal("location", { hash: `#${id}` });
  const request = vi.fn(async () => true);
  const set = vi.fn(async () => undefined);
  vi.stubGlobal("chrome", { storage: { session: { get: vi.fn(async () => ({ [`importPermissions:${id}`]: { origin: "http://127.0.0.1:43123", origins: ["https://example.test/*", "https://other.test/*"], denied: false } })), set } }, permissions: { request } });
  await import("../src/import-permissions");
  const button = document.querySelector<HTMLButtonElement>("#allow")!;
  await vi.waitFor(() => expect(button.disabled).toBe(false));
  expect(request).not.toHaveBeenCalled();
  expect(document.querySelector("#sites")?.textContent).toBe("example.testother.test");
  button.click();
  expect(request).toHaveBeenCalledWith({ origins: ["https://example.test/*", "https://other.test/*"] });
  await vi.waitFor(() => expect(document.querySelector("#status")?.textContent).toContain("Site access allowed"));
  expect(set).toHaveBeenCalledWith({ [`importPermissions:${id}`]: { origin: "http://127.0.0.1:43123", origins: ["https://example.test/*", "https://other.test/*"], denied: false } });
});

it("does not request access when the permission request expired", async () => {
  vi.resetModules();
  document.body.innerHTML = '<ul id="sites"></ul><button id="allow" disabled>Allow</button><p id="status" role="status"></p>';
  vi.stubGlobal("location", { hash: `#${id}` });
  const request = vi.fn(async () => true);
  vi.stubGlobal("chrome", { storage: { session: { get: vi.fn(async () => ({})) } }, permissions: { request } });
  await import("../src/import-permissions");
  await vi.waitFor(() => expect(document.querySelector("#status")?.textContent).toContain("no longer available"));
  expect(document.querySelector<HTMLButtonElement>("#allow")?.disabled).toBe(true);
  expect(request).not.toHaveBeenCalled();
});
