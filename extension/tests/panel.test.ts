import { readFileSync } from "node:fs";
import { beforeEach, afterEach, describe, it, expect, vi } from "vitest";
import type { Event as DaemonEvent } from "../../web/src/api/types";
import type { Card } from "../../web/src/api/types";
import cardFixtures from "../../crates/openside-proto/src/cards_fixtures.json";
// The daemon's own card fixtures stand in for the cards a ticket carries.
function ticketFor(cardId: string, ticketId: string, session: string, extra: Record<string, unknown> = {}, title?: string) {
  const card = structuredClone((cardFixtures as unknown as Card[]).find(candidate => candidate.id === cardId)!);
  card.id = `ticket:${ticketId}`; card.ticket_id = ticketId; if (title) { card.title = title; card.collapsed_summary = title; }
  return { id: ticketId, session_id: session, kind: "confirm", summary: card.title, instructions: null, options: [], origin: {}, status: "pending", created_at: "2026-01-01T00:00:00Z", otp: null, card, ...extra };
}
function baseTickets() {
  return [
    ticketFor("ticket:confirm1", "approval", "session-1", { kind: "confirm" }),
    ticketFor("ticket:form1", "question", "session-1", { kind: "info" }),
    ticketFor("ticket:confirm1", "unrelated", "other", { kind: "confirm" }, "Other session approval"),
  ];
}
const state = vi.hoisted(() => ({
  onEvent: undefined as ((event: DaemonEvent) => void) | undefined,
  onStatus: undefined as ((status: string) => void) | undefined,
  activeTab: 42,
  storageListeners: [] as ((changes: Record<string, unknown>, area: string) => void)[],
  tabListeners: [] as (() => void)[],
  close: vi.fn(),
  api: {
    listSessions: vi.fn(async () => ({ sessions: [{ id: "session-1", title: "Current tab question" }, { id: "other", title: "Other tab" }] })),
    listTickets: vi.fn(async () => ({ tickets: baseTickets() })),
    faviconBlob: vi.fn(async () => { throw new Error("no icon"); }), artifactBlob: vi.fn(async () => { throw new Error("no image"); }),
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
afterEach(() => { window.dispatchEvent(new Event("pagehide")); vi.unstubAllGlobals(); state.api.listTickets.mockImplementation(async () => ({ tickets: baseTickets() })); });
// A request from another conversation, shown as the person would see it; resolves with the first element matching `selector`.
async function showOther(ticket: { card: Card }, selector: string) {
  state.api.listTickets.mockImplementation(async () => ({ tickets: [ticket] }) as never);
  state.onEvent!({ kind: "ticket.opened", seq: 1 } as DaemonEvent);
  return vi.waitFor(() => { const found = document.querySelector<HTMLInputElement>(`#tickets ${selector}`); expect(found).not.toBeNull(); return found!; });
}
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
  it("shows the current session's approval and question and sends exact answers", async () => {
    await vi.waitFor(() => expect(document.querySelector("#tickets")?.textContent).toContain("Send this message?"));
    expect(document.querySelector("#tickets")?.textContent).not.toContain("Other session approval");
    [...document.querySelectorAll<HTMLButtonElement>("#tickets button")].find(button => button.textContent === "Approve")!.click();
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenCalledWith("approval", true));
    const party = document.querySelector<HTMLInputElement>("#tickets input")!;
    party.value = "Kim";
    [...document.querySelectorAll<HTMLButtonElement>("#tickets button")].find(button => button.textContent === "Fill fields")!.click();
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenCalledWith("question", { values: { party: "Kim" } }));
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
    expect(document.querySelector<HTMLElement>("#sessions")!.dataset.value).toBe(""); expect(document.querySelector("#tickets")?.textContent).toBe("");
  });
  it("shows the empty state until the transcript or a ticket has content", async () => {
    document.querySelector<HTMLButtonElement>("#sessions .picker-button")!.click();
    [...document.querySelectorAll<HTMLElement>("#sessions [role=option]")].find(option => option.textContent === "New chat")!.click();
    await vi.waitFor(() => expect(document.querySelector("#tickets")?.textContent).toBe(""));
    expect(document.querySelector("#transcript")?.children.length).toBe(0);
    expect(document.querySelector(".empty")?.textContent).toContain("Ask about this tab");
  });
  it("marks user messages and replies differently and mounts outline icons", () => {
    state.onEvent!({ kind: "message.queued", text: "Question", seq: 1, author_kind: "person", from_agent: null } as DaemonEvent);
    state.onEvent!({ kind: "assistant.final", text: "Answer", seq: 2 } as DaemonEvent);
    expect(document.querySelector("#transcript .bubble.user")?.textContent).toBe("Question");
    expect(document.querySelector("#transcript .bubble.assistant")?.textContent).toBe("Answer");
    expect(document.querySelectorAll("svg[stroke-width='1.8']").length).toBeGreaterThan(0);
    expect(document.querySelector("[data-icon] svg")).not.toBeNull();
  });
  it("renders replies as markdown like the web app, keeps user text literal and shows the typing dots until the first words", () => {
    state.onEvent!({ kind: "message.queued", text: "1. **not** markdown", seq: 1, author_kind: "person", from_agent: null } as DaemonEvent);
    expect(document.querySelector("#transcript .bubble.user")?.textContent).toBe("1. **not** markdown");
    state.onEvent!({ kind: "turn.started", seq: 2 } as DaemonEvent);
    expect(document.querySelector("#transcript .typing")?.textContent).toBe("Agent is working");
    expect(document.querySelector("#transcript .bubble.assistant")).toBeNull();
    state.onEvent!({ kind: "assistant.delta", text: "Steps:\n\n1. **Open** the [page](https://site.example.test)\n2. Click [here](javascript:alert(1))\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n```ts\nconst x = 1;\n```\n\n![chart](https://img.example.test/c.png)", seq: 3 } as DaemonEvent);
    const reply = document.querySelector<HTMLElement>("#transcript .bubble.assistant")!;
    expect(document.querySelector("#transcript .typing")).toBeNull();
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
  });
  it("preserves daemon sender metadata and reserves person receipts for person authority", () => {
    state.onEvent!({ kind: "message.queued", message_id: "plain-internal", text: "Team delivery", seq: 1, author_kind: "internal", from_agent: "chief" } as DaemonEvent);
    expect(document.querySelector<HTMLElement>("#transcript .bubble")?.dataset).toMatchObject({ authorKind: "internal", fromAgent: "chief" });
    expect(document.querySelector("#transcript .receipt")).toBeNull();
    state.onEvent!({ kind: "message.queued", message_id: "api", text: "API request", seq: 2, author_kind: "agent_client", from_agent: null } as DaemonEvent);
    expect(document.querySelector("#transcript .receipt")).toBeNull();
    state.onEvent!({ kind: "message.queued", message_id: "task-report-person", text: "Person request", seq: 3, author_kind: "person", from_agent: null } as DaemonEvent);
    expect(document.querySelector("#transcript .receipt")?.textContent).toBe("Sent");
  });
  it("shows Sent, then Read, then drops the note once the agent answers; a tool call brings the dots back", () => {
    const note = () => document.querySelector("#transcript .receipt")?.textContent ?? null;
    state.onEvent!({ kind: "message.queued", message_id: "m1", text: "Question", seq: 1, author_kind: "person", from_agent: null } as DaemonEvent);
    expect(note()).toBe("Sent");
    expect(document.querySelector("#transcript .bubble.user + .receipt")).not.toBeNull();
    state.onEvent!({ kind: "message.injected", message_id: "m1", seq: 2 } as DaemonEvent);
    expect(note()).toBe("Read");
    state.onEvent!({ kind: "turn.started", seq: 3 } as DaemonEvent);
    state.onEvent!({ kind: "assistant.delta", text: "Looking", seq: 4 } as DaemonEvent);
    expect(note()).toBeNull();
    state.onEvent!({ kind: "tool.started", tool_call_id: "t1", name: "read_file", args: {}, seq: 5 } as unknown as DaemonEvent);
    expect(document.querySelector("#transcript .typing")).not.toBeNull();
    state.onEvent!({ kind: "assistant.delta", text: " more", seq: 6 } as DaemonEvent);
    expect(document.querySelector("#transcript .typing")).toBeNull();
    expect(document.querySelector("#transcript .bubble.assistant")?.textContent).toBe("Looking more");
  });
  it("keeps a failure note under the message and removes it when the next message arrives", () => {
    state.onEvent!({ kind: "message.queued", message_id: "m1", text: "Question", seq: 1, author_kind: "person", from_agent: null } as DaemonEvent);
    state.onEvent!({ kind: "turn.started", seq: 2 } as DaemonEvent);
    state.onEvent!({ kind: "turn.ended", seq: 3 } as DaemonEvent);
    state.onEvent!({ kind: "message.completed", message_id: "m1", outcome: "failed", reason: "interrupted", seq: 4 } as DaemonEvent);
    expect(document.querySelector("#transcript .receipt-failed")?.textContent).toBe("Interrupted");
    expect(document.querySelector("#transcript .typing")).toBeNull();
    state.onEvent!({ kind: "message.queued", message_id: "m2", text: "Again", seq: 5, author_kind: "person", from_agent: null } as DaemonEvent);
    expect(document.querySelectorAll("#transcript .receipt").length).toBe(1);
    expect(document.querySelector("#transcript .receipt")?.textContent).toBe("Sent");
  });
  it("renames the open conversation in the picker when the model proposes a title", async () => {
    state.api.listSessions.mockResolvedValueOnce({ sessions: [{ id: "session-1", title: "Pricing comparison" }] });
    state.onEvent!({ kind: "session.title_proposed", seq: 1 } as unknown as DaemonEvent);
    const picker = document.querySelector<HTMLElement>("#sessions")!;
    await vi.waitFor(() => expect(picker.querySelector(".picker-label")?.textContent).toBe("Pricing comparison"));
    expect(picker.dataset.value).toBe("session-1");
    expect(picker.title).toBe("Pricing comparison");
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
    await vi.waitFor(() => expect(document.querySelector("#tickets")?.textContent).toContain("Send this message?"));
    [...document.querySelectorAll<HTMLButtonElement>("#tickets button")].find(button => button.textContent === "Approve")!.click();
    // A refused answer is reported on the card that was answered, in its own words; the toast is for the connection.
    await vi.waitFor(() => expect(document.querySelector("#tickets .oc-error")?.textContent).toBe("offline"));
    expect(status.textContent).toBe("");
    state.api.listTickets.mockRejectedValueOnce(new Error("offline"));
    state.onEvent!({ kind: "ticket.opened", seq: 9 } as DaemonEvent);
    await vi.waitFor(() => expect(status.textContent).toBe("Cannot reach openside. Check pairing and token scopes."));
    expect(status.closest<HTMLElement>(".toast")!.dataset.tone).toBe("error");
    status.closest(".toast")!.querySelector<HTMLButtonElement>(".toast-close")!.click();
    expect(status.textContent).toBe("");
  });
  it("shows a code request from any conversation as a card, keeps a half-typed code across refreshes and clears it on submit", async () => {
    const input = await showOther(ticketFor("ticket:otp1", "otp", "other", { kind: "secret_input", otp: { channel: "sms", can_register: false } }), "input[autocomplete='one-time-code']");
    expect(document.querySelector("#tickets .oc-card")?.textContent).toContain("Verification code");
    input.value = "48";
    const before = state.api.listTickets.mock.calls.length;
    state.onEvent!({ kind: "ticket.opened", seq: 2 } as DaemonEvent);
    await vi.waitFor(() => expect(state.api.listTickets.mock.calls.length).toBeGreaterThan(before));
    await state.api.listTickets.mock.results.at(-1)!.value;
    await Promise.resolve();
    expect(input.value).toBe("48");
    expect(document.querySelector("#tickets input[autocomplete='one-time-code']")).toBe(input);
    input.value = "482 913";
    document.querySelector<HTMLButtonElement>("#tickets .oc-btn--primary")!.click();
    expect(input.value).toBe("");
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenCalledWith("otp", "482913"));
  });
  it("asks before saving an authenticator found during setup and answers Approve", async () => {
    await showOther(ticketFor("ticket:confirm1", "reg", "other", { kind: "confirm", otp_register: { origin: "https://shop.example", issuer: "Shop", account: "me" } }), ".oc-btn--primary");
    document.querySelector<HTMLButtonElement>("#tickets .oc-btn--primary")!.click();
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenCalledWith("reg", true));
  });
  it("keeps the code form when saving an authenticator is refused and fills one code from the same key", async () => {
    // Dynamic: modules are reset per case, and ApiError must be the renderer's own class.
    const { ApiError } = await import("../../web/src/api/client");
    const ticket = ticketFor("ticket:otp1", "otp", "other", { kind: "secret_input", otp: { channel: "authenticator", can_register: true } });
    ticket.card.blocks.push({ type: "input_fields", id: "auth-fields", fields: [
      { id: "key", label: "Authenticator setup key", kind: "text", secret: true, otp: false, card: null, required: true, autocomplete: null, placeholder: null, validation: "text", max_length: 512, initial: null, suggestions: [], target_origin: null },
      { id: "keep", label: "Save to vault", kind: "checkbox", secret: false, otp: false, card: null, required: false, autocomplete: null, placeholder: null, validation: "text", max_length: 8, initial: true, suggestions: [], target_origin: null },
    ] });
    ticket.card.actions.push({ id: "auth", label: "Use authenticator", style: "secondary", target: { type: "ticket_resolve", answer: { type: "otp_authenticator", field_id: "key", save_field_id: "keep" } } });
    state.api.resolveTicket.mockRejectedValueOnce(new ApiError(403, "forbidden", "Forbidden"));
    await showOther(ticket, "details");
    const key = document.querySelector<HTMLInputElement>("#tickets details input:not([type='checkbox'])")!;
    key.value = "JBSWY3DPEHPK3PXP";
    const use = [...document.querySelectorAll<HTMLButtonElement>("#tickets details button")].find(button => button.textContent === "Use authenticator")!;
    use.click();
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenCalledWith("otp", { totp: "JBSWY3DPEHPK3PXP" }));
    await vi.waitFor(() => expect(document.querySelector("#tickets .oc-error")).not.toBeNull());
    expect(document.querySelector("#tickets input[autocomplete='one-time-code']")).not.toBeNull();
    expect(key.value).toBe("");
    use.click();
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenLastCalledWith("otp", expect.stringMatching(/^\d{6}$/)));
  });
  it("shows another conversation's login as a card with the verified origin and sends the values once, cleared first", async () => {
    const id = await showOther(ticketFor("ticket:login1", "login", "other", { kind: "login" }), "input[autocomplete='username']");
    const password = document.querySelector<HTMLInputElement>("#tickets input[autocomplete='current-password']")!;
    expect(document.querySelector("#tickets .oc-origin")?.textContent).toBe("https://accounts.example");
    id.value = "jane";
    password.value = "pw-secret";
    document.querySelector<HTMLButtonElement>("#tickets .oc-btn--primary")!.click();
    expect(id.value).toBe("");
    expect(password.value).toBe("");
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenCalledWith("login", { values: expect.objectContaining({}) as never, save: false }));
    const body = JSON.stringify(state.api.resolveTicket.mock.calls.at(-1));
    expect(body).toContain("jane");
    expect(body).toContain("pw-secret");
    expect(document.querySelector("#tickets")?.innerHTML).not.toContain("pw-secret");
  });
  it("shows a login-step choice and sends the picked option", async () => {
    await showOther(ticketFor("ticket:choice1", "choice", "other", { kind: "choice" }), "input[type='radio']");
    const radios = [...document.querySelectorAll<HTMLInputElement>("#tickets input[type='radio']")];
    expect(radios).toHaveLength(2);
    radios[1]!.click();
    document.querySelector<HTMLButtonElement>("#tickets .oc-btn--primary")!.click();
    await vi.waitFor(() => expect(state.api.resolveTicket).toHaveBeenCalledWith("choice", { option: "page-account-2" }));
  });
});
