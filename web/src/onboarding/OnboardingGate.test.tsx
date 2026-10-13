import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Navigate, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Client } from "../api/client";
import { savePairing } from "../auth";
import { ClientContext } from "../context";
import { CHIEF_ID } from "../hooks/useAgents";
import { json } from "../test/fakeDaemon";
import { dismissGuide } from "./dismissed";
import { Landing, OnboardingGate } from "./OnboardingGate";
import { onboardingDaemon } from "./testDaemon";

function shell(client: Client, at: string) {
  return render(
    <ClientContext.Provider value={client}>
      <MemoryRouter initialEntries={[at]}>
        <OnboardingGate />
        <Routes>
          <Route path="/welcome" element={<p>guide</p>} />
          <Route path="*" element={<p>app</p>} />
        </Routes>
      </MemoryRouter>
    </ClientContext.Provider>,
  );
}

function landing(client: Client, at = "/") {
  return render(
    <ClientContext.Provider value={client}>
      <MemoryRouter initialEntries={[at]}>
        <OnboardingGate>
          <Routes>
            <Route index element={<Landing onRoot />} />
            <Route path="/welcome" element={<p>guide</p>} />
            <Route path="/agents/:id" element={<p>chief</p>} />
          </Routes>
        </OnboardingGate>
      </MemoryRouter>
    </ClientContext.Provider>,
  );
}

beforeEach(() => localStorage.clear());
afterEach(cleanup);

describe("OnboardingGate", () => {
  it("opens the guide on a new daemon", async () => {
    shell(onboardingDaemon().client, "/");
    expect(await screen.findByText("guide")).toBeInTheDocument();
  });

  it("still opens the guide when `/` has already redirected to the chief", async () => {
    render(
      <ClientContext.Provider value={onboardingDaemon().client}>
        <MemoryRouter initialEntries={["/"]}>
          <OnboardingGate />
          <Routes>
            <Route index element={<Navigate to={`/agents/${CHIEF_ID}`} replace />} />
            <Route path="/welcome" element={<p>guide</p>} />
            <Route path="*" element={<p>app</p>} />
          </Routes>
        </MemoryRouter>
      </ClientContext.Provider>,
    );
    expect(await screen.findByText("guide")).toBeInTheDocument();
  });

  it("does not pull a person who moved on from the landing back to the guide", async () => {
    const daemon = onboardingDaemon();
    shell(daemon.client, "/sessions/s1");
    expect(screen.getByText("app")).toBeInTheDocument();
  });

  it.each(["completed", "skipped"] as const)("leaves the app alone once the guide is %s", async (state) => {
    const daemon = onboardingDaemon({ onboarding: state });
    shell(daemon.client, "/");
    await waitFor(() => expect(daemon.calls).toHaveLength(1));
    expect(screen.getByText("app")).toBeInTheDocument();
  });

  it("does not interrupt a deep link", () => {
    const daemon = onboardingDaemon();
    shell(daemon.client, "/sessions/s1");
    expect(daemon.calls).toHaveLength(0);
    expect(screen.getByText("app")).toBeInTheDocument();
  });

  it("does not ask a device that is known to lack the admin scope", () => {
    savePairing("token", "d1", ["read", "approve"]);
    const daemon = onboardingDaemon();
    shell(daemon.client, "/");
    expect(daemon.calls).toHaveLength(0);
  });

  it("asks a device whose scopes are unknown (pasted token)", async () => {
    const daemon = onboardingDaemon();
    shell(daemon.client, "/");
    await screen.findByText("guide");
    expect(daemon.calls[0]).toMatchObject({ method: "GET", path: "/v1/onboarding" });
  });

  it("does not reopen after this device could not record the outcome", () => {
    dismissGuide();
    const daemon = onboardingDaemon();
    shell(daemon.client, "/");
    expect(daemon.calls).toHaveLength(0);
  });

  it("stays out of the way when the daemon cannot answer", async () => {
    const daemon = onboardingDaemon({ override: () => json(503, { code: "unavailable", message: "down" }) });
    shell(daemon.client, "/");
    await waitFor(() => expect(daemon.calls).toHaveLength(1));
    expect(screen.getByText("app")).toBeInTheDocument();
  });

  describe("the landing route", () => {
    it("holds the chief back until the daemon answers, then opens the guide instead", async () => {
      landing(onboardingDaemon().client);
      expect(screen.queryByText("chief")).not.toBeInTheDocument();
      expect(await screen.findByText("guide")).toBeInTheDocument();
      expect(screen.queryByText("chief")).not.toBeInTheDocument();
    });

    it.each(["completed", "skipped"] as const)("goes to the chief once the guide is %s", async (state) => {
      landing(onboardingDaemon({ onboarding: state }).client);
      expect(await screen.findByText("chief")).toBeInTheDocument();
    });

    it("goes to the chief when the daemon cannot answer", async () => {
      landing(onboardingDaemon({ override: () => json(503, { code: "unavailable", message: "down" }) }).client);
      expect(await screen.findByText("chief")).toBeInTheDocument();
    });

    it("goes straight to the chief without asking when this device cannot record the outcome", async () => {
      savePairing("token", "d1", ["read", "approve"]);
      const daemon = onboardingDaemon();
      landing(daemon.client);
      expect(await screen.findByText("chief")).toBeInTheDocument();
      expect(daemon.calls).toHaveLength(0);
    });
  });
});
