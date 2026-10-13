import { afterEach, expect, it, vi } from "vitest";

const id = "00000000-0000-4000-8000-000000000001";
const key = `importConnect:${id}`;
const origin = "http://127.0.0.1:43123";

async function page(changedApp = false) {
  document.body.innerHTML = '<h2 id="question"></h2><p id="warning" hidden></p><button id="allow" disabled>Connect</button><button id="decline" disabled>Not now</button><p id="status"></p>';
  location.hash = id;
  const request = vi.fn(async () => true);
  const set = vi.fn(async () => undefined);
  vi.stubGlobal("chrome", { storage: { session: { get: vi.fn(async () => ({ [key]: { origin, allowed: null, changedApp } })), set } }, permissions: { request } });
  // The page entry executes once per document; every test models a new extension document.
  vi.resetModules();
  await import("../src/import-connect");
  await vi.waitFor(() => expect((document.querySelector("#allow") as HTMLButtonElement).disabled).toBe(false));
  return { request, set };
}

afterEach(() => { vi.unstubAllGlobals(); location.hash = ""; });

it("shows the exact origin without receiving a device token, pairing code or secret", async () => {
  const { request, set } = await page();
  expect(document.querySelector("#question")?.textContent).toBe(`Connect openside at ${origin} for importing?`);
  expect(request).not.toHaveBeenCalled();
  expect(set).not.toHaveBeenCalled();
});

it("allows only from the direct person click and records only the origin and decision", async () => {
  const { request, set } = await page();
  (document.querySelector("#allow") as HTMLButtonElement).click();
  expect(request).toHaveBeenCalledExactlyOnceWith({ origins: ["http://127.0.0.1/*"] });
  await vi.waitFor(() => expect(set).toHaveBeenCalledExactlyOnceWith({ [key]: { origin, allowed: true } }));
});

it("declines without requesting any access or storing credentials", async () => {
  const { request, set } = await page();
  (document.querySelector("#decline") as HTMLButtonElement).click();
  await vi.waitFor(() => expect(set).toHaveBeenCalledExactlyOnceWith({ [key]: { origin, allowed: false } }));
  expect(request).not.toHaveBeenCalled();
});

it("warns clearly when the listener cannot prove it is the previously paired app", async () => {
  const { request, set } = await page(true);
  expect(document.querySelector("#warning")?.textContent).toContain(`openside at ${origin} could not prove it's the same app you paired before`);
  expect((document.querySelector("#warning") as HTMLElement).hidden).toBe(false);
  expect(request).not.toHaveBeenCalled();
  expect(set).not.toHaveBeenCalled();
});
