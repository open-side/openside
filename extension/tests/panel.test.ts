import { readFileSync } from "node:fs";
import { beforeEach, afterEach, describe, it, expect, vi } from "vitest";
import type { Event as DaemonEvent } from "../../web/src/api/types";
const state = vi.hoisted(() => ({
  onEvent: undefined as ((event: DaemonEvent) => void) | undefined,
  onStatus: undefined as ((status: string) => void) | undefined,
  activeTab: 42,
  storageListeners: [] as ((changes: Record<string, unknown>, area: string) => void)[],
  tabListeners: [] as (() => void)[],
  close: vi.fn(),
  api: {
    listSessions: vi.fn(async () => ({ sessions: [{ id: "session-1", title: "Current tab question" }, { id: "other", title: "Other tab" }] })),
    listTickets: vi.fn(async () => ({ tickets: [
      { id: "approval", session_id: "session-1", kind: "confirm", summary: "Allow this operation?", instructions: "Only this action", options: [], target: { origin: "https://site.example.test" } },
      { id: "question", session_id: "session-1", kind: "info", summary: "Which format?", instructions: null, options: [{ value: "brief", label: "Brief" }] },
      { id: "unrelated", session_id: "other", kind: "confirm", summary: "Other session approval", instructions: null, options: [] },
    ] })),
    resolveTicket: vi.fn(async () => undefined), rejectTicket: vi.fn(async () => undefined), ticketDone: vi.fn(async () => undefined), submitMessage: vi.fn(async () => undefined),
  },
}));
vi.mock("../src/api", () => ({ client: () => state.api, openSessionStream: (options: { onEvent: (event: DaemonEvent) => void; onStatus: (status: string) => void }) => { state.onEvent = options.onEvent; state.onStatus = options.onStatus; return { close: state.close }; } }));
beforeEach(async () => {
  vi.resetModules(); vi.clearAllMocks(); state.storageListeners.length = 0; state.tabListeners.length = 0; state.activeTab = 42; state.onEvent = undefined; state.onStatus = undefined;
  document.documentElement.innerHTML = new DOMParser().parseFromString(readFileSync(`${process.cwd()}/static/panel.html`, "utf8"), "text/html").documentElement.innerHTML;
  vi.stubGlobal("chrome", {
    tabs: { query: vi.fn(async () => [{ id: state.activeTab, url: "https://site.example.test", title: "Page" }]), get: vi.fn(async () => ({ url: "https://site.example.test", title: "Page" })), onActivated: { addListener: (listener: () => void) => state.tabListeners.push(listener) } },
    storage: { session: { get: vi.fn(async (key: string) => ({ [key]: key === "pairing" ? { url: "http://localhost:4321", token: "private" } : key === "sessions:42" ? ["session-1"] : [] })) }, onChanged: { addListener: (listener: (changes: Record<string, unknown>, area: string) => void) => state.storageListeners.push(listener) } },
    scripting: { executeScript: vi.fn(async () => [{ result: "Selected words" }]) }, runtime: { openOptionsPage: vi.fn(), sendMessage: vi.fn() },
  });
  // This entrypoint binds browser globals at module load; each case needs a fresh DOM/Chrome boundary.
  await import("../src/panel");
  await vi.waitFor(() => expect(state.onEvent).toBeTypeOf("function"));
});
afterEach(() => { window.dispatchEvent(new Event("pagehide")); vi.unstubAllGlobals(); });
describe("Ask visible behavior", () => {
  it("lists only this tab's sessions and renders streamed text safely", () => {
    expect(document.querySelector("#sessions")?.textContent).toContain("Current tab question");
    expect(document.querySelector("#sessions")?.textContent).not.toContain("Other tab");
    state.onEvent!({ kind: "assistant.delta", text: "<script>not executable</script>", seq: 1 } as DaemonEvent);
    state.onEvent!({ kind: "assistant.delta", text: " streamed", seq: 2 } as DaemonEvent);
    expect(document.querySelector("#transcript")?.textContent).toBe("<script>not executable</script> streamed");
    expect(document.querySelector("#transcript script")).toBeNull();
    state.onEvent!({ kind: "assistant.final", text: "Final reply", seq: 3 } as DaemonEvent);
    expect(document.querySelector("#transcript")?.textContent).toBe("Final reply");
  });
  it("shows the current session's approval and info choices and sends exact answers", async () => {
    await vi.waitFor(() => expect(document.querySelector("#tickets")?.textContent).toContain("Allow this operation?"));
    expect(document.querySelector("#tickets")?.textContent).not.toContain("Other session approval");
    const buttons = [...document.querySelectorAll<HTMLButtonElement>("#tickets button")];
    buttons.find(button => button.textContent === "Approve")!.click();
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenCalledWith("approval", true));
    buttons.find(button => button.textContent === "Brief")!.click();
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenCalledWith("question", "brief"));
  });
  it("@ attaches the current title URL and selection to the sent message", async () => {
    document.querySelector<HTMLButtonElement>("#attach")!.click();
    await vi.waitFor(() => expect(document.querySelector("#context")?.textContent).toContain("Attached: Page"));
    document.querySelector<HTMLTextAreaElement>("#message")!.value = "Summarize @";
    document.querySelector<HTMLFormElement>("#compose")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(state.api.submitMessage).toHaveBeenCalled());
    const [id, message] = state.api.submitMessage.mock.calls[0] as unknown as [string, { text: string }];
    expect(id).toBe("session-1"); expect(message.text).toContain("Selected words"); expect(message.text).toContain("https://site.example.test"); expect(message.text).toContain("Page");
  });
  it("submitting a standalone @ waits for context even before input capture finishes", async () => {
    document.querySelector<HTMLTextAreaElement>("#message")!.value = "Summarize @";
    document.querySelector<HTMLFormElement>("#compose")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(state.api.submitMessage).toHaveBeenCalled());
    const [, message] = state.api.submitMessage.mock.calls[0] as unknown as [string, { text: string }];
    expect(message.text).toContain("Selected words");
    expect(message.text).toContain("https://site.example.test");
  });
  it("closes the previous stream and clears context when the current tab changes", async () => {
    state.activeTab = 99;
    for (const listener of state.tabListeners) listener();
    await vi.waitFor(() => expect(state.close).toHaveBeenCalled());
    expect(document.querySelector<HTMLSelectElement>("#sessions")!.value).toBe(""); expect(document.querySelector("#tickets")?.textContent).toBe("");
  });
  it("shows the empty state until the transcript or a ticket has content", async () => {
    document.querySelector<HTMLSelectElement>("#sessions")!.value = "";
    document.querySelector("#sessions")!.dispatchEvent(new Event("change"));
    await vi.waitFor(() => expect(document.querySelector("#tickets")?.textContent).toBe(""));
    expect(document.querySelector("#transcript")?.children.length).toBe(0);
    expect(document.querySelector(".empty")?.textContent).toContain("Ask about this tab");
  });
  it("marks user messages and replies differently and mounts outline icons", () => {
    state.onEvent!({ kind: "message.queued", text: "Question", seq: 1 } as DaemonEvent);
    state.onEvent!({ kind: "assistant.final", text: "Answer", seq: 2 } as DaemonEvent);
    expect(document.querySelector("#transcript .bubble.user")?.textContent).toBe("Question");
    expect(document.querySelector("#transcript .bubble.assistant")?.textContent).toBe("Answer");
    expect(document.querySelectorAll("svg[stroke-width='1.8']").length).toBeGreaterThan(0);
    expect(document.querySelector("[data-icon] svg")).not.toBeNull();
  });
  it("renders replies as markdown like the web app, keeps user text literal and shows a writing indicator until the turn ends", () => {
    state.onEvent!({ kind: "message.queued", text: "1. **not** markdown", seq: 1 } as DaemonEvent);
    expect(document.querySelector("#transcript .bubble.user")?.textContent).toBe("1. **not** markdown");
    state.onEvent!({ kind: "turn.started", seq: 2 } as DaemonEvent);
    const reply = document.querySelector<HTMLElement>("#transcript .bubble.assistant")!;
    expect(reply.classList.contains("streaming")).toBe(true);
    state.onEvent!({ kind: "assistant.delta", text: "Steps:\n\n1. **Open** the [page](https://site.example.test)\n2. Click [here](javascript:alert(1))\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n```ts\nconst x = 1;\n```\n\n![chart](https://img.example.test/c.png)", seq: 3 } as DaemonEvent);
    expect([...reply.querySelectorAll("ol > li")].map(item => item.textContent)).toEqual(["Open the page", "Click here"]);
    expect(reply.querySelector("ol strong")?.textContent).toBe("Open");
    const links = [...reply.querySelectorAll<HTMLAnchorElement>("ol a")];
    expect(links[0].getAttribute("href")).toBe("https://site.example.test");
    expect(links[0].rel).toBe("noopener noreferrer");
    expect(links[1].getAttribute("href")).toBe("");
    expect(reply.querySelector(".table-scroll > table td")?.textContent).toBe("1");
    expect(reply.querySelector(".code-block-head span")?.textContent).toBe("ts");
    expect(reply.querySelector(".code-block pre code")?.textContent).toBe("const x = 1;\n");
    expect(reply.querySelector("img")).toBeNull();
    expect(reply.querySelector("a.md-image")?.textContent).toBe("chart");
    expect(reply.classList.contains("streaming")).toBe(true);
    state.onEvent!({ kind: "turn.ended", seq: 4 } as DaemonEvent);
    expect(reply.classList.contains("streaming")).toBe(false);
  });
  it("shows how to pair instead of an error when the pairing is gone", async () => {
    vi.mocked(chrome.storage.session.get).mockImplementation((async () => ({})) as never);
    for (const listener of state.storageListeners) listener({ pairing: { oldValue: {} } }, "session");
    await vi.waitFor(() => expect(document.body.dataset.paired).toBe("false"));
    expect(state.close).toHaveBeenCalled();
    expect(document.querySelector("#transcript")?.children.length).toBe(0);
    expect(document.querySelector("#status")?.textContent).toBe("");
    expect(document.querySelector(".empty-unpaired")?.textContent).toContain("Pair openside first");
    document.querySelector<HTMLButtonElement>("#pair-start")!.click();
    expect(chrome.runtime.openOptionsPage).toHaveBeenCalled();
  });
  it("reports connection progress in the bar and daemon failures in the status toast", async () => {
    const status = document.querySelector<HTMLElement>("#status")!;
    const connection = document.querySelector<HTMLElement>("#connection")!;
    state.onStatus!("reconnecting");
    expect(connection.textContent).toBe("Reconnecting…");
    state.onStatus!("open");
    expect(connection.textContent).toBe("");
    expect(status.textContent).toBe("");
    state.api.resolveTicket.mockRejectedValueOnce(new Error("offline"));
    await vi.waitFor(() => expect(document.querySelector("#tickets")?.textContent).toContain("Allow this operation?"));
    [...document.querySelectorAll<HTMLButtonElement>("#tickets button")].find(button => button.textContent === "Approve")!.click();
    await vi.waitFor(() => expect(status.textContent).toBe("Cannot reach openside. Check pairing and token scopes."));
    expect(status.closest<HTMLElement>(".toast")!.dataset.tone).toBe("error");
    state.onStatus!("closed");
    expect(status.textContent).toBe("Cannot reach openside. Check pairing and token scopes.");
    status.closest(".toast")!.querySelector<HTMLButtonElement>(".toast-close")!.click();
    expect(status.textContent).toBe("");
  });
  it("shows a code request from any conversation as the shared code form, keeps a half-typed code across refreshes and clears it on submit", async () => {
    const otp = { id: "otp", session_id: "other", kind: "secret_input", summary: "Sign-in code", instructions: null, options: [], origin: {}, target: { origin: "https://www.shop.example", tab_id: "t", field_ref: "e1" }, otp: { channel: "sms", can_register: false }, status: "pending" };
    const original = state.api.listTickets.getMockImplementation()!;
    state.api.listTickets.mockResolvedValue({ tickets: [otp] } as never);
    state.onEvent!({ kind: "ticket.opened", seq: 1 } as DaemonEvent);
    const input = await vi.waitFor(() => { const found = document.querySelector<HTMLInputElement>("#tickets input[autocomplete='one-time-code']"); expect(found).not.toBeNull(); return found!; });
    expect(input.getAttribute("inputmode")).toBe("numeric");
    expect(document.querySelector("#tickets")?.textContent).toContain("Code from a text message for shop.example");
    expect(document.querySelector("#tickets .ticket-auth > .action-chip")?.textContent).toBe("Action needed");
    input.value = "48";
    const before = state.api.listTickets.mock.calls.length;
    state.onEvent!({ kind: "ticket.opened", seq: 2 } as DaemonEvent);
    await vi.waitFor(() => expect(state.api.listTickets.mock.calls.length).toBeGreaterThan(before));
    await state.api.listTickets.mock.results.at(-1)!.value;
    await Promise.resolve();
    expect(input.value).toBe("48");
    expect(document.querySelector("#tickets input[autocomplete='one-time-code']")).toBe(input);
    input.value = "482913";
    input.form!.requestSubmit();
    expect(input.value).toBe("");
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenCalledWith("otp", "482913"));
    state.api.listTickets.mockImplementation(original);
  });
  it("asks before saving an authenticator found during setup, with Approve and Decline", async () => {
    const register = { id: "reg", session_id: "other", kind: "confirm", summary: "Save?", instructions: null, options: [], origin: {}, target: { origin: "https://shop.example", tab_id: "t", field_ref: null }, otp: null, otp_register: { origin: "https://shop.example", issuer: "Shop", account: "me" }, status: "pending" };
    const original = state.api.listTickets.getMockImplementation()!;
    state.api.listTickets.mockResolvedValue({ tickets: [register] } as never);
    state.onEvent!({ kind: "ticket.opened", seq: 1 } as DaemonEvent);
    await vi.waitFor(() => expect(document.querySelector("#tickets")?.textContent).toContain("Save the authenticator for shop.example (Shop · me) to your vault?"));
    const buttons = [...document.querySelectorAll<HTMLButtonElement>("#tickets button")];
    buttons.find(button => button.textContent === "Save to vault")!.click();
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenCalledWith("reg", true));
    buttons.find(button => button.textContent === "Not now")!.click();
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenCalledWith("reg", false));
    state.api.listTickets.mockImplementation(original);
  });
  it("keeps the code form when saving an authenticator is refused and fills one code from the same key", async () => {
    // Dynamic: modules are reset per case, and ApiError must be the panel form's own class.
    const { ApiError } = await import("../../web/src/api/client");
    const otp = { id: "otp", session_id: "other", kind: "secret_input", summary: "Sign-in code", instructions: null, options: [], origin: {}, target: { origin: "https://shop.example", tab_id: "t", field_ref: "e1" }, otp: { channel: "authenticator", can_register: true }, status: "pending" };
    const original = state.api.listTickets.getMockImplementation()!;
    state.api.listTickets.mockResolvedValue({ tickets: [otp] } as never);
    state.api.resolveTicket.mockRejectedValueOnce(new ApiError(403, "forbidden", "Forbidden"));
    state.onEvent!({ kind: "ticket.opened", seq: 1 } as DaemonEvent);
    const key = await vi.waitFor(() => { const found = document.querySelector<HTMLInputElement>("#tickets input.otp-key"); expect(found).not.toBeNull(); return found!; });
    key.value = "JBSWY3DPEHPK3PXP";
    key.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(state.api.resolveTicket).toHaveBeenCalledWith("otp", { totp: "JBSWY3DPEHPK3PXP" });
    await vi.waitFor(() => expect(document.querySelector("#tickets .otp-error")?.textContent).toContain("needs approval rights"));
    expect(document.querySelector("#tickets input[autocomplete='one-time-code']")).not.toBeNull();
    expect(key.value).toBe("");
    const fill = [...document.querySelectorAll<HTMLButtonElement>("#tickets button")].find(button => button.textContent === "Fill this code")!;
    fill.click();
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenLastCalledWith("otp", expect.stringMatching(/^\d{6}$/)));
    state.api.listTickets.mockImplementation(original);
  });
  it("shows another conversation's login as the shared login card and sends the values once, cleared first", async () => {
    const login = {
      id: "login", session_id: "other", kind: "login", summary: "Log in", instructions: null, options: [], origin: {}, status: "pending",
      target: { origin: "http://127.0.0.1:8123", tab_id: "t", field_ref: null }, otp: null,
      login: {
        origin: "http://127.0.0.1:8123", site_name: "Naver",
        fields: [
          { id: "f0", kind: "identifier", label: "ID or phone number", masked: false, required: true },
          { id: "f1", kind: "password", label: "Password", masked: true, required: true },
        ],
        alternatives: [{ id: "a0", label: "QR code login", kind: "qr", provider: null }],
        submit_label: "Log in", can_save: true,
      },
    };
    const original = state.api.listTickets.getMockImplementation()!;
    state.api.listTickets.mockResolvedValue({ tickets: [login] } as never);
    state.onEvent!({ kind: "ticket.opened", seq: 1 } as DaemonEvent);
    const id = await vi.waitFor(() => { const found = document.querySelector<HTMLInputElement>("#tickets input[autocomplete='username']"); expect(found).not.toBeNull(); return found!; });
    const password = document.querySelector<HTMLInputElement>("#tickets input[autocomplete='current-password']")!;
    // The page named itself "Naver"; the daemon-verified origin stays on screen and the name reads as the page's claim.
    expect(document.querySelector("#tickets .auth-origin")?.textContent).toBe("http://127.0.0.1:8123");
    expect(document.querySelector("#tickets .auth-page-name")?.textContent).toBe("Page calls itself “Naver”");
    expect(document.querySelector("#tickets .auth-title")?.textContent).toBe("Log in");
    expect(document.querySelector("#tickets")?.textContent).toContain("Handled by");
    id.value = "jane";
    password.value = "pw-secret";
    id.form!.requestSubmit();
    expect(id.value).toBe("");
    expect(password.value).toBe("");
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenCalledWith("login", { values: { f0: "jane", f1: "pw-secret" }, save: true }));
    state.api.listTickets.mockImplementation(original);
  });
  it("shows a login-step choice and sends the picked option", async () => {
    const choice = {
      id: "choice", session_id: "other", kind: "choice", summary: "Verify", instructions: null, options: [], origin: {}, status: "pending",
      target: { origin: "https://nid.naver.com", tab_id: "t", field_ref: null }, otp: null,
      choice: { origin: "https://nid.naver.com", site_name: "Naver", question: "How would you like to verify?", options: [{ id: "o0", label: "Text me a code" }, { id: "o1", label: "Email me a code" }], multiple: false },
    };
    const original = state.api.listTickets.getMockImplementation()!;
    state.api.listTickets.mockResolvedValue({ tickets: [choice] } as never);
    state.onEvent!({ kind: "ticket.opened", seq: 1 } as DaemonEvent);
    const radios = await vi.waitFor(() => { const found = [...document.querySelectorAll<HTMLInputElement>("#tickets input[type='radio']")]; expect(found).toHaveLength(2); return found; });
    radios[1]!.click();
    radios[1]!.form!.requestSubmit();
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenCalledWith("choice", { option: "o1" }));
    state.api.listTickets.mockImplementation(original);
  });
});
