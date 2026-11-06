import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MemoryHealth, MemoryModelStatus } from "../../api/types";
import { ClientContext } from "../../context";
import { fakeClient, json, type Call } from "../../test/fakeDaemon";
import { ToastProvider } from "../../ui";
import { MemoryConfigure } from "./MemoryConfigure";

const SOURCE = { endpoint: "https://huggingface.co", repo: "minishlab/potion-base-8M", revision: "main" };
const UPDATED = new Date(Date.now() - 2 * 3_600_000).toISOString();
const CONFIG = { memory: { history: true, default_retention: "permanent" }, models: { default: "anthropic/x" } };

function model(patch: Partial<MemoryModelStatus> = {}): MemoryModelStatus {
  return {
    state: "idle",
    installed: false,
    model_dir: "/data/models/potion",
    source: SOURCE,
    files_done: 0,
    files_total: 0,
    bytes_done: 0,
    bytes_total: 0,
    restart_required: false,
    ...patch,
  };
}

function health(patch: Partial<MemoryHealth> = {}): MemoryHealth {
  return { count: 10, mode: "hybrid", ...patch };
}

function doc(config: unknown, restart = false) {
  return { config, restart_required: restart, file_only: [], remote: false, remote_editable: [], schema: {} };
}

interface Routes {
  health?: () => MemoryHealth;
  model?: (call: Call) => Response | Promise<Response>;
  rebuild?: () => Response | Promise<Response>;
  put?: (call: Call) => Response;
}

function setup(routes: Routes = {}) {
  const fake = fakeClient((call) => {
    if (call.path === "/v1/memory/health") return json(200, routes.health?.() ?? health());
    if (call.path === "/v1/memory/files")
      return json(200, {
        files: [
          { name: "MEMORY.md", view: "memory", editable: true, items: 10, updated_at: UPDATED },
          { name: "USER.md", view: "user", editable: true, items: 0, updated_at: null },
        ],
      });
    if (call.path.startsWith("/v1/memory/model")) return routes.model?.(call) ?? json(200, model({ state: "installed", installed: true, model_id: "potion@abc" }));
    if (call.path === "/v1/memory/rebuild") return routes.rebuild?.() ?? json(404, { code: "not_found", message: "nope" });
    if (call.path === "/v1/settings/config") return call.method === "PUT" && routes.put ? routes.put(call) : json(200, doc(CONFIG));
    return json(404, { code: "not_found", message: "nope" });
  });
  render(
    <ClientContext.Provider value={fake.client}>
      <ToastProvider>
        <MemoryConfigure />
      </ToastProvider>
    </ClientContext.Provider>,
  );
  return fake;
}

afterEach(() => vi.useRealTimers());

describe("Memory › Configure", () => {
  it("shows the status with the last update and count, and rebuilds to fresh counts", async () => {
    const user = userEvent.setup();
    let count = 10;
    let finish: ((response: Response) => void) | undefined;
    const { calls } = setup({
      health: () => health({ count }),
      rebuild: () => new Promise<Response>((resolve) => (finish = resolve)),
    });
    expect(await screen.findByText("Running normally")).toBeInTheDocument();
    expect(await screen.findByText("Last updated 2 hours ago, 10 memories")).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Search index" })).toHaveAttribute("aria-valuenow", "100");

    await user.click(screen.getByRole("button", { name: "Rebuild" }));
    expect(screen.getByRole("button", { name: "Rebuilding…" })).toBeDisabled();
    expect(screen.getByRole("progressbar", { name: "Search index" })).not.toHaveAttribute("aria-valuenow");
    count = 12;
    await act(async () => finish!(json(200, { reindexed: 12, embedded: 12, health: health({ count: 12 }) })));
    expect(await screen.findByText("Last updated 2 hours ago, 12 memories")).toBeInTheDocument();
    expect(screen.getByText("Rebuilt the search index for 12 memories.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Rebuild" })).toBeEnabled();
    expect(calls.filter((call) => call.method === "POST").map((call) => call.path)).toEqual(["/v1/memory/rebuild"]);
  });

  it("says when search runs on keywords only", async () => {
    setup({ health: () => health({ mode: "lexical_only", problem: "no local embedding model" }) });
    expect(await screen.findByText("Running with keyword search")).toBeInTheDocument();
  });

  it("shows memories as always on and writes retention to [memory] default_retention", async () => {
    const user = userEvent.setup();
    const { calls } = setup({ put: (call) => json(200, doc(call.body && typeof call.body === "object" && "config" in call.body ? call.body.config : null, true)) });
    const enable = screen.getByRole("switch", { name: "Enable memories" });
    expect(enable).toBeChecked();
    expect(enable).toBeDisabled();
    expect(screen.getByText(/can't be turned off yet/)).toBeInTheDocument();

    const retention = screen.getByRole("combobox", { name: "Episodic memory retention" });
    await waitFor(() => expect(retention).toHaveTextContent("Never forget"));
    await user.click(retention);
    expect((await screen.findAllByRole("option")).map((option) => option.textContent)).toEqual(["Never forget", "90 days", "30 days"]);
    await user.click(screen.getByRole("option", { name: "30 days" }));
    await waitFor(() => expect(calls.find((call) => call.method === "PUT")?.body).toEqual({ config: { ...CONFIG, memory: { ...CONFIG.memory, default_retention: "days_30" } } }));
    await waitFor(() => expect(retention).toHaveTextContent("30 days"));
    expect(screen.getByText("Restart openside to apply this change.")).toBeInTheDocument();
  });

  describe("search model row", () => {
    it("downloads on request, shows byte progress, then offers the restart that starts vector search", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      let started = false;
      let installed = false;
      const { calls } = setup({
        model: (call) => {
          if (call.method === "POST" && call.path === "/v1/memory/model/download") {
            started = true;
            return json(202, model({ state: "resolving" }));
          }
          if (!started) return json(200, model());
          if (!installed) return json(200, model({ state: "downloading", file: "model.safetensors", files_done: 0, files_total: 2, bytes_done: 5 * 1024 * 1024, bytes_total: 30 * 1024 * 1024 }));
          return json(200, model({ state: "installed", installed: true, model_id: "potion-base-8M@abc", restart_required: true }));
        },
      });
      const row = await screen.findByRole("region", { name: "Search model" });
      await within(row).findByText("Not installed");
      expect(row).toHaveTextContent("minishlab/potion-base-8M on huggingface.co");
      await user.click(within(row).getByRole("button", { name: "Download model" }));
      expect(calls.find((call) => call.method === "POST")?.body).toEqual({});

      await act(() => vi.advanceTimersByTimeAsync(1_000));
      const bar = await within(row).findByRole("progressbar", { name: "Model download" });
      expect(bar).toHaveAttribute("max", String(30 * 1024 * 1024));
      expect(bar).toHaveAttribute("value", String(5 * 1024 * 1024));
      expect(row).toHaveTextContent("5.0 MB of 30.0 MB");
      expect(row).toHaveTextContent("file 1 of 2");
      expect(within(row).getByRole("button", { name: "Cancel" })).toBeInTheDocument();

      installed = true;
      await act(() => vi.advanceTimersByTimeAsync(1_000));
      expect(await within(row).findByText("Installed")).toBeInTheDocument();
      expect(row).toHaveTextContent("potion-base-8M@abc");
      expect(within(row).getByRole("button", { name: "Restart daemon" })).toBeInTheDocument();
    });

    it("cancels a running download", async () => {
      const user = userEvent.setup();
      let cancelled = false;
      const { calls } = setup({
        model: (call) => {
          if (call.path === "/v1/memory/model/cancel") {
            cancelled = true;
            return json(200, model({ state: "cancelled" }));
          }
          return json(200, cancelled ? model({ state: "cancelled" }) : model({ state: "downloading", files_total: 2, bytes_done: 10, bytes_total: 100 }));
        },
      });
      const row = await screen.findByRole("region", { name: "Search model" });
      await user.click(await within(row).findByRole("button", { name: "Cancel" }));
      expect(await within(row).findByText("Cancelled")).toBeInTheDocument();
      expect(calls.some((call) => call.method === "POST" && call.path === "/v1/memory/model/cancel")).toBe(true);
      expect(within(row).getByText(/Nothing was installed/)).toBeInTheDocument();
      expect(within(row).getByRole("button", { name: "Try again" })).toBeInTheDocument();
    });

    it("does not let an older status poll overwrite a cancellation", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      const pendingPolls: ((response: Response) => void)[] = [];
      let cancel: ((response: Response) => void) | undefined;
      let initial = true;
      setup({
        model: (call) => {
          if (call.path === "/v1/memory/model/cancel") return new Promise<Response>((resolve) => (cancel = resolve));
          if (initial) {
            initial = false;
            return json(200, model({ state: "downloading" }));
          }
          return new Promise<Response>((resolve) => pendingPolls.push(resolve));
        },
      });
      const button = await screen.findByRole("button", { name: "Cancel" });
      await act(() => vi.advanceTimersByTimeAsync(1_000));
      expect(pendingPolls.length).toBeGreaterThan(0);
      await user.click(button);
      expect(cancel).toBeDefined();
      await act(async () => {
        cancel!(json(200, model({ state: "cancelled" })));
        for (const resolve of pendingPolls) resolve(json(200, model({ state: "downloading" })));
      });
      expect(screen.getByText("Cancelled")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
      expect(screen.queryByRole("progressbar", { name: "Model download" })).toBeNull();
    });

    it("says why a download failed in plain words and offers to try again", async () => {
      setup({ model: () => json(200, model({ state: "failed", error: { code: "egress_refused", message: "refused" } })) });
      const row = await screen.findByRole("region", { name: "Search model" });
      expect(await within(row).findByText("Failed")).toBeInTheDocument();
      expect(within(row).getByRole("alert")).toHaveTextContent("network allow-list does not include the model source");
      expect(within(row).getByRole("button", { name: "Try again" })).toBeInTheDocument();
    });

    it("hides the row for a device without the scope to read it", async () => {
      setup({ model: () => json(403, { code: "forbidden", message: "no" }) });
      await screen.findByText("Running normally");
      await waitFor(() => expect(screen.queryByRole("region", { name: "Search model" })).toBeNull());
    });
  });
});
