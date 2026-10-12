import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Ticket } from "../api/types";
import { ClientContext } from "../context";
import { fakeClient, json } from "../test/fakeDaemon";
import { TicketCard } from "./TicketCard";

function ticket(overrides: Partial<Ticket>): Ticket {
  return {
    id: "k1",
    session_id: "s1",
    tool_call_id: "call",
    deferral: "external",
    kind: "info",
    summary: "Need input",
    instructions: null,
    options: [],
    origin: { subagent_id: null },
    tool: null,
    target: null,
    resume_when: null,
    created_at: "2026-01-01T00:00:00Z",
    expires_at: "2026-01-02T00:00:00Z",
    status: "pending",
    resolution: null,
    ...overrides,
  };
}

function setup(value: Ticket, status = 200) {
  const daemon = fakeClient(() =>
    status === 200 ? json(200, { ...value, status: "resolved" }) : json(status, { code: "secret_input_unavailable", message: "x" }),
  );
  render(
    <ClientContext.Provider value={daemon.client}>
      <TicketCard ticket={value} />
    </ClientContext.Provider>,
  );
  return daemon;
}

const SECRET = "hunter2-canary";

describe("TicketCard", () => {
  it("sends a secret once as a password field and clears it without rendering it", async () => {
    const user = userEvent.setup();
    const { calls } = setup(ticket({ kind: "secret_input", summary: "Bank password", target: { origin: "https://bank.test", tab_id: "t", field_ref: "e1" } }));
    const input = screen.getByLabelText("Secret value");
    expect(input).toHaveAttribute("type", "password");
    expect(input).toHaveAttribute("autocomplete", "off");
    await user.type(input, SECRET);
    await user.click(screen.getByRole("button", { name: "Submit" }));
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ method: "POST", path: "/v1/tickets/k1/resolve", body: { value: SECRET } });
    expect(input).toHaveValue("");
    expect(document.body.innerHTML).not.toContain(SECRET);
    // Submitting the now-empty field sends nothing more.
    await user.click(screen.getByRole("button", { name: "Submit" }));
    expect(calls).toHaveLength(1);
  });

  it("clears the secret even when the daemon refuses it", async () => {
    const user = userEvent.setup();
    setup(ticket({ kind: "secret_input" }), 501);
    const input = screen.getByLabelText("Secret value");
    await user.type(input, SECRET);
    await user.click(screen.getByRole("button", { name: "Submit" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/not available/);
    expect(input).toHaveValue("");
    expect(document.body.innerHTML).not.toContain(SECRET);
  });

  it("approves or rejects confirm tickets and shows the redacted tool call", async () => {
    const user = userEvent.setup();
    const { calls } = setup(ticket({ kind: "confirm", deferral: "approval", tool: { name: "bash", args: { command: "ls" } } }));
    expect(screen.getByText("bash")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Approve" }));
    await user.click(screen.getByRole("button", { name: "Reject" }));
    expect(calls.map((c) => c.body)).toEqual([{ value: true }, { value: false }]);
  });

  it("gives a short pulse for an approval and a double pulse for a rejection", async () => {
    const vibrate = vi.fn(() => true);
    Object.defineProperty(navigator, "vibrate", { value: vibrate, configurable: true });
    try {
      const user = userEvent.setup();
      setup(ticket({ kind: "confirm", deferral: "approval", tool: { name: "bash", args: { command: "ls" } } }));
      await user.click(screen.getByRole("button", { name: "Approve" }));
      await user.click(screen.getByRole("button", { name: "Reject" }));
      expect(vibrate.mock.calls).toEqual([[[14]], [[10, 40, 10]]]);
    } finally {
      Reflect.deleteProperty(navigator, "vibrate");
    }
  });

  it("answers info tickets with an option or free text", async () => {
    const user = userEvent.setup();
    const { calls } = setup(ticket({ options: [{ value: "blue", label: "Blue" }] }));
    await user.click(screen.getByRole("button", { name: "Blue" }));
    expect(calls[0]).toMatchObject({ path: "/v1/tickets/k1/resolve", body: { value: "blue" } });
  });

  it("shows terminal outcomes without action buttons", () => {
    setup(ticket({ kind: "secret_input", status: "resolved", resolution: { status: "filled" } }));
    expect(screen.getByText("Secret submitted")).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("names the subagent that raised it by task, falling back to a short id", () => {
    const id = "0123456789abcdef";
    const raised = ticket({ kind: "confirm", origin: { subagent_id: id } });
    const daemon = fakeClient(() => json(200, raised));
    const { rerender } = render(
      <ClientContext.Provider value={daemon.client}>
        <TicketCard ticket={raised} subagentTask="Draft the report" />
      </ClientContext.Provider>,
    );
    expect(screen.getByText("Draft the report")).toHaveAttribute("data-subagent-id", id);
    rerender(
      <ClientContext.Provider value={daemon.client}>
        <TicketCard ticket={raised} />
      </ClientContext.Provider>,
    );
    expect(screen.getByText("Subagent 01234567")).toHaveAttribute("data-subagent-id", id);
  });

  it("asks for a code in place with the shared form and registers the authenticator as {totp}", async () => {
    const user = userEvent.setup();
    const KEY = "otpauth://totp/Bank:me?secret=JBSWY3DPEHPK3PXP";
    const { calls } = setup(
      ticket({
        kind: "secret_input",
        summary: "Verification code for https://bank.test",
        target: { origin: "https://bank.test", tab_id: "t", field_ref: "e1" },
        otp: { channel: "authenticator", can_register: true },
      }),
    );
    expect(screen.getByRole("region", { name: "Verification code" })).toBeInTheDocument();
    const code = screen.getByLabelText("Code");
    expect(code).toHaveFocus();
    expect(code).toHaveAttribute("autocomplete", "one-time-code");
    expect(screen.getByText("https://bank.test")).toBeInTheDocument();
    await user.click(screen.getByText("Save this site's authenticator"));
    const key = screen.getByLabelText("Setup key or otpauth:// link");
    await user.type(key, KEY);
    await user.click(screen.getByRole("button", { name: "Save and fill" }));
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ method: "POST", path: "/v1/tickets/k1/resolve", body: { value: { totp: KEY } } });
    expect(key).toHaveValue("");
    expect(document.body.innerHTML).not.toContain("JBSWY3DPEHPK3PXP");
  });

  it("offers saving the authenticator only to a device paired with approve", () => {
    const otp = ticket({
      kind: "secret_input",
      target: { origin: "https://bank.test", tab_id: "t", field_ref: "e1" },
      otp: { channel: "authenticator", can_register: true },
    });
    localStorage.setItem("openside.scopes", JSON.stringify(["read", "human_step"]));
    setup(otp);
    expect(screen.getByLabelText("Code")).toBeInTheDocument();
    expect(screen.queryByText("Save this site's authenticator")).toBeNull();
    cleanup();
    localStorage.setItem("openside.scopes", JSON.stringify(["read", "approve", "human_step"]));
    setup(otp);
    expect(screen.getByText("Save this site's authenticator")).toBeInTheDocument();
    localStorage.removeItem("openside.scopes");
  });

  it("sends an emailed code once on Enter and clears it", async () => {
    const user = userEvent.setup();
    const { calls } = setup(
      ticket({ kind: "secret_input", target: { origin: "https://bank.test", tab_id: "t", field_ref: "e1" }, otp: { channel: "email", can_register: false } }),
    );
    expect(screen.queryByText("Save this site's authenticator")).toBeNull();
    const code = screen.getByLabelText("Code");
    await user.type(code, "Q7K2XM{Enter}");
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ body: { value: "Q7K2XM" } });
    expect(code).toHaveValue("");
    expect(document.body.innerHTML).not.toContain("Q7K2XM");
  });

  it("asks to save an authenticator found during setup without showing the key", async () => {
    const user = userEvent.setup();
    const register = {
      ...ticket({ kind: "confirm", target: { origin: "https://bank.test", tab_id: "t", field_ref: null } }),
      otp_register: { origin: "https://bank.test", issuer: "Bank", account: "me@bank.test" },
    };
    const { calls } = setup(register);
    const card = screen.getByRole("region", { name: "Save authenticator" });
    expect(card).toHaveTextContent("Save the authenticator for bank.test (Bank · me@bank.test) to your vault?");
    expect(card).toHaveTextContent("bank.test offered to set up two-step sign-in");
    await user.click(screen.getByRole("button", { name: "Save to vault" }));
    await user.click(screen.getByRole("button", { name: "Not now" }));
    expect(calls.map((call) => call.body)).toEqual([{ value: true }, { value: false }]);
  });

  it("reports a saved or declined authenticator", () => {
    const resolved = (resolution: unknown) => ({
      ...ticket({ kind: "confirm", status: "resolved", resolution }),
      otp_register: { origin: "https://bank.test", issuer: null, account: null },
    });
    const { unmount } = render(
      <ClientContext.Provider value={fakeClient(() => json(200, {})).client}>
        <TicketCard ticket={resolved({ status: "registered", filled: true })} />
      </ClientContext.Provider>,
    );
    expect(screen.getByText("Authenticator saved")).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
    unmount();
    render(
      <ClientContext.Provider value={fakeClient(() => json(200, {})).client}>
        <TicketCard ticket={resolved({ approved: false, status: "declined" })} />
      </ClientContext.Provider>,
    );
    expect(screen.getByText("Declined")).toBeInTheDocument();
  });
});
