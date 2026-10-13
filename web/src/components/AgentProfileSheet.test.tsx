import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { describe, expect, it } from "vitest";
import type { Agent, HireDraft } from "../api/types";
import { ClientContext } from "../context";
import { agentSummary } from "../test/agents";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { ToastProvider } from "../ui";
import { AgentProfileSheet, formOf, profilePatch } from "./AgentProfileSheet";
import { mergeHireDraft } from "./TeamActions";

const mina: Agent = agentSummary("mina", "Mina", {
  model: "openai:gpt-test",
  skills: ["prices"],
  persona: { role: "Watches prices", voice: "Brief", responsibilities: ["Weekly price check"], boundaries: [] },
});

describe("profilePatch", () => {
  it("sends nothing when nothing changed", () => {
    expect(profilePatch(mina, formOf(mina))).toEqual({});
  });

  it("sends only the edited fields, and the persona whole", () => {
    const form = { ...formOf(mina), name: " Mina K ", responsibilities: "Weekly price check\n\n Monthly report " };
    expect(profilePatch(mina, form)).toEqual({
      name: "Mina K",
      persona: { role: "Watches prices", voice: "Brief", responsibilities: ["Weekly price check", "Monthly report"], boundaries: [] },
    });
  });

  it("uses null to go back to the defaults", () => {
    const form = { ...formOf(mina), model: "", skills: null };
    expect(profilePatch(mina, form)).toEqual({ model: null, skills: null });
  });

  it("sends a narrower account pool and a new ceiling", () => {
    const ref = { provider: "openai", source: "key", account_id: "k1" } as never;
    const form = { ...formOf(mina), accountPool: [ref], mode: "read-only" as const };
    expect(profilePatch(mina, form)).toEqual({ account_pool: [ref], mode: "read-only" });
  });
});

describe("profile sheet", () => {
  it("saves only what changed and says when it applies", async () => {
    const user = userEvent.setup();
    const daemon = fakeClient((call: Call) => {
      if (call.method === "PATCH") return json(200, { ...mina, ...(call.body as object) });
      if (call.path.startsWith("/v1/skills")) return json(200, { skills: [] });
      if (call.path.startsWith("/v1/routines")) return json(200, { routines: [] });
      if (call.path.startsWith("/v1/settings/channels")) return json(200, { accounts: [] });
      if (call.path.startsWith("/v1/settings/pools")) return json(200, { pools: [] });
      if (call.path.startsWith("/v1/connections")) return json(200, { connections: [], candidates: [] });
      if (call.path.startsWith("/v1/settings/models")) return json(200, { providers: [], default_model: null, fallbacks: [] });
      return json(404, { code: "not_found", message: "not found" });
    });
    render(
      <ClientContext.Provider value={daemon.client}>
        <ToastProvider>
          <MemoryRouter>
            <AgentProfileSheet agent={mina} open onClose={() => {}} onRetire={() => {}} />
          </MemoryRouter>
        </ToastProvider>
      </ClientContext.Provider>,
    );
    const role = screen.getByRole("textbox", { name: "Role" });
    await user.clear(role);
    await user.type(role, "Plans trips");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(daemon.calls.find((call) => call.method === "PATCH")).toBeDefined());
    const patch = daemon.calls.find((call) => call.method === "PATCH")!;
    expect(patch.path).toBe("/v1/agents/mina");
    expect(patch.body).toEqual({ persona: { role: "Plans trips", voice: "Brief", responsibilities: ["Weekly price check"], boundaries: [] } });
    expect(await screen.findByText(/uses it from the next message/)).toBeInTheDocument();
  });
});

describe("mergeHireDraft", () => {
  const draft = { name: "Mina", initials: "M", color: "#2f6fed", persona: { role: "Watches prices", voice: "", responsibilities: ["Weekly check"], boundaries: [] } } as unknown as HireDraft;

  it("keeps the proposal when nothing was edited", () => {
    expect(mergeHireDraft(draft, { name: "Mina", role: "Watches prices" })).toBe(draft);
    expect(mergeHireDraft(draft, { name: "  " })).toBe(draft);
    expect(mergeHireDraft(draft, undefined)).toBe(draft);
  });

  it("takes a new name and lets the daemon derive initials again", () => {
    expect(mergeHireDraft(draft, { name: " Noa " })).toEqual({ ...draft, name: "Noa", initials: null });
  });

  it("takes a new role into the persona and keeps the rest", () => {
    expect(mergeHireDraft(draft, { role: "Plans trips" })).toEqual({ ...draft, persona: { ...draft.persona, role: "Plans trips" } });
  });
});
