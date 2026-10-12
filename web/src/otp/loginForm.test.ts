import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api/client";
import type { Ticket } from "../api/types";
import {
  authOutcome,
  mountApprovalCard,
  mountChoiceForm,
  mountLoginForm,
  type ApprovalTicket,
  type ChoiceTicket,
  type LoginTicket,
} from "./loginForm";

const PASSWORD = "hunter2-correct-horse";
const ICON = "data:image/png;base64,iVBORw0KGgo=";

function base(kind: "login" | "choice" | "device_approval"): Ticket {
  return {
    id: "k1",
    session_id: "s1",
    tool_call_id: kind,
    deferral: "external",
    kind,
    summary: "Sign in",
    instructions: null,
    options: [],
    origin: { subagent_id: null },
    tool: null,
    target: { origin: "https://nid.naver.com", tab_id: "t", field_ref: null },
    resume_when: null,
    created_at: "2026-01-01T00:00:00Z",
    expires_at: "2026-01-02T00:00:00Z",
    status: "pending",
    resolution: null,
  } as Ticket;
}

function loginTicket(overrides: Partial<LoginTicket["login"]> = {}): LoginTicket {
  return {
    ...base("login"),
    login: {
      origin: "https://nid.naver.com",
      site_name: "Naver",
      icon: ICON,
      fields: [
        { id: "id", kind: "identifier", label: "ID or phone number", masked: false, required: true },
        { id: "pw", kind: "password", label: "Password", masked: true, required: true },
        { id: "keep", kind: "checkbox", label: "Stay signed in", masked: false, required: false },
      ],
      alternatives: [
        { id: "a1", label: "Sign in with Google", kind: "provider", provider: "google" },
        { id: "a2", label: "Passkey", kind: "passkey", provider: null },
        { id: "a3", label: "One-time number", kind: "other", provider: null },
      ],
      submit_label: "Log in",
      can_save: true,
      ...overrides,
    },
  } as LoginTicket;
}

function choiceTicket(): ChoiceTicket {
  return {
    ...base("choice"),
    choice: {
      origin: "https://nid.naver.com",
      site_name: "Naver",
      icon: null,
      question: "How would you like to verify?",
      options: [
        { id: "o1", label: "Text me a code (Mobile Phone)" },
        { id: "o2", label: "Email me a code (Email Address)", description: "j***@example.com" },
      ],
      multiple: false,
    },
  } as ChoiceTicket;
}

function approvalTicket(number: string | null = "42"): ApprovalTicket {
  return {
    ...base("device_approval"),
    created_at: new Date(Date.now() - 65_000).toISOString(),
    approval: {
      origin: "https://bank.test",
      site_name: "Northwind",
      icon: null,
      message: "We sent a sign-in request to the Northwind app on your phone.",
      number,
    },
  } as ApprovalTicket;
}

function client(resolve: (body: unknown) => unknown = () => ({})) {
  return {
    resolveTicket: vi.fn(async (_id: string, body: unknown) => resolve(body) as Ticket),
    rejectTicket: vi.fn(async () => ({}) as Ticket),
  };
}

function host(): HTMLElement {
  const element = document.createElement("div");
  document.body.append(element);
  return element;
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("mountLoginForm", () => {
  it("builds the fields from the page descriptor with password-manager autocomplete", () => {
    const api = client();
    mountLoginForm(host(), { ticket: loginTicket(), client: api, buttonClass: () => "" });
    const form = screen.getByRole("form", { name: "Log in https://nid.naver.com" });
    expect(form.querySelector(".auth-icon img")).toHaveAttribute("src", ICON);
    const id = screen.getByLabelText("ID or phone number");
    expect(id).toHaveAttribute("autocomplete", "username");
    expect(id).toHaveAttribute("type", "text");
    expect(id).toHaveFocus();
    const password = screen.getByLabelText("Password");
    expect(password).toHaveAttribute("autocomplete", "current-password");
    expect(password).toHaveAttribute("type", "password");
    expect(screen.getByLabelText("Stay signed in")).toHaveAttribute("type", "checkbox");
    expect(screen.getByLabelText("Save to vault")).toBeChecked();
    const alternatives = within(screen.getByRole("group", { name: "Other ways to sign in" })).getAllByRole("button");
    expect(alternatives.map((button) => button.getAttribute("aria-label"))).toEqual(["Sign in with Google", "Passkey", "One-time number"]);
    expect(alternatives[2]).toHaveAttribute("title", "One-time number");
    expect(form).toHaveTextContent("Handled by");
    expect(form.querySelector("img:not(.auth-icon img), [src^='http']")).toBeNull();
  });

  it("a retried card shows the page's own error above the fields", () => {
    mountLoginForm(host(), { ticket: loginTicket({ error_text: "Incorrect email or password." }), client: client(), buttonClass: () => "" });
    expect(document.querySelector(".auth-page-error")).toHaveTextContent("nid.naver.com says: Incorrect email or password.");
    document.body.innerHTML = "";
    mountLoginForm(host(), { ticket: loginTicket(), client: client(), buttonClass: () => "" });
    expect(document.querySelector(".auth-page-error")).toBeNull();
  });

  it("an icon that is not a data image is never loaded; the provider mark or the initial stands in", () => {
    mountLoginForm(host(), { ticket: loginTicket({ icon: "https://tracker.example/x.png" }), client: client(), buttonClass: () => "" });
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector(".auth-icon--glyph svg")).not.toBeNull();
    document.body.innerHTML = "";
    mountLoginForm(host(), { ticket: loginTicket({ icon: null, origin: "https://bank.test", site_name: "Example Bank" }), client: client(), buttonClass: () => "" });
    expect(document.querySelector(".auth-icon--letter")).toHaveTextContent("B");
  });

  it("the eye toggles the password between hidden and shown", async () => {
    const user = userEvent.setup();
    mountLoginForm(host(), { ticket: loginTicket(), client: client(), buttonClass: () => "" });
    const password = screen.getByLabelText("Password");
    await user.click(screen.getByRole("button", { name: "Show Password" }));
    expect(password).toHaveAttribute("type", "text");
    expect(screen.getByRole("button", { name: "Hide Password" })).toHaveAttribute("aria-pressed", "true");
    await user.click(screen.getByRole("button", { name: "Hide Password" }));
    expect(password).toHaveAttribute("type", "password");
  });

  it("sends the values once, with every field emptied before the request and nothing left in the page", async () => {
    const user = userEvent.setup();
    let seen: { id: string; pw: string } | null = null;
    const api = client(() => {
      seen = { id: screen.getByLabelText<HTMLInputElement>("ID or phone number").value, pw: screen.getByLabelText<HTMLInputElement>("Password").value };
      return {};
    });
    const onSubmitted = vi.fn();
    mountLoginForm(host(), { ticket: loginTicket(), client: api, buttonClass: () => "", onSubmitted });
    await user.type(screen.getByLabelText("ID or phone number"), "jane");
    await user.click(screen.getByRole("button", { name: "Show Password" }));
    await user.type(screen.getByLabelText("Password"), PASSWORD);
    await user.click(screen.getByLabelText("Stay signed in"));
    await user.click(screen.getByRole("button", { name: "Log in" }));
    expect(api.resolveTicket).toHaveBeenCalledExactlyOnceWith("k1", { values: { id: "jane", pw: PASSWORD, keep: true }, save: true });
    expect(seen).toEqual({ id: "", pw: "" });
    expect(screen.getByLabelText("Password")).toHaveAttribute("type", "password");
    expect(document.body.innerHTML).not.toContain(PASSWORD);
    await vi.waitFor(() => expect(onSubmitted).toHaveBeenCalledOnce());
  });

  it("a missing required field is named and nothing is sent", async () => {
    const user = userEvent.setup();
    const api = client();
    mountLoginForm(host(), { ticket: loginTicket(), client: api, buttonClass: () => "" });
    await user.type(screen.getByLabelText("ID or phone number"), "jane{Enter}");
    expect(api.resolveTicket).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("Enter your password.");
    expect(screen.getByLabelText("Password")).toHaveFocus();
    expect(screen.getByLabelText("ID or phone number")).toHaveValue("jane");
  });

  it("save is hidden when the ticket cannot save or the device lacks approve", () => {
    mountLoginForm(host(), { ticket: loginTicket({ can_save: false }), client: client(), buttonClass: () => "" });
    expect(screen.queryByLabelText("Save to vault")).toBeNull();
    document.body.innerHTML = "";
    mountLoginForm(host(), { ticket: loginTicket(), client: client(), canSave: false, buttonClass: () => "" });
    expect(screen.queryByLabelText("Save to vault")).toBeNull();
    expect(screen.getByRole("form")).not.toHaveTextContent("vault");
  });

  it("a 403 on save puts the details back, hides save and logs in without saving on the next press", async () => {
    const user = userEvent.setup();
    const api = client((body) => {
      if (body && typeof body === "object" && "save" in body && body.save) throw new ApiError(403, "forbidden", "Forbidden");
      return {};
    });
    const onSubmitted = vi.fn();
    mountLoginForm(host(), { ticket: loginTicket(), client: api, buttonClass: () => "", onSubmitted });
    await user.type(screen.getByLabelText("ID or phone number"), "jane");
    await user.type(screen.getByLabelText("Password"), `${PASSWORD}{Enter}`);
    expect(await screen.findByRole("alert")).toHaveTextContent("needs approval rights");
    expect(screen.getByLabelText("Save to vault")).not.toBeVisible();
    expect(onSubmitted).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Log in" }));
    expect(api.resolveTicket).toHaveBeenNthCalledWith(2, "k1", { values: { id: "jane", pw: PASSWORD, keep: false }, save: false });
    await vi.waitFor(() => expect(onSubmitted).toHaveBeenCalledOnce());
  });

  it("a changed page is explained and the values are not put back", async () => {
    const user = userEvent.setup();
    const api = client(() => {
      throw new ApiError(409, "target_unavailable", "gone");
    });
    mountLoginForm(host(), { ticket: loginTicket(), client: api, buttonClass: () => "" });
    await user.type(screen.getByLabelText("ID or phone number"), "jane");
    await user.type(screen.getByLabelText("Password"), `${PASSWORD}{Enter}`);
    expect(await screen.findByRole("alert")).toHaveTextContent("The sign-in page changed");
    expect(screen.getByLabelText("Password")).toHaveValue("");
  });

  it("an alternative, a saved account and close send their own bodies", async () => {
    const user = userEvent.setup();
    const accounts = [{ item_id: "v1", username_hint: "j***@naver.com" }, { item_id: "v2", username_hint: "k***@naver.com" }];
    const mountWith = () => {
      document.body.innerHTML = "";
      const api = client();
      mountLoginForm(host(), { ticket: loginTicket({ accounts }), client: api, buttonClass: () => "" });
      return api;
    };
    let api = mountWith();
    const rows = within(screen.getByRole("group", { name: "Saved accounts" })).getAllByRole("button");
    expect(rows[0]).toHaveFocus();
    await user.click(rows[1]!);
    expect(api.resolveTicket).toHaveBeenCalledExactlyOnceWith("k1", { account: "v2" });
    // In flight, the card takes no second answer.
    expect(screen.getByRole("button", { name: "Log in" })).toBeDisabled();

    api = mountWith();
    await user.click(screen.getByRole("button", { name: "Passkey" }));
    expect(api.resolveTicket).toHaveBeenCalledExactlyOnceWith("k1", { alternative: "a2" });

    api = mountWith();
    await user.click(screen.getByRole("button", { name: "Cancel login" }));
    expect(api.rejectTicket).toHaveBeenCalledWith("k1");
    expect(api.resolveTicket).not.toHaveBeenCalled();
  });
});

describe("mountChoiceForm", () => {
  it("picks one option and sends its id; Continue waits for a pick", async () => {
    const user = userEvent.setup();
    const api = client();
    const onSubmitted = vi.fn();
    mountChoiceForm(host(), { ticket: choiceTicket(), client: api, buttonClass: () => "", onSubmitted });
    const group = screen.getByRole("radiogroup", { name: "How would you like to verify?" });
    expect(within(group).getAllByRole("radio")).toHaveLength(2);
    expect(group).toHaveTextContent("j***@example.com");
    const next = screen.getByRole("button", { name: "Continue" });
    expect(next).toBeDisabled();
    await user.click(screen.getByLabelText(/Email me a code/));
    await user.click(next);
    expect(api.resolveTicket).toHaveBeenCalledExactlyOnceWith("k1", { option: "o2" });
    await vi.waitFor(() => expect(onSubmitted).toHaveBeenCalledOnce());
  });

  it("dismiss rejects the ticket", async () => {
    const user = userEvent.setup();
    const api = client();
    mountChoiceForm(host(), { ticket: choiceTicket(), client: api, buttonClass: () => "" });
    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(api.rejectTicket).toHaveBeenCalledWith("k1");
  });
});

describe("mountApprovalCard", () => {
  it("shows the number to tap and a running clock, and cancel rejects", async () => {
    const user = userEvent.setup();
    const api = client();
    const cleanup = mountApprovalCard(host(), { ticket: approvalTicket(), client: api, buttonClass: () => "" });
    const card = screen.getByRole("region", { name: "Approve on your phone https://bank.test" });
    expect(within(card).getByLabelText("Number 42")).toHaveTextContent("42");
    expect(within(card).getByRole("status")).toHaveTextContent(/Waiting for approval\s*1:0[5-9]/);
    expect(card).toHaveTextContent("sign-in request to the Northwind app");
    await user.click(within(card).getByRole("button", { name: "Cancel" }));
    expect(api.rejectTicket).toHaveBeenCalledExactlyOnceWith("k1");
    expect(api.resolveTicket).not.toHaveBeenCalled();
    cleanup();
    expect(screen.queryByRole("region", { name: /^Approve on your phone/ })).toBeNull();
  });

  it("leaves the number out when the page shows none", () => {
    mountApprovalCard(host(), { ticket: approvalTicket(null), client: client(), buttonClass: () => "" });
    expect(screen.queryByText("Tap this number on your phone")).toBeNull();
  });
});

describe("authOutcome", () => {
  it("names what happened without any value", () => {
    expect(authOutcome({ ...loginTicket(), status: "resolved", resolution: { status: "submitted", saved: true, next: "done" } })).toEqual({ text: "nid.naver.com login saved to the vault", failed: false });
    expect(authOutcome({ ...loginTicket(), status: "resolved", resolution: { status: "submitted", saved: false, next: "done" } })?.text).toBe("nid.naver.com login sent");
    expect(authOutcome({ ...loginTicket(), status: "resolved", resolution: { status: "filled_from_vault", next: "otp" } })?.text).toBe("Signed in to nid.naver.com with a saved account");
    expect(authOutcome({ ...loginTicket(), status: "resolved", resolution: { status: "submitted", next: "error", error_text: "Wrong password" } })).toEqual({ text: "nid.naver.com showed an error: Wrong password", failed: true });
    expect(authOutcome({ ...choiceTicket(), status: "resolved", resolution: { status: "chosen", option: "o1", next: "otp" } })?.text).toBe("Chose “Text me a code (Mobile Phone)”");
    expect(authOutcome(loginTicket())).toBeNull();
    expect(authOutcome({ ...approvalTicket(), status: "resolved", resolution: { status: "done", next: "done" } })?.text).toBe("Sign-in to bank.test approved on your phone");
  });
});

describe("verified origin on every card", () => {
  // The page chose its own name and favicon; the daemon chose the origin. The origin must stay
  // visible and the page's name must read as a claim.
  const spoofed = { origin: "http://127.0.0.1:8123", site_name: "Naver" };

  it.each([
    ["login", () => mountLoginForm(host(), { ticket: loginTicket(spoofed), client: client(), buttonClass: () => "" })],
    ["choice", () => mountChoiceForm(host(), { ticket: { ...choiceTicket(), choice: { ...choiceTicket().choice, ...spoofed } }, client: client(), buttonClass: () => "" })],
    ["approval", () => mountApprovalCard(host(), { ticket: { ...approvalTicket(), approval: { ...approvalTicket().approval, ...spoofed } }, client: client(), buttonClass: () => "" })],
  ])("%s card shows the real origin under a page-provided name", (_kind, mount) => {
    mount();
    const origin = document.querySelector(".auth-origin")!;
    expect(origin).toHaveTextContent("http://127.0.0.1:8123");
    expect(origin).toHaveAttribute("data-verified", "true");
    expect(document.querySelector(".auth-page-name")).toHaveTextContent("Page calls itself “Naver”");
    expect(document.querySelector(".auth-title")).not.toHaveTextContent("Naver");
    expect(document.querySelector(".auth-icon--glyph")).toBeNull();
  });

  it("spells an internationalised host in punycode and keeps a non-default port", () => {
    mountLoginForm(host(), { ticket: loginTicket({ origin: "https://nаver.com:8443", site_name: "Naver" }), client: client(), buttonClass: () => "" });
    expect(document.querySelector(".auth-origin")?.textContent).toMatch(/^https:\/\/xn--.+\.com:8443$/);
  });

  it("omits the claim when the page name is the verified host, and flags an unreadable origin", () => {
    mountLoginForm(host(), { ticket: loginTicket({ origin: "https://nid.naver.com", site_name: "nid.naver.com" }), client: client(), buttonClass: () => "" });
    expect(document.querySelector(".auth-page-name")).toBeNull();
    document.body.innerHTML = "";
    mountLoginForm(host(), { ticket: loginTicket({ origin: "not a url" }), client: client(), buttonClass: () => "" });
    expect(document.querySelector(".auth-origin")).toHaveAttribute("data-verified", "false");
  });
});
