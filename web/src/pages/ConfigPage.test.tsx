import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { SettingsDocument } from "../api/types";
import { ClientContext } from "../context";
import { remoteEditable, schemaFields } from "../settings/schemaForm";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { ConfigPage } from "./ConfigPage";

/** Shaped like `openside_config::config_schema()` (schemars: $defs, $ref, oneOf consts, nullable types). */
const SCHEMA = {
  type: "object",
  additionalProperties: false,
  properties: {
    agent: { $ref: "#/$defs/AgentConfig", description: "Python sidecar launch settings." },
    credentials: { type: "object", description: "Opaque credential sources." },
    memory: { $ref: "#/$defs/MemoryConfig" },
    models: { $ref: "#/$defs/ModelsConfig" },
    tickets: { $ref: "#/$defs/TicketsConfig" },
  },
  $defs: {
    AgentConfig: {
      type: "object",
      properties: {
        command: { type: ["array", "null"], items: { type: "string" }, description: "Sidecar argv." },
        startup_timeout_ms: { type: "integer", format: "uint64", minimum: 0, description: "Startup deadline." },
      },
    },
    MemoryConfig: {
      type: "object",
      properties: {
        default_retention: { $ref: "#/$defs/Retention", description: "Retention for notes." },
        history: { type: "boolean", description: "Index visited pages." },
        embedding: { $ref: "#/$defs/EmbeddingConfig" },
      },
    },
    EmbeddingConfig: { type: "object", properties: { model_dir: { type: ["string", "null"], description: "Local model directory." } } },
    Retention: {
      oneOf: [
        { const: "permanent", type: "string" },
        { const: "days_90", type: "string" },
        { const: "days_30", type: "string" },
      ],
    },
    ModelsConfig: {
      type: "object",
      properties: {
        default: { type: ["string", "null"], description: "Default model." },
        fallback: { type: "array", items: { type: "string" }, description: "Ordered fallback chain." },
      },
    },
    TicketsConfig: {
      type: "object",
      properties: {
        confirm_ttl_secs: { type: "integer", minimum: 1, description: "Confirmation expiry." },
        takeover_ttl_secs: { type: "integer", minimum: 1, description: "Takeover expiry." },
      },
    },
  },
};

const CONFIG = {
  agent: { command: ["python", "-m", "openside_agent"], startup_timeout_ms: 30000 },
  memory: { default_retention: "permanent", history: true, embedding: { model_dir: null } },
  models: { default: "script", fallback: [] },
  tickets: { confirm_ttl_secs: 600, takeover_ttl_secs: 900 },
};

const REMOTE_EDITABLE = ["tickets.", "models.default", "models.fallback", "memory.default_retention", "memory.history"];

/** Mirrors the daemon rule: a remote PUT may only differ in remote-editable leaves. */
function daemon(remote: boolean) {
  let stored = structuredClone(CONFIG) as Record<string, unknown>;
  const document = (): SettingsDocument => ({
    config: stored,
    file_only: ["credentials"],
    restart_required: JSON.stringify(stored) !== JSON.stringify(CONFIG),
    schema: SCHEMA,
    remote_editable: REMOTE_EDITABLE,
    remote,
  });
  const leaves = (value: unknown, prefix = ""): string[] =>
    value && typeof value === "object" && !Array.isArray(value)
      ? Object.entries(value).flatMap(([key, child]) => leaves(child, prefix ? `${prefix}.${key}` : key))
      : [`${prefix}=${JSON.stringify(value)}`];
  return fakeClient((call: Call) => {
    if (call.method === "GET") return json(200, document());
    const next = (call.body as { config: Record<string, unknown> }).config;
    const before = leaves(stored);
    const after = leaves(next);
    const changed = [...before.filter((l) => !after.includes(l)), ...after.filter((l) => !before.includes(l))].map((l) => l.split("=")[0]!);
    if (remote && changed.some((key) => !remoteEditable(key, REMOTE_EDITABLE))) {
      return json(403, { code: "local_socket_required", message: "local_socket_required" });
    }
    stored = next;
    return json(200, document());
  });
}

function setup(remote: boolean) {
  const fake = daemon(remote);
  render(
    <ClientContext.Provider value={fake.client}>
      <ConfigPage />
    </ClientContext.Provider>,
  );
  return fake;
}

const field = (key: string) => document.querySelector(`[data-key="${key}"]`) as HTMLElement;

describe("schemaFields", () => {
  it("flattens every key of the schema with its control kind, hiding file-only sections", () => {
    const fields = schemaFields(SCHEMA, ["credentials"]);
    expect(fields.map((f) => [f.path, f.kind, f.nullable])).toEqual([
      ["agent.command", "list", true],
      ["agent.startup_timeout_ms", "integer", false],
      ["memory.default_retention", "enum", false],
      ["memory.history", "boolean", false],
      ["memory.embedding.model_dir", "string", true],
      ["models.default", "string", true],
      ["models.fallback", "list", false],
      ["tickets.confirm_ttl_secs", "integer", false],
      ["tickets.takeover_ttl_secs", "integer", false],
    ]);
    expect(fields.find((f) => f.path === "memory.default_retention")?.options).toEqual(["permanent", "days_90", "days_30"]);
    expect(fields.find((f) => f.path === "agent.command")?.description).toBe("Sidecar argv.");
  });
});

describe("ConfigPage", () => {
  it("over TCP edits an allowed key and saves only that change; local-only keys are read-only", async () => {
    const user = userEvent.setup();
    const { calls } = setup(true);
    expect(await screen.findByRole("note")).toHaveTextContent("paired device");
    expect(within(field("agent.command")).getByRole("textbox")).toBeDisabled();
    expect(within(field("agent.command")).getByText("local only")).toBeInTheDocument();
    expect(field("credentials.x")).toBeNull();

    const ttl = within(field("tickets.confirm_ttl_secs")).getByRole("spinbutton");
    expect(ttl).toBeEnabled();
    await user.clear(ttl);
    await user.type(ttl, "1200");
    await user.selectOptions(within(field("memory.default_retention")).getByRole("combobox"), "days_90");
    await user.click(screen.getByRole("button", { name: "Save (2)" }));

    expect(await screen.findByRole("status")).toHaveTextContent("apply after the daemon restarts");
    const put = calls.find((c) => c.method === "PUT")!.body as { config: typeof CONFIG };
    expect(put.config).toEqual({
      ...CONFIG,
      tickets: { ...CONFIG.tickets, confirm_ttl_secs: 1200 },
      memory: { ...CONFIG.memory, default_retention: "days_90" },
    });
    expect(within(field("tickets.confirm_ttl_secs")).getByRole("spinbutton")).toHaveValue(1200);
  });

  it("refuses a local-only key edited as JSON over TCP with the local-socket message", async () => {
    const user = userEvent.setup();
    setup(true);
    await screen.findByRole("note");
    await user.click(screen.getByRole("tab", { name: "JSON" }));
    const editor = screen.getByLabelText("Configuration document (JSON form of config.toml)");
    const changed = { ...CONFIG, agent: { ...CONFIG.agent, command: ["/tmp/evil"] } };
    await user.clear(editor);
    await user.click(editor);
    await user.paste(JSON.stringify(changed));
    await user.click(screen.getByRole("button", { name: "Save JSON" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Only the computer running openside can make this change (local socket); paired devices cannot.",
    );
  });

  it.each(["before", "after"] as const)("preserves the JSON draft and refusal when an earlier form response arrives %s it", async (order) => {
    const user = userEvent.setup();
    let releaseForm!: (response: Response) => void;
    let releaseJson!: (response: Response) => void;
    let saves = 0;
    const document = (config = CONFIG): SettingsDocument => ({
      config,
      file_only: ["credentials"],
      restart_required: JSON.stringify(config) !== JSON.stringify(CONFIG),
      schema: SCHEMA,
      remote_editable: REMOTE_EDITABLE,
      remote: true,
    });
    const fake = fakeClient((call) => {
      if (call.method === "GET") return json(200, document());
      return new Promise<Response>((resolve) => {
        if (++saves === 1) releaseForm = resolve;
        else releaseJson = resolve;
      });
    });
    render(
      <ClientContext.Provider value={fake.client}>
        <ConfigPage />
      </ClientContext.Provider>,
    );
    await screen.findByRole("note");
    const ttl = within(field("tickets.confirm_ttl_secs")).getByRole("spinbutton");
    await user.clear(ttl);
    await user.type(ttl, "1234");
    await user.click(screen.getByRole("button", { name: "Save (1)" }));
    await waitFor(() => expect(fake.calls.filter((call) => call.method === "PUT")).toHaveLength(1));
    await user.click(screen.getByRole("tab", { name: "JSON" }));
    const editor = screen.getByLabelText("Configuration document (JSON form of config.toml)");
    const changed = { ...CONFIG, agent: { ...CONFIG.agent, command: ["/tmp/evil"] } };
    await user.clear(editor);
    await user.click(editor);
    await user.paste(JSON.stringify(changed));
    await user.click(screen.getByRole("button", { name: "Save JSON" }));
    await waitFor(() => expect(fake.calls.filter((call) => call.method === "PUT")).toHaveLength(2));
    const completeForm = async () => {
      await act(async () => releaseForm(json(200, document({ ...CONFIG, tickets: { ...CONFIG.tickets, confirm_ttl_secs: 1234 } }))));
      expect(screen.queryByRole("status")).toBeNull();
      expect(screen.getByLabelText("Configuration document (JSON form of config.toml)")).toHaveValue(JSON.stringify(changed));
    };
    if (order === "before") await completeForm();
    await act(async () => releaseJson(json(403, { code: "local_socket_required", message: "local_socket_required" })));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Only the computer running openside can make this change (local socket); paired devices cannot.",
    );
    if (order === "after") await completeForm();
    expect(screen.getByRole("alert")).toHaveTextContent("Only the computer running openside can make this change");
    expect(screen.getByLabelText("Configuration document (JSON form of config.toml)")).toBe(editor);
  });

  it.each([false, true])("ignores an older JSON response after a newer edit (saved: %s)", async (saved) => {
    const user = userEvent.setup();
    const pending: Array<(response: Response) => void> = [];
    const document = (config = CONFIG): SettingsDocument => ({
      config,
      file_only: ["credentials"],
      restart_required: JSON.stringify(config) !== JSON.stringify(CONFIG),
      schema: SCHEMA,
      remote_editable: REMOTE_EDITABLE,
      remote: true,
    });
    const fake = fakeClient((call) => call.method === "GET"
      ? json(200, document())
      : new Promise<Response>((resolve) => pending.push(resolve)));
    render(
      <ClientContext.Provider value={fake.client}>
        <ConfigPage />
      </ClientContext.Provider>,
    );
    await screen.findByRole("note");
    await user.click(screen.getByRole("tab", { name: "JSON" }));
    const editor = screen.getByLabelText("Configuration document (JSON form of config.toml)");
    const first = { ...CONFIG, tickets: { ...CONFIG.tickets, confirm_ttl_secs: 1234 } };
    const latest = { ...CONFIG, tickets: { ...CONFIG.tickets, confirm_ttl_secs: 2345 } };
    for (const next of [first, latest]) {
      await user.clear(editor);
      await user.click(editor);
      await user.paste(JSON.stringify(next));
      if (next === first || saved) await user.click(screen.getByRole("button", { name: "Save JSON" }));
    }
    await waitFor(() => expect(pending).toHaveLength(saved ? 2 : 1));
    if (saved) await act(async () => pending[1]!(json(200, document(latest))));
    await act(async () => pending[0]!(json(200, document(first))));
    expect(JSON.parse((screen.getByLabelText("Configuration document (JSON form of config.toml)") as HTMLTextAreaElement).value)).toEqual(latest);
    expect(screen.getByLabelText("Configuration document (JSON form of config.toml)")).toBe(editor);
    if (!saved) {
      expect(screen.queryByRole("status")).toBeNull();
      await user.click(screen.getByRole("button", { name: "Save JSON" }));
      await waitFor(() => expect(pending).toHaveLength(2));
      await act(async () => pending[1]!(json(200, document(latest))));
    }
    await user.click(screen.getByRole("tab", { name: "Form" }));
    expect(within(field("tickets.confirm_ttl_secs")).getByRole("spinbutton")).toHaveValue(2345);
  });
  it("over the local socket every key is editable and a missing number is caught before sending", async () => {
    const user = userEvent.setup();
    const { calls } = setup(false);
    const command = within(await screen.findByRole("form", { name: "Configuration" }).then(() => field("agent.command"))).getByRole("textbox");
    expect(command).toBeEnabled();
    expect(screen.queryByRole("note")).toBeNull();
    const ttl = within(field("tickets.confirm_ttl_secs")).getByRole("spinbutton");
    await user.clear(ttl);
    await user.click(screen.getByRole("button", { name: "Save (1)" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("tickets.confirm_ttl_secs needs a value.");
    expect(calls.some((c) => c.method === "PUT")).toBe(false);
  });
});
