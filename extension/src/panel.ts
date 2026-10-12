import { isOtpTicket, mountOtpForm, otpRegistrationOf, registrationQuestion, type OtpButton } from "../../web/src/otp/otpForm";
import { client, openSessionStream, type Ticket } from "./api";
import { hasContextMention, withContext, type TabContext } from "./context";
import { pairing, tabSessions } from "./storage";
import { bindStatus, icon, mountIcons, type IconName } from "./ui";
const select = document.querySelector<HTMLSelectElement>("#sessions")!;
const composer = document.querySelector<HTMLTextAreaElement>("#message")!;
const status = document.querySelector<HTMLElement>("#status")!;
const transcript = document.querySelector<HTMLElement>("#transcript")!;
const ticketBox = document.querySelector<HTMLElement>("#tickets")!;
let tabId: number | undefined;
let attached: TabContext | undefined;
let stream: ReturnType<typeof openSessionStream> | undefined;
let generation = 0;
const notify = bindStatus(status);
const scroller = document.querySelector<HTMLElement>(".scroll")!;
const fail = () => { notify("Cannot reach openside. Check pairing and token scopes.", "error"); };
mountIcons();
// Follow the stream only while the reader is already at the bottom, so reading older text is never interrupted.
function followLatest(update: () => void): void {
  const pinned = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 48;
  update();
  if (pinned) scroller.scrollTop = scroller.scrollHeight;
}
function bubble(role: "user" | "assistant"): HTMLElement {
  const element = document.createElement("article");
  element.className = `bubble ${role}`;
  return element;
}
function fitComposer(): void {
  composer.style.height = "auto";
  composer.style.height = `${Math.min(composer.scrollHeight, 200)}px`;
  composer.style.overflowY = composer.scrollHeight > 200 ? "auto" : "hidden";
}
async function currentContext(sourceTab = tabId): Promise<TabContext> {
  if (sourceTab === undefined) throw new Error("No current tab");
  const tab = await chrome.tabs.get(sourceTab);
  const results = await chrome.scripting.executeScript({ target: { tabId: sourceTab }, func: () => window.getSelection()?.toString() ?? "" });
  return { url: tab.url ?? "", title: tab.title ?? "", selection: results[0]?.result ?? "" };
}
async function attach(): Promise<void> {
  const sourceTab = tabId;
  const context = await currentContext(sourceTab);
  if (sourceTab !== tabId) return;
  attached = context;
  document.querySelector("#context")!.textContent = `Attached: ${context.title} · ${context.url}`;
}
function button(parent: HTMLElement, label: string, variant: "primary" | "secondary" | "danger", action: () => Promise<unknown>): void {
  const element = document.createElement("button");
  element.type = "button";
  element.className = `button compact${variant === "secondary" ? "" : ` ${variant}`}`;
  element.textContent = label;
  element.addEventListener("click", () => { element.disabled = true; void action().then(refreshTickets).catch(fail).finally(() => { element.disabled = false; }); });
  parent.append(element);
}
const ticketIcons: Record<Ticket["kind"], IconName> = { confirm: "shield-check", info: "info", secret_input: "lock", device_approval: "monitor", takeover: "monitor" };
const otpButtons: Record<OtpButton, string> = { primary: "button compact primary", secondary: "button compact", ghost: "button compact" };
function renderTicket(ticket: Ticket, api: ReturnType<typeof client>): HTMLElement {
  const card = document.createElement("article");
  card.className = "ticket";
  card.dataset.ticket = ticket.id;
  // The shared web form: one code field, autofilled codes, Enter submits, cleared before sending.
  if (isOtpTicket(ticket)) {
    card.classList.add("ticket-otp", "ticket-anywhere");
    mountOtpForm(card, { ticket, client: api, buttonClass: variant => otpButtons[variant], onSubmitted: () => { void refreshTickets().catch(fail); } });
    return card;
  }
  const registration = otpRegistrationOf(ticket);
  const head = document.createElement("div"); head.className = "ticket-head";
  const title = document.createElement("strong"); title.textContent = registration ? registrationQuestion(registration) : ticket.summary;
  head.append(icon(registration ? "key-round" : ticketIcons[ticket.kind], 18), title); card.append(head);
  const instructions = document.createElement("p"); instructions.className = "ticket-note";
  instructions.textContent = registration ? "openside fills this site's codes from the vault from now on. The setup key is never shown." : ticket.instructions ?? ticket.target?.origin ?? "";
  card.append(instructions);
  const actions = document.createElement("div"); actions.className = "ticket-actions";
  if (registration) {
    card.classList.add("ticket-anywhere");
    button(actions, "Approve", "primary", () => api.resolveTicket(ticket.id, true));
    button(actions, "Decline", "secondary", () => api.resolveTicket(ticket.id, false));
    card.append(actions);
    return card;
  }
  if (ticket.kind === "confirm") button(actions, "Approve", "primary", () => api.resolveTicket(ticket.id, true));
  else if (ticket.kind === "info" && ticket.options.length) for (const option of ticket.options) button(actions, option.label, "secondary", () => api.resolveTicket(ticket.id, option.value));
  else if (ticket.kind === "info" || ticket.kind === "secret_input") {
    const input = document.createElement("input"); input.type = ticket.kind === "secret_input" ? "password" : "text"; input.autocomplete = "off"; input.setAttribute("aria-label", ticket.kind === "secret_input" ? "Secret" : "Answer"); card.append(input);
    button(actions, "Answer", "primary", async () => { const value = input.value; input.value = ""; return api.resolveTicket(ticket.id, value); });
  } else button(actions, "Done on desktop", "primary", () => api.ticketDone(ticket.id));
  button(actions, "Reject", "danger", () => api.rejectTicket(ticket.id));
  card.append(actions);
  return card;
}
// Code requests from any conversation show here, not only this tab's: the person types the code
// where they already are. Cards are kept by ticket id so a refresh never drops a half-typed code.
async function refreshTickets(): Promise<void> {
  const selected = select.value;
  const api = client(await pairing());
  const result = await api.listTickets("pending");
  if (selected !== select.value) return;
  const shown = result.tickets.filter(ticket => ticket.session_id === selected || isOtpTicket(ticket) || otpRegistrationOf(ticket));
  const existing = new Map([...ticketBox.querySelectorAll<HTMLElement>(":scope > .ticket")].map(card => [card.dataset.ticket, card]));
  ticketBox.replaceChildren(...shown.map(ticket => existing.get(ticket.id) ?? renderTicket(ticket, api)));
}
async function watch(): Promise<void> {
  const epoch = ++generation;
  stream?.close(); transcript.replaceChildren();
  for (const card of ticketBox.querySelectorAll(":scope > .ticket:not(.ticket-anywhere)")) card.remove();
  const id = select.value;
  if (!id) { await refreshTickets(); return; }
  const api = client(await pairing());
  if (epoch !== generation) return;
  let reply: HTMLElement | undefined;
  let connection = "";
  const connectionNotes: Partial<Record<string, string>> = { connecting: "Connecting…", reconnecting: "Reconnecting…" };
  // "closed" also fires after our own close() and after a fatal error already reported through onError, so it never writes.
  stream = openSessionStream({ client: api, sessionId: id, onError: fail, onStatus: value => {
    const note = connectionNotes[value];
    if (note) { connection = note; notify(note); } else if (value === "open" && connection && status.textContent === connection) { connection = ""; notify(""); }
  }, onEvent: event => {
    if (event.kind === "message.queued") followLatest(() => { const item = bubble("user"); item.textContent = event.text; transcript.append(item); });
    if (event.kind === "turn.started") reply = undefined;
    if (event.kind === "assistant.delta" || event.kind === "assistant.final") followLatest(() => {
      if (!reply) { reply = bubble("assistant"); transcript.append(reply); }
      reply.textContent = event.kind === "assistant.final" ? event.text : (reply.textContent ?? "") + event.text;
    });
    if (event.kind.startsWith("ticket.")) void refreshTickets().catch(fail);
  } });
  await refreshTickets();
}
async function refreshSessions(): Promise<void> {
  if (tabId === undefined) return;
  const currentTab = tabId;
  const ids = new Set(await tabSessions(currentTab));
  const selected = select.value;
  const sessions = (await client(await pairing()).listSessions()).sessions.filter(session => ids.has(session.id));
  if (currentTab !== tabId) return;
  select.replaceChildren(new Option("New session", ""), ...sessions.map(session => new Option(session.title, session.id)));
  select.value = ids.has(selected) ? selected : "";
}
select.addEventListener("change", () => { void watch().catch(fail); });
document.querySelector("#attach")!.addEventListener("click", () => { void attach().catch(fail); });
composer.addEventListener("input", () => { fitComposer(); if (hasContextMention(composer.value)) void attach().catch(fail); });
document.querySelector("#pair")!.addEventListener("click", () => { void chrome.runtime.openOptionsPage(); });
document.querySelector<HTMLFormElement>("#compose")!.addEventListener("submit", event => {
  event.preventDefault();
  if (!composer.value.trim() || tabId === undefined) return;
  const text = composer.value;
  const explicitContext = attached;
  const sourceTab = tabId;
  const chosen = select.value;
  const submit = document.querySelector<HTMLButtonElement>('button[type="submit"]')!; submit.disabled = true;
  void (async () => {
    const context = explicitContext ?? (hasContextMention(text) ? await currentContext(sourceTab) : undefined);
    if (chosen) await client(await pairing()).submitMessage(chosen, { text: withContext(text, context), mode: "now", message_id: crypto.randomUUID() });
    else {
      const result = await chrome.runtime.sendMessage({ type: "start", tabId: sourceTab, text, context });
      if (result.error) throw new Error(result.error);
      await refreshSessions(); select.value = result.id; await watch();
    }
    composer.value = ""; fitComposer(); attached = undefined; document.querySelector("#context")!.textContent = "";
  })().catch(fail).finally(() => { submit.disabled = false; });
});
async function initialize(): Promise<void> {
  const active = (await chrome.tabs.query({ active: true, currentWindow: true }))[0];
  const changed = tabId !== active?.id;
  tabId = active?.id;
  if (changed) {
    generation += 1;
    stream?.close(); transcript.replaceChildren();
    for (const card of ticketBox.querySelectorAll(":scope > .ticket:not(.ticket-anywhere)")) card.remove();
    attached = undefined; document.querySelector("#context")!.textContent = "";
    select.value = "";
  }
  await refreshSessions();
  const ids = tabId === undefined ? [] : await tabSessions(tabId);
  if (!select.value && ids.length) { select.value = ids[0]; await watch(); } else if (!select.value) await refreshTickets();
}
chrome.storage.onChanged.addListener((changes, area) => {
  if (area === "session" && (changes.pairing || (tabId !== undefined && changes[`sessions:${tabId}`]))) void initialize().catch(fail);
});
chrome.tabs.onActivated.addListener(() => { void initialize().catch(fail); });
// Code requests from conversations this panel is not streaming arrive by polling. A failed poll
// stays quiet; the stream and the actions report connection problems.
const poll = window.setInterval(() => { if (document.visibilityState === "visible") void refreshTickets().catch(() => undefined); }, 5000);
window.addEventListener("pagehide", () => { stream?.close(); window.clearInterval(poll); });
void initialize().catch(fail);
