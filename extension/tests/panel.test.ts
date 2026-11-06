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
  micPermission: "granted" as PermissionState,
  storageListeners: [] as ((changes: Record<string, unknown>, area: string) => void)[],
  tabListeners: [] as (() => void)[],
  close: vi.fn(),
  openConversation: vi.fn(async () => undefined),
  // chrome.storage.session as a plain store; the panel reads pairing, this tab's chats and hand-offs from it.
  session: {} as Record<string, unknown>,
  api: {
    listSessions: vi.fn(async () => ({ sessions: [{ id: "session-1", title: "Current tab question", model: "openai:gpt-test", effort: null, confirmation: "ask_first", status: "idle", context: { used_tokens: 50000, window_tokens: 200000 } }, { id: "other", title: "Other tab", model: "openai:gpt-test", effort: null, confirmation: "ask_first", status: "idle" }] })),
    listTickets: vi.fn(async () => ({ tickets: baseTickets() })),
    faviconBlob: vi.fn(async () => { throw new Error("no icon"); }), artifactBlob: vi.fn(async () => { throw new Error("no image"); }),
    resolveTicket: vi.fn(async () => undefined), rejectTicket: vi.fn(async () => undefined), ticketDone: vi.fn(async () => undefined), submitMessage: vi.fn(async () => undefined),
    modelCatalog: vi.fn(async () => ({ default_model: "openai:gpt-test", providers: [{ provider: "openai", models: [
      { id: "openai:gpt-test", display_name: "GPT Test", label: "gpt-test", kind: "chat", available: true, default_effort: "high" },
      { id: "openai:gpt-mini", display_name: "GPT Mini", label: "gpt-mini", kind: "chat", available: true, default_effort: null },
    ] }] })),
    preferences: vi.fn(async () => ({ confirmation: "ask_first" })),
    updateSession: vi.fn(async (id: string, patch: Record<string, unknown>) => ({ id, title: "Current tab question", model: "openai:gpt-test", effort: null, confirmation: "ask_first", status: "idle", ...patch })),
    createSession: vi.fn(async () => ({ id: "created" })),
    stopSession: vi.fn(async () => undefined),
    session: vi.fn(async (id: string) => ({ id, title: "Current tab question", model: "openai:gpt-test", effort: null, confirmation: "ask_first", status: "idle", context: { used_tokens: 190000, window_tokens: 200000 } })),
    forkSession: vi.fn(async () => ({ id: "branch-1" })),
    sttStatus: vi.fn(async () => ({ available: true, reason: null, max_bytes: 10_000_000, max_seconds: 120 })),
    transcribe: vi.fn(async () => ({ text: "spoken words" })),
  },
}));
vi.mock("../src/api", () => ({ client: () => state.api, openSessionStream: (options: { onEvent: (event: DaemonEvent) => void; onStatus: (status: string) => void }) => { state.onEvent = options.onEvent; state.onStatus = options.onStatus; return { close: state.close }; } }));
vi.mock("../src/desktop", () => ({ openConversation: state.openConversation }));
function storageChange(changes: Record<string, unknown>) { for (const listener of state.storageListeners) listener(changes, "session"); }
// Records one short clip: stop() hands over its data and fires onstop, as Chrome's recorder does.
class FakeRecorder {
  static isTypeSupported = () => true;
  state = "inactive";
  mimeType = "audio/webm";
  ondataavailable: ((event: { data: Blob }) => void) | null = null;
  onstop: (() => void) | null = null;
  start() { this.state = "recording"; }
  stop() { this.state = "inactive"; this.ondataavailable?.({ data: new Blob(["clip"], { type: "audio/webm" }) }); this.onstop?.(); }
}
beforeEach(async () => {
  vi.resetModules(); vi.clearAllMocks(); state.storageListeners.length = 0; state.tabListeners.length = 0; state.activeTab = 42; state.onEvent = undefined; state.onStatus = undefined;
  state.session = { pairing: { url: "http://localhost:4321", token: "private" }, "sessions:42": ["session-1"] };
  state.micPermission = "granted";
  Object.defineProperty(navigator, "permissions", { configurable: true, value: { query: vi.fn(async () => ({ state: state.micPermission })) } });
  Object.defineProperty(navigator, "mediaDevices", { configurable: true, value: { getUserMedia: vi.fn(async () => ({ getTracks: () => [{ stop: vi.fn() }] })) } });
  vi.stubGlobal("MediaRecorder", FakeRecorder);
  document.documentElement.innerHTML = new DOMParser().parseFromString(readFileSync(`${process.cwd()}/static/panel.html`, "utf8"), "text/html").documentElement.innerHTML;
  vi.stubGlobal("chrome", {
    tabs: { query: vi.fn(async () => [{ id: state.activeTab, url: "https://site.example.test", title: "Page" }]), get: vi.fn(async () => ({ url: "https://site.example.test", title: "Page" })), create: vi.fn(async () => ({ id: 99 })), onActivated: { addListener: (listener: () => void) => state.tabListeners.push(listener) } },
    storage: {
      session: {
        get: vi.fn(async (key: string) => (key in state.session ? { [key]: state.session[key] } : {})),
        set: vi.fn(async (items: Record<string, unknown>) => { Object.assign(state.session, items); }),
        remove: vi.fn(async (key: string) => { delete state.session[key]; }),
      },
      local: { get: vi.fn(async () => ({})) },
      onChanged: { addListener: (listener: (changes: Record<string, unknown>, area: string) => void) => state.storageListeners.push(listener) },
    },
    scripting: { executeScript: vi.fn(async () => [{ result: "Selected words" }]) }, runtime: { openOptionsPage: vi.fn(), sendMessage: vi.fn(async () => false), getURL: (path: string) => `chrome-extension://openside/${path}` },
    windows: { getCurrent: vi.fn(async () => ({ id: 7 })) },
    commands: { getAll: vi.fn(async () => [{ name: "start-lasso", shortcut: "⌥⇧L" }]) },
    sidePanel: { close: vi.fn(async () => undefined) },
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
    state.api.listSessions.mockResolvedValueOnce({ sessions: [{ id: "session-1", title: "Pricing comparison" }] } as never);
    state.onEvent!({ kind: "session.title_proposed", seq: 1 } as unknown as DaemonEvent);
    const picker = document.querySelector<HTMLElement>("#sessions")!;
    await vi.waitFor(() => expect(picker.querySelector(".picker-label")?.textContent).toBe("Pricing comparison"));
    expect(picker.dataset.value).toBe("session-1");
    expect(picker.title).toBe("Pricing comparison");
  });
  it("shows one line to connect instead of an error when the pairing is gone", async () => {
    vi.mocked(chrome.storage.session.get).mockImplementation((async () => ({})) as never);
    for (const listener of state.storageListeners) listener({ pairing: { oldValue: {} } }, "session");
    await vi.waitFor(() => expect(document.body.dataset.paired).toBe("false"));
    expect(state.close).toHaveBeenCalled();
    expect(document.querySelector("#transcript")?.children.length).toBe(0);
    expect(document.querySelector("#status")?.textContent).toBe("");
    expect(document.querySelector(".connect-strip")?.textContent).toBe("Open openside to connect");
    document.querySelector<HTMLButtonElement>("#pair-start")!.click();
    expect(chrome.runtime.openOptionsPage).toHaveBeenCalled();
  });
  it("waits for the desktop launcher's own pairing instead of offering manual pairing", async () => {
    vi.mocked(chrome.storage.session.get).mockImplementation((async () => ({})) as never);
    vi.mocked(chrome.runtime.sendMessage).mockImplementation((async () => true) as never);
    for (const listener of state.storageListeners) listener({ pairing: { oldValue: {} } }, "session");
    await vi.waitFor(() => expect(chrome.runtime.sendMessage).toHaveBeenCalledWith({ type: "openside.launcher.adopt" }));
    await vi.mocked(chrome.runtime.sendMessage).mock.results[0].value;
    expect(document.body.dataset.paired).not.toBe("false");
    expect(document.querySelector("#status")?.textContent).toBe("");
  });
  it("forgets a launcher pairing revoked in Devices, quietly, so the panel offers pairing again", async () => {
    // beforeEach resets modules before loading the panel; only this registry's ApiError passes its instanceof check.
    const { ApiError } = await import("../../web/src/api/client");
    vi.mocked(chrome.storage.session.get).mockImplementation((async (key: string) => ({ [key]: key === "pairing" ? { url: "http://localhost:4321", token: "revoked", launcher: true } : [] })) as never);
    state.api.listSessions.mockRejectedValueOnce(new ApiError(401, "unauthorized"));
    for (const listener of state.storageListeners) listener({ pairing: { oldValue: {} } }, "session");
    await vi.waitFor(() => expect(chrome.storage.session.remove).toHaveBeenCalledWith("pairing"));
    expect(document.querySelector("#status")?.textContent).toBe("");
  });
  it("keeps Send unavailable until the panel is ready, so nothing is sent without a conversation picker", async () => {
    window.dispatchEvent(new Event("pagehide"));
    vi.resetModules();
    document.documentElement.innerHTML = new DOMParser().parseFromString(readFileSync(`${process.cwd()}/static/panel.html`, "utf8"), "text/html").documentElement.innerHTML;
    const locale = Promise.withResolvers<Record<string, unknown>>();
    vi.mocked(chrome.storage.local.get).mockImplementationOnce((() => locale.promise) as never);
    // A fresh panel whose stored language has not loaded yet.
    await import("../src/panel");
    const send = document.querySelector<HTMLButtonElement>('#compose button[type="submit"]')!;
    expect(send.disabled).toBe(true);
    document.querySelector<HTMLTextAreaElement>("#message")!.value = "Too early";
    expect(() => document.querySelector<HTMLFormElement>("#compose")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }))).not.toThrow();
    locale.resolve({});
    await vi.waitFor(() => expect(send.disabled).toBe(false));
    expect(state.api.submitMessage).not.toHaveBeenCalled();
    expect(chrome.runtime.sendMessage).not.toHaveBeenCalledWith(expect.objectContaining({ type: "start" }));
    expect(document.querySelector("#status")?.textContent).toBe("");
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
// Picks `label` in one of the composer's chips (#confirmation, #model, #effort).
function choose(root: string, label: string) {
  document.querySelector<HTMLButtonElement>(`${root} .picker-button`)!.click();
  [...document.querySelectorAll<HTMLElement>(`${root} [role=option]`)].find(option => option.textContent === label)!.click();
}
function submit(text: string) {
  document.querySelector<HTMLTextAreaElement>("#message")!.value = text;
  document.querySelector<HTMLFormElement>("#compose")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
}
type Sent = [string, { text: string; attachments?: { name: string; data_base64: string }[] }];
describe("Ask side panel like Aside's docked chat", () => {
  it("starts a new chat, pops the shown chat out to openside and closes the panel from the header", async () => {
    const popOut = document.querySelector<HTMLButtonElement>("#pop-out")!;
    await vi.waitFor(() => expect(popOut.disabled).toBe(false));
    popOut.click();
    expect(state.openConversation).toHaveBeenCalledWith("session-1");
    document.querySelector<HTMLButtonElement>("#new-chat")!.click();
    expect(document.querySelector<HTMLElement>("#sessions")!.dataset.value).toBe("");
    expect(document.querySelector("#sessions .picker-label")?.textContent).toBe("New chat");
    expect(popOut.disabled).toBe(true);
    expect(document.activeElement).toBe(document.querySelector("#message"));
    document.querySelector<HTMLButtonElement>("#close")!.click();
    const sidePanel = (chrome as unknown as { sidePanel: { close: (options: { windowId: number }) => Promise<void> } }).sidePanel;
    await vi.waitFor(() => expect(sidePanel.close).toHaveBeenCalledWith({ windowId: 7 }));
  });
  it("offers Attach this tab, Upload photos & files and Capture a region under +, and Capture starts the lasso on this tab", () => {
    const plus = document.querySelector<HTMLButtonElement>("#plus")!;
    plus.click();
    expect(plus.getAttribute("aria-expanded")).toBe("true");
    expect([...document.querySelectorAll("#plus-menu [role=menuitem]")].map(item => item.textContent)).toEqual(["Attach this tab", "Upload photos & files", "Capture a region"]);
    document.querySelector<HTMLButtonElement>("#capture")!.click();
    expect(chrome.runtime.sendMessage).toHaveBeenCalledWith({ type: "lasso.start", tabId: 42 });
    expect(document.querySelector<HTMLElement>("#plus-menu")!.hidden).toBe(true);
  });
  it("tells how to capture from the page when the panel click can't reach the site", async () => {
    const sendMessage = vi.mocked(chrome.runtime.sendMessage);
    sendMessage.mockImplementation((async () => ({ error: "no_access" })) as never);
    document.querySelector<HTMLButtonElement>("#capture")!.click();
    await vi.waitFor(() => expect(document.querySelector("#status")!.textContent).toBe("To capture here, press ⌥⇧L on the page or right-click › Capture."));
    // Without a shortcut assigned, only the right-click route is offered.
    vi.mocked(chrome.commands.getAll).mockResolvedValue([{ name: "start-lasso", shortcut: "" }] as never);
    document.querySelector<HTMLButtonElement>("#capture")!.click();
    await vi.waitFor(() => expect(document.querySelector("#status")!.textContent).toBe("To capture here, right-click the page and choose Capture."));
  });
  it("shows the usual failure when starting a capture fails for another reason", async () => {
    vi.mocked(chrome.runtime.sendMessage).mockImplementation((async () => ({ error: "unreachable" })) as never);
    document.querySelector<HTMLButtonElement>("#capture")!.click();
    await vi.waitFor(() => expect(document.querySelector("#status")!.textContent).toBe("Cannot reach openside. Check pairing and token scopes."));
    expect(chrome.commands.getAll).not.toHaveBeenCalled();
  });
  it("attaches the tab's address and title without the selection when the page refuses the panel's script", async () => {
    vi.mocked(chrome.scripting.executeScript).mockRejectedValue(new Error("Cannot access contents of the page. Extension manifest must request permission to access the respective host."));
    document.querySelector<HTMLButtonElement>("#attach")!.click();
    await vi.waitFor(() => expect(document.querySelector("#context")?.textContent).toBe("Attached: Page · https://site.example.test"));
    expect(document.querySelector("#status")!.textContent).toBe("");
    submit("Explain");
    await vi.waitFor(() => expect(state.api.submitMessage).toHaveBeenCalled());
    const [, message] = state.api.submitMessage.mock.calls[0] as unknown as Sent;
    expect(message.text).toContain("https://site.example.test");
    expect(message.text).not.toContain("Selected words");
  });
  it("says how to ask about the page instead when neither the page nor its address can be read", async () => {
    vi.mocked(chrome.scripting.executeScript).mockRejectedValue(new Error("Cannot access a chrome:// URL"));
    vi.mocked(chrome.tabs.get).mockResolvedValue({ title: "" } as never);
    document.querySelector<HTMLButtonElement>("#attach")!.click();
    await vi.waitFor(() => expect(document.querySelector("#status")!.textContent).toBe("openside can't read this page from the side panel. Select text and right-click › Ask openside instead."));
    expect(document.querySelector("#context")?.textContent).toBe("");
  });
  it("sends uploaded files with the next message and refuses picks beyond the daemon's limits", async () => {
    const input = document.querySelector<HTMLInputElement>("#file-input")!;
    const pick = (files: File[]) => { Object.defineProperty(input, "files", { value: files, configurable: true }); input.dispatchEvent(new Event("change")); };
    // jsdom's File has no arrayBuffer(); the browser's does.
    const note = Object.assign(new File(["hello"], "note.txt"), { arrayBuffer: async () => new TextEncoder().encode("hello").buffer });
    pick([note]);
    await vi.waitFor(() => expect(document.querySelector("#files")?.textContent).toBe("note.txt"));
    pick([new File([new Uint8Array(3 * 1024 * 1024)], "big.png")]);
    expect(document.querySelector("#status")?.textContent).toBe("Attachments are limited to 2 MiB in total.");
    pick(Array.from({ length: 4 }, (_, index) => new File(["x"], `f${index}.txt`)));
    expect(document.querySelector("#status")?.textContent).toBe("A message can carry at most 4 attachments.");
    submit("");
    await vi.waitFor(() => expect(state.api.submitMessage).toHaveBeenCalled());
    const [id, message] = state.api.submitMessage.mock.calls[0] as unknown as Sent;
    expect(id).toBe("session-1");
    expect(message.attachments).toEqual([{ name: "note.txt", data_base64: btoa("hello") }]);
    await vi.waitFor(() => expect(document.querySelector("#files")?.children.length).toBe(0));
  });
  it("shows the open chat's confirmation, model and effort and changes them on that chat", async () => {
    await vi.waitFor(() => expect(document.querySelector("#model .picker-label")?.textContent).toBe("GPT Test"));
    expect(document.querySelector("#effort .picker-label")?.textContent).toBe("High");
    expect(document.querySelector<HTMLElement>("#confirmation")!.dataset.value).toBe("ask_first");
    choose("#effort", "Max");
    await vi.waitFor(() => expect(state.api.updateSession).toHaveBeenCalledWith("session-1", { effort: "max" }));
    await vi.waitFor(() => expect(document.querySelector("#effort .picker-label")?.textContent).toBe("Max"));
    choose("#model", "GPT Mini");
    await vi.waitFor(() => expect(state.api.updateSession).toHaveBeenCalledWith("session-1", { model: "openai:gpt-mini" }));
    choose("#confirmation", "Skip confirmation");
    await vi.waitFor(() => expect(state.api.updateSession).toHaveBeenCalledWith("session-1", { confirmation: "skip" }));
    await vi.waitFor(() => expect(document.querySelector<HTMLElement>("#confirmation")!.dataset.value).toBe("skip"));
  });
  it("starts a new chat with the options picked before its first message", async () => {
    await vi.waitFor(() => expect(document.querySelector("#model .picker-label")?.textContent).toBe("GPT Test"));
    document.querySelector<HTMLButtonElement>("#new-chat")!.click();
    choose("#confirmation", "Skip confirmation");
    choose("#effort", "Low");
    expect(state.api.updateSession).not.toHaveBeenCalled();
    submit("Hello");
    await vi.waitFor(() => expect(state.api.createSession).toHaveBeenCalledWith(expect.objectContaining({ confirmation: "skip", effort: "low" })));
    await vi.waitFor(() => expect(state.api.submitMessage).toHaveBeenCalledWith("created", expect.objectContaining({ text: "Hello" })));
    expect(state.session["sessions:42"]).toEqual(["created", "session-1"]);
  });
  it("turns Send into Stop while the chat works and stops it", async () => {
    const send = document.querySelector<HTMLButtonElement>('#compose button[type="submit"]')!;
    const stop = document.querySelector<HTMLButtonElement>("#stop")!;
    const composer = document.querySelector<HTMLTextAreaElement>("#message")!;
    expect(stop.hidden).toBe(true);
    state.onEvent!({ kind: "turn.started", seq: 1 } as DaemonEvent);
    expect(send.hidden).toBe(true);
    expect(stop.hidden).toBe(false);
    stop.click();
    await vi.waitFor(() => expect(state.api.stopSession).toHaveBeenCalledWith("session-1"));
    composer.value = "And then";
    composer.dispatchEvent(new Event("input"));
    expect(send.hidden).toBe(false);
    composer.value = "";
    composer.dispatchEvent(new Event("input"));
    state.onEvent!({ kind: "turn.ended", seq: 2 } as DaemonEvent);
    expect(stop.hidden).toBe(true);
    // Nothing typed and nothing running: the circle is the microphone, as in Aside.
    expect(send.hidden).toBe(true);
    expect(document.querySelector("#mic")!.className).toContain("composer-mic-circle");
  });
  it("takes a hand-off with text once and sends it, with its capture, into the chat the panel shows", async () => {
    const ask = { text: "Summarize this.", context: { url: "https://site.example.test", title: "Page", selection: "" }, attachments: [{ name: "capture.png", data_base64: "AAAA" }] };
    state.session["panelAsk:42"] = ask;
    storageChange({ "panelAsk:42": { newValue: ask } });
    storageChange({ "panelAsk:42": { newValue: ask } });
    await vi.waitFor(() => expect(state.api.submitMessage).toHaveBeenCalled());
    // Each wake-up reads the slot once (load, then two changes); only the first finds the ask.
    await vi.waitFor(() => expect(vi.mocked(chrome.storage.session.get).mock.calls.filter(call => String(call[0]) === "panelAsk:42")).toHaveLength(3));
    expect(state.api.submitMessage).toHaveBeenCalledTimes(1);
    const [id, message] = state.api.submitMessage.mock.calls[0] as unknown as Sent;
    expect(id).toBe("session-1");
    expect(message.text).toContain("Summarize this.");
    expect(message.text).toContain("https://site.example.test");
    expect(message.attachments).toEqual(ask.attachments);
    expect(state.session["panelAsk:42"]).toBeUndefined();
  });
  it("puts a hand-off without text into the composer as context, focused, for the person to finish", async () => {
    const ask = { text: "", context: { url: "https://site.example.test/a", title: "Article", selection: "picked words" } };
    state.session["panelAsk:42"] = ask;
    storageChange({ "panelAsk:42": { newValue: ask } });
    await vi.waitFor(() => expect(document.querySelector("#context")?.textContent).toBe("Attached: Article · https://site.example.test/a"));
    expect(document.activeElement).toBe(document.querySelector("#message"));
    expect(state.api.submitMessage).not.toHaveBeenCalled();
    submit("Explain");
    await vi.waitFor(() => expect(state.api.submitMessage).toHaveBeenCalled());
    const [, message] = state.api.submitMessage.mock.calls[0] as unknown as Sent;
    expect(message.text).toContain("picked words");
  });
  it("starts a new chat for a hand-off while the panel shows New chat", async () => {
    document.querySelector<HTMLButtonElement>("#new-chat")!.click();
    const ask = { text: "Translate this into English.", context: { url: "https://site.example.test", title: "Page", selection: "bonjour" } };
    state.session["panelAsk:42"] = ask;
    storageChange({ "panelAsk:42": { newValue: ask } });
    await vi.waitFor(() => expect(state.api.submitMessage).toHaveBeenCalledWith("created", expect.objectContaining({ text: expect.stringContaining("bonjour") as never })));
    await vi.waitFor(() => expect(document.querySelector<HTMLElement>("#sessions")!.dataset.value).toBe("created"));
  });
  it("names a message's attachments under its bubble", () => {
    state.onEvent!({ kind: "message.queued", message_id: "m1", text: "Look", seq: 1, author_kind: "person", from_agent: null, attachments: [{ kind: "image", mime: "image/png", name: "capture.png", path: "artifacts/m1/capture.png", sha256: "0", size: 3 }] } as unknown as DaemonEvent);
    expect(document.querySelector("#transcript .bubble.user .file-chip")?.textContent).toBe("capture.png");
  });
  it("puts copy, branch and the time under a reply, and branches into a new chat for this tab", async () => {
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(window, "isSecureContext", { configurable: true, value: true });
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    state.onEvent!({ kind: "message.queued", message_id: "m1", text: "Compare", seq: 1, author_kind: "person", from_agent: null } as unknown as DaemonEvent);
    state.onEvent!({ kind: "assistant.final", text: "Answer", seq: 2, ts: "2026-01-01T09:07:00Z" } as unknown as DaemonEvent);
    const footer = document.querySelector<HTMLElement>("#transcript .bubble.assistant .message-footer")!;
    const [copy, branch] = footer.querySelectorAll<HTMLButtonElement>(".footer-button");
    expect(copy!.getAttribute("aria-label")).toBe("Copy message");
    expect(footer.querySelector("time")!.dateTime).toBe("2026-01-01T09:07:00Z");
    expect(footer.querySelector("time")!.textContent).toMatch(/\d{1,2}:07/);
    // Branching needs the person's message to have finished.
    expect(branch!.hidden).toBe(true);
    state.onEvent!({ kind: "message.completed", message_id: "m1", outcome: "completed", reason: null, seq: 3 } as unknown as DaemonEvent);
    expect(branch!.hidden).toBe(false);
    expect(branch!.getAttribute("aria-label")).toBe("Branch from here");
    copy!.click();
    await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith("Answer"));
    state.api.listSessions.mockImplementation(async () => ({ sessions: [
      { id: "session-1", title: "Current tab question", model: "openai:gpt-test", effort: null, confirmation: "ask_first", status: "idle", context: { used_tokens: 50000, window_tokens: 200000 } },
      { id: "branch-1", title: "Compare (branch)", model: "openai:gpt-test", effort: null, confirmation: "ask_first", status: "idle", context: { used_tokens: 50000, window_tokens: 200000 } },
    ] }) as never);
    branch!.click();
    await vi.waitFor(() => expect(document.querySelector<HTMLElement>("#sessions")!.dataset.value).toBe("branch-1"));
    expect(state.api.forkSession).toHaveBeenCalledWith("session-1", { from_message: "m1" });
    expect(state.session["sessions:42"]).toEqual(["branch-1", "session-1"]);
  });
  it("says a branch can't be made yet when the daemon reports the message unfinished", async () => {
    const { ApiError } = await import("../../web/src/api/client");
    state.api.forkSession.mockRejectedValueOnce(new ApiError(409, "message_not_finished", "not finished"));
    state.onEvent!({ kind: "message.queued", message_id: "m1", text: "Compare", seq: 1, author_kind: "person", from_agent: null } as unknown as DaemonEvent);
    state.onEvent!({ kind: "message.completed", message_id: "m1", outcome: "completed", reason: null, seq: 2 } as unknown as DaemonEvent);
    state.onEvent!({ kind: "assistant.final", text: "Answer", seq: 3, ts: "2026-01-01T09:07:00Z" } as unknown as DaemonEvent);
    document.querySelectorAll<HTMLButtonElement>(".message-footer .footer-button")[1]!.click();
    await vi.waitFor(() => expect(document.querySelector("#status")!.textContent).toBe("This reply isn’t finished yet. Branch from it once it’s done."));
  });
  it("shows how full the open chat's context is and rereads it when a turn ends", async () => {
    const ring = document.querySelector<HTMLElement>("#ring")!;
    await vi.waitFor(() => expect(ring.hidden).toBe(false));
    expect(ring.getAttribute("aria-label")).toBe("Context 25% full · 50K of 200K tokens");
    state.onEvent!({ kind: "turn.ended", seq: 2 } as DaemonEvent);
    await vi.waitFor(() => expect(ring.getAttribute("aria-label")).toBe("Context 95% full · 190K of 200K tokens"));
    expect(ring.classList.contains("is-full")).toBe(true);
    document.querySelector<HTMLButtonElement>("#new-chat")!.click();
    await vi.waitFor(() => expect(ring.hidden).toBe(true));
  });
  it("shows the mic as the circle on an empty box and beside Send once something is typed", async () => {
    const mic = document.querySelector<HTMLButtonElement>("#mic")!;
    const send = document.querySelector<HTMLButtonElement>('#compose button[type="submit"]')!;
    const composer = document.querySelector<HTMLTextAreaElement>("#message")!;
    await vi.waitFor(() => expect(mic.hasAttribute("aria-disabled")).toBe(false));
    expect(mic.className).toBe("composer-send composer-mic-circle");
    expect(mic.getAttribute("aria-label")).toBe("Dictate");
    expect(send.hidden).toBe(true);
    composer.value = "Hello";
    composer.dispatchEvent(new Event("input"));
    expect(mic.className).toBe("composer-mic");
    expect(send.hidden).toBe(false);
  });
  it("records, transcribes through the daemon and puts the words into the box", async () => {
    // jsdom's Blob has no arrayBuffer(); Chrome's does.
    Object.defineProperty(Blob.prototype, "arrayBuffer", { configurable: true, value(this: Blob) { return new Promise(resolve => { const reader = new FileReader(); reader.onload = () => resolve(reader.result); reader.readAsArrayBuffer(this); }); } });
    const mic = document.querySelector<HTMLButtonElement>("#mic")!;
    const composer = document.querySelector<HTMLTextAreaElement>("#message")!;
    composer.value = "Say";
    composer.dispatchEvent(new Event("input"));
    await vi.waitFor(() => expect(mic.hasAttribute("aria-disabled")).toBe(false));
    mic.click();
    await vi.waitFor(() => expect(mic.classList.contains("is-recording")).toBe(true));
    expect(mic.getAttribute("aria-label")).toBe("Stop dictating");
    mic.click();
    await vi.waitFor(() => expect(composer.value).toBe("Say spoken words"));
    expect(state.api.transcribe).toHaveBeenCalledWith(expect.objectContaining({ mime: "audio/webm", audio_base64: expect.any(String) as never }), expect.any(AbortSignal));
    expect(mic.classList.contains("is-recording")).toBe(false);
    delete (Blob.prototype as { arrayBuffer?: unknown }).arrayBuffer;
  });
  it("opens a tab to ask for the microphone, because a side panel can't show the prompt", async () => {
    state.micPermission = "prompt";
    const mic = document.querySelector<HTMLButtonElement>("#mic")!;
    await vi.waitFor(() => expect(mic.hasAttribute("aria-disabled")).toBe(false));
    mic.click();
    await vi.waitFor(() => expect(chrome.tabs.create).toHaveBeenCalledWith({ url: "chrome-extension://openside/mic-access.html" }));
    expect(navigator.mediaDevices.getUserMedia).not.toHaveBeenCalled();
    expect(document.querySelector("#status")!.textContent).toBe("Allow the microphone in the tab that just opened, then press the mic again.");
  });
  it("keeps the mic visible but unavailable, with the reason, when the daemon has no speech-to-text", async () => {
    state.api.sttStatus.mockResolvedValue({ available: false, reason: "stt_unavailable", max_bytes: 0, max_seconds: 0 } as never);
    try {
      vi.resetModules();
      document.documentElement.innerHTML = new DOMParser().parseFromString(readFileSync(`${process.cwd()}/static/panel.html`, "utf8"), "text/html").documentElement.innerHTML;
      await import("../src/panel");
      const mic = document.querySelector<HTMLButtonElement>("#mic")!;
      await vi.waitFor(() => expect(mic.title).toBe("Voice input needs an OpenAI API key in Settings › Models."));
      expect(mic.getAttribute("aria-disabled")).toBe("true");
    } finally {
      state.api.sttStatus.mockResolvedValue({ available: true, reason: null, max_bytes: 10_000_000, max_seconds: 120 });
    }
  });
});
