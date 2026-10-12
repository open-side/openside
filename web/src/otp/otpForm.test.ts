import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api/client";
import type { Ticket } from "../api/types";
import { mountOtpForm, otpRegistrationOf, registrationQuestion, type OtpTicket } from "./otpForm";

function otpTicket(channel: NonNullable<Ticket["otp"]>["channel"], canRegister = false): OtpTicket {
  return {
    id: "k1",
    session_id: "s1",
    tool_call_id: "otp",
    deferral: "external",
    kind: "secret_input",
    summary: "Code for the sign-in",
    instructions: null,
    options: [],
    origin: { subagent_id: null },
    tool: null,
    target: { origin: "https://www.example.com", tab_id: "t", field_ref: "e1" },
    resume_when: null,
    created_at: "2026-01-01T00:00:00Z",
    expires_at: "2026-01-02T00:00:00Z",
    status: "pending",
    resolution: null,
    otp: { channel, can_register: canRegister },
  };
}

function mount(ticket: OtpTicket, options: { focus?: "polite" | "force"; fail?: boolean } = {}) {
  const host = document.createElement("div");
  document.body.append(host);
  const client = {
    resolveTicket: vi.fn(async () => {
      if (options.fail) throw new ApiError(409, "secret_input_failed", "The code could not be entered.");
      return { ...ticket, status: "resolved" } as Ticket;
    }),
    rejectTicket: vi.fn(async () => ({ ...ticket, status: "rejected" }) as Ticket),
  };
  const onSubmitted = vi.fn();
  const cleanup = mountOtpForm(host, { ticket, client, focus: options.focus, buttonClass: (variant) => `button ${variant}`, onSubmitted });
  return { client, onSubmitted, cleanup };
}

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("mountOtpForm", () => {
  it("is one focused one-time-code field naming the site and channel; Enter submits and the value is gone", async () => {
    const user = userEvent.setup();
    const { client, onSubmitted } = mount(otpTicket("sms"));
    const input = screen.getByLabelText("Code");
    expect(screen.getByRole("form", { name: "Verification code" })).toBeInTheDocument();
    expect(input).toHaveFocus();
    expect(input).toHaveAttribute("autocomplete", "one-time-code");
    expect(input).toHaveAttribute("inputmode", "numeric");
    expect(screen.getByText("example.com")).toBeInTheDocument();
    expect(input).toHaveAccessibleDescription("Code from a text message for example.com");

    await user.type(input, "482913{Enter}");
    expect(client.resolveTicket).toHaveBeenCalledExactlyOnceWith("k1", "482913");
    expect(input).toHaveValue("");
    expect(document.body.innerHTML).not.toContain("482913");
    await vi.waitFor(() => expect(onSubmitted).toHaveBeenCalledOnce());
  });

  it("takes a whole pasted code, dropping the separators sites print", async () => {
    const user = userEvent.setup();
    const { client } = mount(otpTicket("authenticator"));
    await user.click(screen.getByLabelText("Code"));
    await user.paste("123 456");
    await user.click(screen.getByRole("button", { name: "Submit" }));
    expect(client.resolveTicket).toHaveBeenCalledWith("k1", "123456");
  });

  it("keeps letters for an emailed code and offers no authenticator", async () => {
    const user = userEvent.setup();
    const { client } = mount(otpTicket("email"));
    const input = screen.getByLabelText("Code");
    expect(input).toHaveAttribute("inputmode", "text");
    expect(input).toHaveAccessibleDescription("Code from your email for example.com");
    expect(screen.queryByText("Save this site's authenticator")).toBeNull();
    await user.type(input, "Q7K2XM{Enter}");
    expect(client.resolveTicket).toHaveBeenCalledWith("k1", "Q7K2XM");
  });

  it("does not take focus from a field the person is typing in, unless forced", () => {
    const composer = document.createElement("textarea");
    document.body.append(composer);
    composer.focus();
    mount(otpTicket("sms"));
    expect(composer).toHaveFocus();
    document.body.innerHTML = "";
    document.body.append(composer);
    composer.focus();
    mount(otpTicket("sms"), { focus: "force" });
    expect(screen.getByLabelText("Code")).toHaveFocus();
  });

  it("shows a refused code and lets the person try again", async () => {
    const user = userEvent.setup();
    const { client, onSubmitted } = mount(otpTicket("sms"), { fail: true });
    const input = screen.getByLabelText("Code");
    await user.type(input, "111111{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent("The code could not be entered.");
    expect(input).toBeEnabled();
    expect(input).toHaveFocus();
    expect(onSubmitted).not.toHaveBeenCalled();
    expect(client.resolveTicket).toHaveBeenCalledOnce();
  });

  it("saves the authenticator as {totp} from a masked field and keeps nothing on the page", async () => {
    const user = userEvent.setup();
    const KEY = "otpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PXP";
    const { client } = mount(otpTicket("authenticator", true));
    await user.click(screen.getByText("Save this site's authenticator"));
    const key = screen.getByLabelText("Setup key or otpauth:// link");
    expect(key).toHaveAttribute("type", "password");
    expect(screen.getByLabelText("Don't ask again for this site")).toBeChecked();
    await user.type(key, `${KEY}{Enter}`);
    expect(client.resolveTicket).toHaveBeenCalledExactlyOnceWith("k1", { totp: KEY });
    expect(key).toHaveValue("");
    expect(document.body.innerHTML).not.toContain("JBSWY3DPEHPK3PXP");
  });

  it("without 'Don't ask again' fills one code made here and sends no key", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(59_000);
    const user = userEvent.setup();
    const { client } = mount(otpTicket("authenticator", true));
    await user.click(screen.getByText("Save this site's authenticator"));
    await user.click(screen.getByLabelText("Don't ask again for this site"));
    await user.type(screen.getByLabelText("Setup key or otpauth:// link"), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
    await user.click(screen.getByRole("button", { name: "Fill this code" }));
    // RFC 6238 SHA-1 vector at T=59 s, last six digits.
    await vi.waitFor(() => expect(client.resolveTicket).toHaveBeenCalledExactlyOnceWith("k1", "287082"));
  });

  it("hides saving when this device cannot approve", () => {
    const host = document.createElement("div");
    document.body.append(host);
    const client = { resolveTicket: vi.fn(), rejectTicket: vi.fn() };
    mountOtpForm(host, { ticket: otpTicket("authenticator", true), client, canSave: false, buttonClass: () => "" });
    expect(screen.queryByText("Save this site's authenticator")).toBeNull();
    expect(screen.getByLabelText("Code")).toBeInTheDocument();
  });

  it("when saving is refused for missing approval, keeps the form and fills one code from the same key", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(59_000);
    const user = userEvent.setup();
    const host = document.createElement("div");
    document.body.append(host);
    const client = {
      resolveTicket: vi
        .fn()
        .mockRejectedValueOnce(new ApiError(403, "forbidden", "Forbidden"))
        .mockResolvedValue({ ...otpTicket("authenticator", true), status: "resolved" }),
      rejectTicket: vi.fn(),
    };
    const onSubmitted = vi.fn();
    mountOtpForm(host, { ticket: otpTicket("authenticator", true), client, buttonClass: () => "", onSubmitted });
    await user.click(screen.getByText("Save this site's authenticator"));
    await user.type(screen.getByLabelText("Setup key or otpauth:// link"), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ{Enter}");
    expect(client.resolveTicket).toHaveBeenNthCalledWith(1, "k1", { totp: "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ" });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Saving an authenticator needs approval rights on this device. You can still fill this one code.",
    );
    expect(screen.getByLabelText("Don't ask again for this site")).not.toBeVisible();
    expect(screen.getByLabelText("Setup key or otpauth:// link")).toHaveValue("");
    expect(document.body.innerHTML).not.toContain("GEZDGNBVGY3TQOJQ");
    expect(onSubmitted).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Fill this code" }));
    await vi.waitFor(() => expect(client.resolveTicket).toHaveBeenNthCalledWith(2, "k1", "287082"));
    await vi.waitFor(() => expect(onSubmitted).toHaveBeenCalledOnce());
  });

  it("removes the form on cleanup", () => {
    const { cleanup } = mount(otpTicket("sms"));
    cleanup();
    expect(screen.queryByLabelText("Code")).toBeNull();
  });
});

describe("authenticator registration ticket", () => {
  it("names the site and account and never needs the seed", () => {
    const ticket = { ...otpTicket("unknown"), kind: "confirm", otp: null, otp_register: { origin: "https://example.com", issuer: "Example", account: "alice" } } as Ticket;
    const info = otpRegistrationOf(ticket)!;
    expect(registrationQuestion(info)).toBe("Save the authenticator for example.com (Example · alice) to your vault?");
    expect(otpRegistrationOf({ ...ticket, kind: "info" })).toBeNull();
  });
});
