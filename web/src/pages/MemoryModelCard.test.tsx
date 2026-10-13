import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { MemoryModelStatus } from "../api/types";
import { ClientContext } from "../context";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { MemoryModelCard } from "./MemoryModelCard";

const SOURCE = { endpoint: "https://huggingface.co", repo: "minishlab/potion-base-8M", revision: "main" };

function status(patch: Partial<MemoryModelStatus> = {}): MemoryModelStatus {
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

function setup(handler: (call: Call) => Response | undefined) {
  const fake = fakeClient((call) => handler(call) ?? json(404, { code: "not_found", message: "nope" }));
  const changed = vi.fn();
  render(
    <ClientContext.Provider value={fake.client}>
      <MemoryModelCard onChanged={changed} pollMs={20} />
    </ClientContext.Provider>,
  );
  return { ...fake, changed };
}

describe("MemoryModelCard", () => {
  it("downloads on request, shows byte progress, then offers the restart that starts vector search", async () => {
    const user = userEvent.setup();
    let polls = 0;
    let started = false;
    const { calls, changed } = setup((call) => {
      if (call.method === "POST" && call.path === "/v1/memory/model/download") {
        started = true;
        return json(202, status({ state: "resolving" }));
      }
      if (call.path === "/v1/memory/model") {
        if (!started) return json(200, status());
        polls += 1;
        if (polls === 1) return json(200, status({ state: "downloading", file: "model.safetensors", files_done: 0, files_total: 2, bytes_done: 5 * 1024 * 1024, bytes_total: 30 * 1024 * 1024 }));
        return json(200, status({ state: "installed", installed: true, model_id: "potion-base-8M@abc", restart_required: true }));
      }
      return undefined;
    });

    const card = await screen.findByRole("region", { name: "Search model" });
    expect(card).toHaveTextContent("Not installed");
    expect(card).toHaveTextContent("minishlab/potion-base-8M on huggingface.co");
    await user.click(screen.getByRole("button", { name: "Download model" }));
    expect(calls.find((call) => call.method === "POST")?.body).toEqual({});

    const bar = await screen.findByRole("progressbar", { name: "Model download" });
    await waitFor(() => expect(bar).toHaveAttribute("max", String(30 * 1024 * 1024)));
    expect(bar).toHaveAttribute("value", String(5 * 1024 * 1024));
    expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
    expect(card).toHaveTextContent("5.0 MB of 30.0 MB");
    expect(card).toHaveTextContent("file 1 of 2");

    expect(await screen.findByText("Installed")).toBeInTheDocument();
    expect(card).toHaveTextContent("potion-base-8M@abc");
    expect(screen.getByRole("button", { name: "Restart daemon" })).toBeInTheDocument();
    await waitFor(() => expect(changed).toHaveBeenCalled());
  });

  it("cancels a running download", async () => {
    const user = userEvent.setup();
    const { calls } = setup((call) => {
      if (call.method === "POST" && call.path === "/v1/memory/model/cancel") return json(200, status({ state: "cancelled" }));
      if (call.path === "/v1/memory/model") return json(200, status({ state: "downloading", files_total: 2, bytes_done: 10, bytes_total: 100 }));
      return undefined;
    });
    await user.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("Cancelled")).toBeInTheDocument();
    expect(calls.some((call) => call.method === "POST" && call.path === "/v1/memory/model/cancel")).toBe(true);
    expect(screen.getByText(/Nothing was installed/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
  });

  it("says why a download failed in plain words and offers to try again", async () => {
    setup((call) =>
      call.path === "/v1/memory/model"
        ? json(200, status({ state: "failed", error: { code: "egress_refused", message: "refused" } }))
        : undefined,
    );
    expect(await screen.findByText("Failed")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("network allow-list does not include the model source");
    expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
  });

  it("shows nothing for a device without the scope to read it", async () => {
    setup((call) => (call.path === "/v1/memory/model" ? json(403, { code: "forbidden", message: "no" }) : undefined));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Search model" })).toBeNull());
  });
});
