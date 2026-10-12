import type { AuthButton } from "../../web/src/otp/authCard";
import { isApprovalTicket, isChoiceTicket, isLoginTicket, mountApprovalCard, mountChoiceForm, mountLoginForm } from "../../web/src/otp/loginForm";
import { isOtpTicket, mountOtpForm, otpRegistrationOf, registrationNote, registrationQuestion } from "../../web/src/otp/otpForm";
import { KIND_LABEL, siteNote, TAKEOVER_NOTE } from "../../web/src/otp/ticketCopy";
import { client, openSessionStream, type Client, type Ticket } from "./api";
import { hasContextMention, withContext, type TabContext } from "./context";
import { renderMarkdown } from "./markdown";
import { pairing, tabSessions } from "./storage";
import { bindStatus, icon, mountIcons, type IconName } from "./ui";
const select = document.querySelector<HTMLSelectElement>("#sessions")!;
const composer = document.querySelector<HTMLTextAreaElement>("#message")!;
const status = document.querySelector<HTMLElement>("#status")!;
const transcript = document.querySelector<HTMLElement>("#transcript")!;
const connection = document.querySelector<HTMLElement>("#connection")!;
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
  const chip = document.querySelector<HTMLElement>("#context")!;
  chip.textContent = `Attached: ${context.title} · ${context.url}`;
  chip.title = chip.textContent;
}
function button(parent: HTMLElement, label: string, variant: "primary" | "secondary" | "ghost", action: () => Promise<unknown>): void {
  const element = document.createElement("button");
  element.type = "button";
  element.className = `button compact${variant === "secondary" ? "" : ` ${variant}`}`;
  element.textContent = label;
  element.addEventListener("click", () => { element.disabled = true; void action().then(refreshTickets).catch(fail).finally(() => { element.disabled = false; }); });
  parent.append(element);
}
// Labels and notes come from web/src/otp/ticketCopy.ts; only the icon set is the panel's own.
const ticketIcons: Record<Ticket["kind"], IconName> = {
  confirm: "shield-check",
  info: "message-circle-question",
  secret_input: "key-round",
  device_approval: "monitor-smartphone",
  takeover: "hand",
  // Pending logins and choices render as the shared cards; these name them elsewhere.
  login: "key-round",
  choice: "message-circle-question",
};
const authButtons: Record<AuthButton, string> = { primary: "button compact primary", secondary: "button compact", ghost: "button compact ghost" };
function paragraph(parent: HTMLElement, className: string, text: string): void {
  if (!text) return;
  const element = document.createElement("p"); element.className = className; element.textContent = text; parent.append(element);
}
function renderTicket(ticket: Ticket, api: Client): HTMLElement {
  const card = document.createElement("article");
  card.className = "ticket";
  card.dataset.ticket = ticket.id;
  // The shared web cards: autofill-friendly inputs, values emptied before anything is sent.
  const shared = { client: api, buttonClass: (variant: AuthButton) => authButtons[variant], onSubmitted: () => { void refreshTickets().catch(fail); } };
  if (isOtpTicket(ticket) || isLoginTicket(ticket) || isChoiceTicket(ticket) || isApprovalTicket(ticket)) {
    // Same markup as web Timeline: the chip sits above the card while the person has to act.
    card.classList.add("ticket-otp", "ticket-auth", "ticket-anywhere", "ticket-row");
    const chip = document.createElement("span"); chip.className = "action-chip"; chip.dataset.tone = "warn";
    const dot = document.createElement("span"); dot.className = "action-chip-dot"; dot.setAttribute("aria-hidden", "true");
    chip.append(dot, "Action needed"); card.append(chip);
    if (isLoginTicket(ticket)) mountLoginForm(card, { ...shared, ticket });
    else if (isChoiceTicket(ticket)) mountChoiceForm(card, { ...shared, ticket });
    else if (isApprovalTicket(ticket)) mountApprovalCard(card, { ...shared, ticket });
    else mountOtpForm(card, { ...shared, ticket });
    return card;
  }
  const registration = otpRegistrationOf(ticket);
  const [kindIcon, kindLabel] = registration ? ["key-round" as const, "Save authenticator"] : [ticketIcons[ticket.kind], KIND_LABEL[ticket.kind]];
  const head = document.createElement("div"); head.className = "ticket-head";
  const kind = document.createElement("span"); kind.className = "ticket-kind"; kind.textContent = kindLabel;
  const kindGlyph = icon(kindIcon, 16); kindGlyph.classList.add("ticket-icon");
  head.append(kindGlyph, kind); card.append(head);
  card.setAttribute("aria-label", kindLabel);
  const summary = document.createElement("strong"); summary.className = "ticket-summary"; summary.textContent = registration ? registrationQuestion(registration) : ticket.summary;
  card.append(summary);
  const actions = document.createElement("div"); actions.className = "ticket-actions";
  if (registration) {
    card.classList.add("ticket-anywhere");
    paragraph(card, "ticket-note", registrationNote(registration));
    button(actions, "Save to vault", "primary", () => api.resolveTicket(ticket.id, true));
    button(actions, "Not now", "secondary", () => api.resolveTicket(ticket.id, false));
    card.append(actions);
    return card;
  }
  paragraph(card, "ticket-note", ticket.instructions ?? "");
  paragraph(card, "ticket-site", ticket.target?.origin ? siteNote(ticket.target.origin) : "");
  if (ticket.kind === "confirm") button(actions, "Approve", "primary", () => api.resolveTicket(ticket.id, true));
  else if (ticket.kind === "info" && ticket.options.length) for (const option of ticket.options) button(actions, option.label, "secondary", () => api.resolveTicket(ticket.id, option.value));
  else if (ticket.kind === "info" || ticket.kind === "secret_input") {
    const input = document.createElement("input"); input.type = ticket.kind === "secret_input" ? "password" : "text"; input.autocomplete = "off"; input.placeholder = ticket.kind === "secret_input" ? "Secret value" : "Answer"; input.setAttribute("aria-label", ticket.kind === "secret_input" ? "Secret" : "Answer"); card.append(input);
    button(actions, ticket.kind === "secret_input" ? "Submit" : "Send", "primary", async () => { const value = input.value; input.value = ""; return api.resolveTicket(ticket.id, value); });
  } else {
    if (ticket.kind === "takeover" && !ticket.instructions) paragraph(card, "ticket-note", TAKEOVER_NOTE);
    button(actions, "Done", "primary", () => api.ticketDone(ticket.id));
  }
  button(actions, ticket.kind === "info" ? "Dismiss" : "Reject", "ghost", () => api.rejectTicket(ticket.id));
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
  const shown = result.tickets.filter(ticket => ticket.session_id === selected || isOtpTicket(ticket) || otpRegistrationOf(ticket) || isLoginTicket(ticket) || isChoiceTicket(ticket) || isApprovalTicket(ticket));
  const existing = new Map([...ticketBox.querySelectorAll<HTMLElement>(":scope > .ticket")].map(card => [card.dataset.ticket, card]));
  ticketBox.replaceChildren(...shown.map(ticket => existing.get(ticket.id) ?? renderTicket(ticket, api)));
}
async function watch(): Promise<void> {
  const epoch = ++generation;
  stream?.close(); transcript.replaceChildren(); connection.textContent = "";
  for (const card of ticketBox.querySelectorAll(":scope > .ticket:not(.ticket-anywhere)")) card.remove();
  const id = select.value;
  select.title = select.selectedOptions[0]?.text ?? "";
  if (!id) { await refreshTickets(); return; }
  const api = client(await pairing());
  if (epoch !== generation) return;
  let reply: HTMLElement | undefined;
  let replyText = "";
  const startReply = () => { reply = bubble("assistant"); reply.classList.add("streaming"); replyText = ""; transcript.append(reply); return reply; };
  const connectionNotes: Partial<Record<string, string>> = { connecting: "Connecting…", reconnecting: "Reconnecting…" };
  // A replaced stream still reports "closed" after watch() moved on, so only the current one writes.
  stream = openSessionStream({ client: api, sessionId: id, onError: fail, onStatus: value => { if (epoch === generation) connection.textContent = connectionNotes[value] ?? ""; }, onEvent: event => {
    if (event.kind === "message.queued") followLatest(() => { const item = bubble("user"); item.textContent = event.text; transcript.append(item); });
    if (event.kind === "turn.started") followLatest(startReply);
    if (event.kind === "turn.ended") reply?.classList.remove("streaming");
    if (event.kind === "assistant.delta" || event.kind === "assistant.final") followLatest(() => {
      const target = reply ?? startReply();
      replyText = event.kind === "assistant.final" ? event.text : replyText + event.text;
      renderMarkdown(target, replyText);
      if (event.kind === "assistant.final") target.classList.remove("streaming");
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
  select.replaceChildren(new Option("New chat", ""), ...sessions.map(session => new Option(session.title, session.id)));
  select.value = ids.has(selected) ? selected : "";
  // The picker truncates long titles with an ellipsis; the tooltip carries the full one.
  select.title = select.selectedOptions[0]?.text ?? "";
}
select.addEventListener("change", () => { void watch().catch(fail); });
document.querySelector("#attach")!.addEventListener("click", () => { void attach().catch(fail); });
composer.addEventListener("input", () => { if (hasContextMention(composer.value)) void attach().catch(fail); });
for (const pair of document.querySelectorAll("#pair, #pair-start")) pair.addEventListener("click", () => { void chrome.runtime.openOptionsPage(); });
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
    composer.value = ""; attached = undefined; document.querySelector("#context")!.textContent = "";
  })().catch(fail).finally(() => { submit.disabled = false; });
});
async function initialize(): Promise<void> {
  // Without a pairing every request would fail; show how to pair instead of an error.
  const paired = (await chrome.storage.session.get("pairing")).pairing !== undefined;
  document.body.dataset.paired = String(paired);
  if (!paired) {
    generation += 1;
    stream?.close(); transcript.replaceChildren(); ticketBox.replaceChildren(); connection.textContent = "";
    select.replaceChildren(new Option("New chat", ""));
    return;
  }
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
