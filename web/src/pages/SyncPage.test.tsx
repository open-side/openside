import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { PinnedTab, SyncStatus } from "../api/types";
import { ClientContext } from "../context";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { SyncPage } from "./SyncPage";

const OFF: SyncStatus = {
  configured: false,
  set_up: false,
  unlocked: false,
  records: 0,
  pending: 0,
  exhausted: false,
  settings: false,
  memory_tags: [],
  passphrase_source: "entered",
  last_run: null,
  location: null,
  password_source: null,
  webdav_username: null,
};

function setup(initial: SyncStatus, handler: (call: Call, status: () => SyncStatus, set: (next: SyncStatus) => void) => Response | undefined = () => undefined) {
  let status = initial;
  const tabs: PinnedTab[] = [{ id: "t1", url: "https://docs.example.com", title: "Docs", pinned_at: "2026-10-01T00:00:00Z" }];
  const fake = fakeClient((call) => {
    const custom = handler(call, () => status, (next) => (status = next));
    if (custom) return custom;
    if (call.path === "/v1/sync") return json(200, status);
    if (call.path === "/v1/pinned-tabs" && call.method === "GET") return json(200, { tabs });
    if (call.path === "/v1/pinned-tabs" && call.method === "POST") {
      const { url } = call.body as { url: string };
      tabs.push({ id: `t${tabs.length + 1}`, url, title: null, pinned_at: "2026-10-02T00:00:00Z" });
      return json(201, tabs.at(-1));
    }
    if (call.method === "DELETE" && call.path.startsWith("/v1/pinned-tabs/")) {
      tabs.splice(0, tabs.length, ...tabs.filter((tab) => tab.id !== call.path.split("/").pop()));
      return json(204, undefined);
    }
    return json(404, { code: "not_found", message: "nope" });
  });
  render(
    <ClientContext.Provider value={fake.client}>
      <SyncPage />
    </ClientContext.Provider>,
  );
  return fake;
}

describe("SyncPage", () => {
  it("starts off, saves a shared folder and then offers to start or join an account", async () => {
    const user = userEvent.setup();
    const { calls } = setup(OFF, (call, _status, set) => {
      if (call.method === "PUT" && call.path === "/v1/sync/config") {
        const body = call.body as { dir: string; settings: boolean; memory_tags: string[] };
        const next: SyncStatus = { ...OFF, configured: true, backend: "dir", location: body.dir, settings: body.settings, memory_tags: body.memory_tags };
        set(next);
        return json(200, next);
      }
      return undefined;
    });
    await screen.findByRole("form", { name: "Sync storage" });
    expect(screen.getByText("Off", { selector: ".badge" })).toBeInTheDocument();
    await user.click(screen.getByRole("radio", { name: "Shared folder" }));
    await user.type(screen.getByLabelText("Folder"), "~/Sync/openside");
    await user.click(screen.getByRole("switch", { name: "Sync the permission policy" }));
    await user.type(screen.getByLabelText("Memory tags to sync"), "shared, travel");
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("radio", { name: "Start a new account" })).toBeInTheDocument();
    expect(calls.find((call) => call.method === "PUT")?.body).toEqual({ dir: "~/Sync/openside", settings: true, memory_tags: ["shared", "travel"] });
  });

  it("starts an account with a typed passphrase and shows the code for the other device, without keeping the passphrase", async () => {
    const user = userEvent.setup();
    const configured: SyncStatus = { ...OFF, configured: true, backend: "dir", location: "/sync" };
    const { calls } = setup(configured, (call, _status, set) => {
      if (call.method === "POST" && call.path === "/v1/sync/setup") {
        const next: SyncStatus = { ...configured, set_up: true, unlocked: true, account_id: "acct", device_id: "dev1" };
        set(next);
        return json(201, { pairing_code: "osync1-ABC", status: next });
      }
      return undefined;
    });
    const form = await screen.findByRole("form", { name: "Start a sync account" });
    const field = within(form).getByLabelText("Sync passphrase");
    expect(field).toHaveAttribute("type", "password");
    await user.type(field, "correct horse battery");
    await user.click(within(form).getByRole("button", { name: "Start account" }));
    expect(await screen.findByRole("button", { name: "Sync now" })).toBeInTheDocument();
    expect(calls.find((call) => call.path === "/v1/sync/setup")?.body).toEqual({ passphrase: "correct horse battery" });
  });

  it("joins with a code and the same passphrase", async () => {
    const user = userEvent.setup();
    const configured: SyncStatus = { ...OFF, configured: true, backend: "webdav", location: "https://dav.example.com/", webdav_username: "me", password_source: "entered" };
    const { calls } = setup(configured, (call, _status, set) => {
      if (call.method === "POST" && call.path === "/v1/sync/pair") {
        set({ ...configured, set_up: true, unlocked: true });
        return json(200, { ...configured, set_up: true, unlocked: true });
      }
      return undefined;
    });
    await user.click(await screen.findByRole("radio", { name: "Join with a code" }));
    const form = screen.getByRole("form", { name: "Join a sync account" });
    await user.type(within(form).getByLabelText("Sync code"), "osync1-ABC");
    await user.type(within(form).getByLabelText("Sync passphrase"), "correct horse battery");
    await user.type(within(form).getByLabelText("WebDAV password"), "hunter2");
    await user.click(within(form).getByRole("button", { name: "Join account" }));
    expect(await screen.findByRole("button", { name: "Sync now" })).toBeInTheDocument();
    expect(calls.find((call) => call.path === "/v1/sync/pair")?.body).toEqual({ pairing_code: "osync1-ABC", passphrase: "correct horse battery", webdav_password: "hunter2" });
  });

  it("unlocks a joined account, syncs now and lists what happened", async () => {
    const user = userEvent.setup();
    const locked: SyncStatus = { ...OFF, configured: true, backend: "dir", location: "/sync", set_up: true, account_id: "acct", device_id: "dev1", last_sync: "2026-10-11T08:00:00Z", records: 12, pending: 2 };
    const { calls } = setup(locked, (call, _status, set) => {
      if (call.path === "/v1/sync/unlock") {
        set({ ...locked, unlocked: true });
        return json(200, { ...locked, unlocked: true });
      }
      if (call.path === "/v1/sync/run") return json(200, { pushed: 2, pulled: 5, applied: 4, conflicts: 1, rejected: 0, deferred: 3, vault_available: false });
      return undefined;
    });
    const form = await screen.findByRole("form", { name: "Unlock sync" });
    await user.type(within(form).getByLabelText("Sync passphrase"), "correct horse battery");
    await user.click(within(form).getByRole("button", { name: "Unlock" }));
    await user.click(await screen.findByRole("button", { name: "Sync now" }));
    const report = await screen.findByRole("status", { name: "Last sync" });
    expect(report).toHaveTextContent("Uploaded2");
    expect(report).toHaveTextContent("Replaced by a newer version1");
    expect(report).toHaveTextContent("Left out (the vault is locked)");
    expect(calls.find((call) => call.path === "/v1/sync/unlock")?.body).toEqual({ passphrase: "correct horse battery" });
  });

  it("explains a wrong passphrase in words", async () => {
    const user = userEvent.setup();
    const locked: SyncStatus = { ...OFF, configured: true, backend: "dir", location: "/sync", set_up: true };
    setup(locked, (call) => (call.path === "/v1/sync/unlock" ? json(400, { code: "sync_wrong_passphrase", message: "bad" }) : undefined));
    const form = await screen.findByRole("form", { name: "Unlock sync" });
    await user.type(within(form).getByLabelText("Sync passphrase"), "nope nope nope");
    await user.click(within(form).getByRole("button", { name: "Unlock" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Wrong sync passphrase.");
  });

  it("lists, adds and removes pinned tabs", async () => {
    const user = userEvent.setup();
    const { calls } = setup(OFF);
    const list = await screen.findByRole("list", { name: "Pinned tabs" });
    expect(within(list).getByText("Docs")).toBeInTheDocument();
    await user.type(screen.getByLabelText("Address"), "https://news.example.com");
    await user.click(screen.getByRole("button", { name: "Pin" }));
    expect(await screen.findByText("https://news.example.com")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Unpin Docs" }));
    await screen.findByText("https://news.example.com");
    expect(screen.queryByText("Docs")).toBeNull();
    expect(calls.some((call) => call.method === "DELETE" && call.path === "/v1/pinned-tabs/t1")).toBe(true);
  });
});
