import { askerOf, isAttentionTicket } from "../../web/src/cards/attention";
import { mountCard, type MountedCard } from "../../web/src/cards/render";
import type { Action } from "../../web/src/api/types";
import { client, openSessionStream } from "./api";
import { hasContextMention, withContext, type TabContext } from "./context";
import { renderMarkdown } from "./markdown";
import { pairing, tabSessions } from "./storage";
import { createPicker } from "./picker";
import { bindStatus, icon, mountIcons } from "./ui";
import { installScrollReveal } from "../../web/src/ui/scrollReveal";
import { outcomeNote } from "../../web/src/components/toolStatus";
installScrollReveal();
const sessionsRoot = document.querySelector<HTMLElement>("#sessions")!;
const NEW_CHAT = { value: "", label: "New chat" };
const picker = createPicker(sessionsRoot, "Conversation for this tab", () => { void watch().catch(fail); });
picker.setItems([NEW_CHAT], "");
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
type DeliveryState = "sent" | "read" | "failed" | "note";
// Same classes and wording as the web timeline (components/Timeline.tsx); web/src/styles.css styles both.
function deliveryNote(state: DeliveryState, text?: string): HTMLElement {
  const element = document.createElement("p");
  element.className = state === "failed" ? "receipt receipt-failed" : "receipt";
  element.dataset.state = state;
  if (state === "sent" || state === "read") { element.textContent = state === "sent" ? "Sent" : "Read"; return element; }
  const label = document.createElement("span");
  label.textContent = text ?? "";
  if (state === "failed") element.append(icon("circle-alert", 14));
  element.append(label);
  return element;
}
function typingRow(): HTMLElement {
  const row = document.createElement("div");
  row.className = "typing-row";
  const dots = document.createElement("p");
  dots.className = "typing";
  dots.setAttribute("role", "status");
  const label = document.createElement("span");
  label.className = "sr-only";
  label.textContent = "Agent is working";
  dots.append(label);
  for (let dot = 0; dot < 3; dot += 1) {
    const span = document.createElement("span");
    span.className = "typing-dot";
    span.setAttribute("aria-hidden", "true");
    dots.append(span);
  }
  row.append(dots);
  return row;
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
// Request cards are the shared renderer (web/src/cards). The panel can answer, reject, finish and
// open links; targets that need the web app (remote screen, in-app navigation) are not offered.
const PANEL_TARGETS: readonly Action["target"]["type"][] = ["ticket_resolve", "ticket_reject", "ticket_done", "link"];
const supports = (action: Action): boolean => PANEL_TARGETS.includes(action.target.type);
interface PanelCard { host: HTMLElement; mounted: MountedCard; anywhere: boolean }
const ticketCards = new Map<string, PanelCard>();
const presentedCards: MountedCard[] = [];
function cardHost(extra = ""): HTMLElement {
  const host = document.createElement("div");
  host.className = `panel-card${extra}`;
  return host;
}
// Requests that need a human step show here from any conversation, not only this tab's: the person
// answers where they already are. A card stays mounted by ticket id so a refresh never drops a
// half-typed code, and the shared renderer empties its fields itself when the request settles.
async function refreshTickets(): Promise<void> {
  const selected = picker.value;
  const api = client(await pairing());
  const result = await api.listTickets("pending");
  if (selected !== picker.value) return;
  const shown = result.tickets.filter(ticket => ticket.card && (ticket.session_id === selected || isAttentionTicket(ticket)));
  const keep = new Set(shown.map(ticket => ticket.id));
  for (const [id, entry] of ticketCards) if (!keep.has(id)) { entry.mounted.dispose(); entry.host.remove(); ticketCards.delete(id); }
  let cursor = ticketBox.firstElementChild;
  for (const ticket of shown) {
    let entry = ticketCards.get(ticket.id);
    if (entry) entry.mounted.update(ticket.card!);
    else {
      const host = cardHost();
      // The person opened the panel to answer, so a waiting request starts open.
      const mounted = mountCard(host, ticket.card!, { port: api, sessionId: ticket.session_id, supports, focus: "none", defaultOpen: true, asker: askerOf(ticket), onSettled: () => { void refreshTickets().catch(fail); } });
      entry = { host, mounted, anywhere: isAttentionTicket(ticket) };
      ticketCards.set(ticket.id, entry);
    }
    if (entry.host === cursor) cursor = cursor.nextElementSibling;
    else ticketBox.insertBefore(entry.host, cursor);
  }
}
async function watch(): Promise<void> {
  const epoch = ++generation;
  stream?.close(); transcript.replaceChildren(); connection.textContent = "";
  for (const [id, entry] of ticketCards) if (!entry.anywhere) { entry.mounted.dispose(); entry.host.remove(); ticketCards.delete(id); }
  for (const mounted of presentedCards.splice(0)) mounted.dispose();
  const id = picker.value;
  sessionsRoot.title = picker.label;
  if (!id) { await refreshTickets(); return; }
  const api = client(await pairing());
  if (epoch !== generation) return;
  let reply: HTMLElement | undefined;
  let replyText = "";
  // Only the newest message carries a delivery note, like the web timeline.
  let lastBubble: HTMLElement | undefined;
  let lastId = "";
  let note: HTMLElement | undefined;
  let typing: HTMLElement | undefined;
  const startReply = () => { reply = bubble("assistant"); replyText = ""; transcript.append(reply); return reply; };
  const setNote = (state?: DeliveryState, text?: string) => { note?.remove(); note = state && lastBubble ? deliveryNote(state, text) : undefined; if (note) lastBubble!.after(note); };
  const setTyping = (on: boolean) => { if (on && !typing) { typing = typingRow(); transcript.append(typing); } else if (!on) { typing?.remove(); typing = undefined; } };
  // Sent and Read go away once the agent answers; a failure or stop note stays until the next message.
  const settleNote = () => { if (note?.dataset.state === "sent" || note?.dataset.state === "read") setNote(); };
  const connectionNotes: Partial<Record<string, string>> = { connecting: "Connecting…", reconnecting: "Reconnecting…" };
  // A replaced stream still reports "closed" after watch() moved on, so only the current one writes.
  stream = openSessionStream({ client: api, sessionId: id, onError: fail, onStatus: value => { if (epoch === generation) connection.textContent = connectionNotes[value] ?? ""; }, onEvent: event => {
    if (event.kind === "message.queued") followLatest(() => {
      const item = bubble("user");
      item.textContent = event.text;
      item.dataset.authorKind = event.author_kind ?? "internal";
      if (event.from_agent) item.dataset.fromAgent = event.from_agent;
      transcript.append(item);
      if (event.author_kind === "person") {
        lastBubble = item;
        lastId = event.message_id;
        setNote("sent");
      }
    });
    if (event.kind === "message.injected" && event.message_id === lastId) setNote("read");
    if (event.kind === "message.completed" && event.message_id === lastId) followLatest(() => {
      setTyping(false);
      if (event.outcome === "completed" || event.outcome === "deferred") setNote();
      else setNote(event.outcome === "failed" ? "failed" : "note", outcomeNote(event.outcome, event.reason ?? undefined));
    });
    if (event.kind === "turn.started") followLatest(() => { reply = undefined; replyText = ""; if (note?.dataset.state === "sent") setNote("read"); setTyping(true); });
    if (event.kind === "tool.started") followLatest(() => setTyping(true));
    if (event.kind === "turn.ended" || event.kind === "session.settled" || event.kind === "session.stopped") followLatest(() => { setTyping(false); settleNote(); });
    if (event.kind === "assistant.delta" || event.kind === "assistant.final") followLatest(() => {
      setTyping(false);
      settleNote();
      const target = reply ?? startReply();
      replyText = event.kind === "assistant.final" ? event.text : replyText + event.text;
      renderMarkdown(target, replyText);
    });
    if (event.kind === "card.presented") followLatest(() => { const host = cardHost(" transcript-card"); transcript.append(host); presentedCards.push(mountCard(host, event.card, { port: api, sessionId: id, supports, focus: "none" })); });
    if (event.kind.startsWith("ticket.")) void refreshTickets().catch(fail);
    // The model names a new conversation after its first reply; rename the picker entry without waiting for a tab switch.
    if (event.kind === "session.updated" || event.kind.startsWith("session.title")) void refreshSessions().catch(fail);
  } });
  await refreshTickets();
}
async function refreshSessions(): Promise<void> {
  if (tabId === undefined) return;
  const currentTab = tabId;
  const ids = new Set(await tabSessions(currentTab));
  const selected = picker.value;
  const sessions = (await client(await pairing()).listSessions()).sessions.filter(session => ids.has(session.id));
  if (currentTab !== tabId) return;
  picker.setItems([NEW_CHAT, ...sessions.map(session => ({ value: session.id, label: session.title }))], ids.has(selected) ? selected : "");
  // The picker truncates long titles with an ellipsis; the tooltip carries the full one.
  sessionsRoot.title = picker.label;
}
document.querySelector("#attach")!.addEventListener("click", () => { void attach().catch(fail); });
composer.addEventListener("input", () => { if (hasContextMention(composer.value)) void attach().catch(fail); });
for (const pair of document.querySelectorAll("#pair, #pair-start")) pair.addEventListener("click", () => { void chrome.runtime.openOptionsPage(); });
document.querySelector<HTMLFormElement>("#compose")!.addEventListener("submit", event => {
  event.preventDefault();
  if (!composer.value.trim() || tabId === undefined) return;
  const text = composer.value;
  const explicitContext = attached;
  const sourceTab = tabId;
  const chosen = picker.value;
  const submit = document.querySelector<HTMLButtonElement>('button[type="submit"]')!; submit.disabled = true;
  void (async () => {
    const context = explicitContext ?? (hasContextMention(text) ? await currentContext(sourceTab) : undefined);
    if (chosen) await client(await pairing()).submitMessage(chosen, { text: withContext(text, context), mode: "now", message_id: crypto.randomUUID() });
    else {
      const result = await chrome.runtime.sendMessage({ type: "start", tabId: sourceTab, text, context });
      if (result.error) throw new Error(result.error);
      await refreshSessions(); picker.value = result.id; await watch();
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
    picker.setItems([NEW_CHAT], "");
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
    picker.value = "";
  }
  await refreshSessions();
  const ids = tabId === undefined ? [] : await tabSessions(tabId);
  if (!picker.value && ids.length) { picker.value = ids[0]; await watch(); } else if (!picker.value) await refreshTickets();
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
