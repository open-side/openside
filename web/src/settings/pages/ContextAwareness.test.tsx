import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ClientContext } from "../../context";
import { fakeClient, json, type Call } from "../../test/fakeDaemon";
import { ToastProvider } from "../../ui";
import { ContextAwareness } from "./ContextAwareness";

const CONFIG = {
  memory: { history: false, default_retention: "permanent", embedding: { model_dir: "/data/model" } },
  models: { default: "anthropic/x" },
};

function doc(config: unknown, restart = false) {
  return { config, restart_required: restart, file_only: ["credentials"], remote: false, remote_editable: [], schema: {} };
}

function setup(put: (call: Call) => Response) {
  const fake = fakeClient((call) => {
    if (call.path !== "/v1/settings/config") return json(404, { code: "not_found", message: "nope" });
    return call.method === "PUT" ? put(call) : json(200, doc(CONFIG));
  });
  render(
    <ClientContext.Provider value={fake.client}>
      <ToastProvider>
        <ContextAwareness />
      </ToastProvider>
    </ClientContext.Provider>,
  );
  return fake;
}

describe("Memory › Context Awareness", () => {
  it("turns page capture on through [memory] history, keeping the rest of the config, then asks for a restart", async () => {
    const user = userEvent.setup();
    const { calls } = setup((call) => json(200, doc(call.body && typeof call.body === "object" && "config" in call.body ? call.body.config : null, true)));
    expect(screen.getByRole("heading", { name: "openside remembers what you browse" })).toBeInTheDocument();
    expect(screen.getByText(/Activity in other apps isn't captured yet/)).toBeInTheDocument();

    const toggle = screen.getByRole("switch", { name: "Enable Context Awareness" });
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(toggle).not.toBeChecked();
    await user.click(toggle);

    expect(calls.find((call) => call.method === "PUT")?.body).toEqual({
      config: { ...CONFIG, memory: { ...CONFIG.memory, history: true } },
    });
    await waitFor(() => expect(toggle).toBeChecked());
    expect(screen.getByText("Restart openside to apply this change.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Restart daemon" })).toBeInTheDocument();
  });

  it("keeps the old value and says why when the daemon refuses", async () => {
    const user = userEvent.setup();
    setup(() => json(403, { code: "local_access_required", message: "Only the computer can change this." }));
    const toggle = screen.getByRole("switch", { name: "Enable Context Awareness" });
    await waitFor(() => expect(toggle).toBeEnabled());
    await user.click(toggle);
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(toggle).not.toBeChecked();
  });
});
