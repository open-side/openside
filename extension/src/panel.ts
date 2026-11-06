import { askerOf, isAttentionTicket } from "../../web/src/cards/attention";
import { mountCard, type MountedCard } from "../../web/src/cards/render";
import { ApiError } from "../../web/src/api/client";
import type { Action, ChatConfirmation, Event as DaemonEvent, ModelCatalog, Session } from "../../web/src/api/types";
import { CONFIRMATION_OPTIONS, EFFORT_OPTIONS, optionsPatch, type ThinkingEffort } from "../../web/src/components/sessionOptions";
import { chatModelChoices, modelDisplayName } from "../../web/src/models/modelNames";
import { formatDate, formatNumber, t as webT } from "../../web/src/i18n";
import { copyText } from "../../web/src/ui/clipboard";
import { client, openSessionStream } from "./api";
import { base64Of, type Upload } from "./capture";
import { panelAskKey, startChat, type ChatOptions, type PanelAsk } from "./chat";
import { hasContextMention, withContext, type TabContext } from "./context";
import { createDictation } from "./dictation";
import { LASSO_COMMAND } from "./lasso-settings";
import { openConversation } from "./desktop";
import { renderMarkdown } from "./markdown";
import { pairing, rememberSession, tabSessions } from "./storage";
import { createPicker, type Picker, type PickerItem } from "./picker";
import { bindStatus, icon, mountIcons, type IconName } from "./ui";
import { installScrollReveal } from "../../web/src/ui/scrollReveal";
import { outcomeNote } from "../../web/src/components/toolStatus";
import { initLocale, localizeDocument, t } from "./i18n";
import { ADOPT_LAUNCHER_PAIRING, forgetRevokedLauncherPairing } from "./launcher";
installScrollReveal();
// The daemon's limits (openside-proto MAX_ATTACHMENTS / MAX_ATTACHMENT_BYTES), as the web composer checks them.
const MAX_ATTACHMENTS = 4;
const MAX_ATTACHMENT_BYTES = 2 * 1024 * 1024;
const sessionsRoot = document.querySelector<HTMLElement>("#sessions")!;
const confirmationRoot = document.querySelector<HTMLElement>("#confirmation")!;
const modelRoot = document.querySelector<HTMLElement>("#model")!;
const effortRoot = document.querySelector<HTMLElement>("#effort")!;
const newChat = () => ({ value: "", label: t("ext.panel.newChat") });
let picker!: Picker;
let confirmationPicker!: Picker;
let modelPicker!: Picker;
let effortPicker!: Picker;
// Starts disabled in the markup: nothing may be sent before the picker exists, nor twice while a send is in flight.
const send = document.querySelector<HTMLButtonElement>('#compose button[type="submit"]')!;
const stop = document.querySelector<HTMLButtonElement>("#stop")!;
const popOut = document.querySelector<HTMLButtonElement>("#pop-out")!;
// The picker and every string wait for the stored language; a later change reloads the page in it.
const ready = initLocale(() => { location.reload(); }).then(() => {
  localizeDocument();
  picker = createPicker(sessionsRoot, t("ext.panel.picker"), () => { void watch().catch(fail); });
  picker.setItems([newChat()], "");
  confirmationPicker = createPicker(confirmationRoot, webT("thread.confirmation.title"), value => { void changeOption({ confirmation: value as ChatConfirmation }); });
  modelPicker = createPicker(modelRoot, webT("thread.model.label"), value => { if (value) void changeOption({ model: value }); });
  effortPicker = createPicker(effortRoot, webT("thread.options.effort"), value => { if (value) void changeOption({ effort: value as ThinkingEffort }); });
  modelRoot.querySelector(".picker-button")!.prepend(brandMark());
  renderOptions();
  send.disabled = false;
  syncSend();
});
const composer = document.querySelector<HTMLTextAreaElement>("#message")!;
const status = document.querySelector<HTMLElement>("#status")!;
const transcript = document.querySelector<HTMLElement>("#transcript")!;
const connection = document.querySelector<HTMLElement>("#connection")!;
const ticketBox = document.querySelector<HTMLElement>("#tickets")!;
const contextChip = document.querySelector<HTMLElement>("#context")!;
const fileList = document.querySelector<HTMLElement>("#files")!;
const fileInput = document.querySelector<HTMLInputElement>("#file-input")!;
const plus = document.querySelector<HTMLButtonElement>("#plus")!;
const plusMenu = document.querySelector<HTMLElement>("#plus-menu")!;
const ring = document.querySelector<HTMLElement>("#ring")!;
const mic = document.querySelector<HTMLButtonElement>("#mic")!;
let tabId: number | undefined;
let attached: TabContext | undefined;
let files: Upload[] = [];
let stream: ReturnType<typeof openSessionStream> | undefined;
let generation = 0;
// The chats listed for this tab, as the daemon last reported them; the toolbar shows the open one's options.
const sessions = new Map<string, Session>();
// A new chat's options until its first message creates it; null leaves the field to the daemon.
let draft: { model: string | null; effort: ThinkingEffort | null; confirmation: ChatConfirmation | null } = { model: null, effort: null, confirmation: null };
let catalog: ModelCatalog | null = null;
let defaultConfirmation: ChatConfirmation = "ask_first";
let running = false;
const notify = bindStatus(status);
const scroller = document.querySelector<HTMLElement>(".scroll")!;
const fail = () => { notify(t("ext.panel.unreachable"), "error"); };
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
// The web app's mark (web/src/ui/BrandMark.tsx) on the model chip, where Aside shows the provider's.
function brandMark(): SVGSVGElement {
  const NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(NS, "svg");
  for (const [key, value] of Object.entries({ viewBox: "-96 -96 192 192", width: "14", height: "14", fill: "currentColor", "aria-hidden": "true", class: "chip-mark" })) svg.setAttribute(key, value);
  const path = document.createElementNS(NS, "path");
  path.setAttribute("d", "M0 96A96 96 0 1 1 90.18 -32.92L61.28 -18.47A64 64 0 1 0 0 64ZM46 16L94 -8L94 56L46 80Z");
  svg.append(path);
  return svg;
}
type DeliveryState = "sent" | "read" | "failed" | "note";
// Same classes and wording as the web timeline (components/Timeline.tsx); web/src/styles.css styles both.
function deliveryNote(state: DeliveryState, text?: string): HTMLElement {
  const element = document.createElement("p");
  element.className = state === "failed" ? "receipt receipt-failed" : "receipt";
  element.dataset.state = state;
  if (state === "sent" || state === "read") { element.textContent = t(state === "sent" ? "ext.panel.receipt.sent" : "ext.panel.receipt.read"); return element; }
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
  label.textContent = t("ext.panel.typing");
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
function userBubble(event: Extract<DaemonEvent, { kind: "message.queued" }>): HTMLElement {
  const item = bubble("user");
  item.textContent = event.text;
  const names = (event.attachments ?? []).map(attachment => attachment.name || attachment.path.split("/").pop() || attachment.mime);
  if (names.length) {
    const list = document.createElement("ul");
    list.className = "bubble-files";
    for (const name of names) {
      const chip = document.createElement("li");
      chip.className = "file-chip";
      const label = document.createElement("span");
      label.textContent = name;
      chip.append(icon("paperclip", 14), label);
      list.append(chip);
    }
    item.append(list);
  }
  return item;
}
// The page itself refused the script and the tab gave no address either, so nothing can be attached.
class PageUnreadable extends Error {}
// A click in the side panel grants no activeTab, so reading the selection fails on a site without
// access; the tab's address and title still come from the tabs permission, and attach without it.
async function currentContext(sourceTab = tabId): Promise<TabContext> {
  if (sourceTab === undefined) throw new Error("No current tab");
  const tab = await chrome.tabs.get(sourceTab);
  const selection = await chrome.scripting.executeScript({ target: { tabId: sourceTab }, func: () => window.getSelection()?.toString() ?? "" })
    .then(results => results[0]?.result ?? "", () => null);
  if (selection === null && !tab.url) throw new PageUnreadable();
  return { url: tab.url ?? "", title: tab.title ?? "", selection: selection ?? "" };
}
const failContext = (cause: unknown) => { if (cause instanceof PageUnreadable) notify(t("ext.panel.attachNeedsAccess"), "error"); else fail(); };
function setAttached(context: TabContext | undefined): void {
  attached = context;
  contextChip.textContent = context ? t("ext.panel.attached", { title: context.title, url: context.url }) : "";
  contextChip.title = contextChip.textContent;
}
async function attach(): Promise<void> {
  const sourceTab = tabId;
  const context = await currentContext(sourceTab);
  if (sourceTab !== tabId) return;
  setAttached(context);
}
const uploadBytes = (upload: Upload) => Math.floor(upload.data_base64.length * 3 / 4);
// Refuses a pick that would break the daemon's limits before anything is read, like the web composer.
function admits(count: number, bytes: number): boolean {
  if (files.length + count > MAX_ATTACHMENTS) { notify(webT("thread.composer.error.tooManyAttachments", { max: MAX_ATTACHMENTS }), "error"); return false; }
  if (files.reduce((sum, upload) => sum + uploadBytes(upload), 0) + bytes > MAX_ATTACHMENT_BYTES) { notify(webT("thread.composer.error.attachmentsTooLarge"), "error"); return false; }
  return true;
}
function renderFiles(): void {
  fileList.replaceChildren(...files.map((upload, index) => {
    const chip = document.createElement("li");
    chip.className = "file-chip";
    const label = document.createElement("span");
    label.textContent = upload.name;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "file-chip-remove";
    remove.setAttribute("aria-label", webT("thread.composer.removeAttachment", { name: upload.name }));
    remove.append(icon("x", 12));
    remove.addEventListener("click", () => { files = files.filter((_, at) => at !== index); renderFiles(); });
    chip.append(icon("paperclip", 14), label, remove);
    return chip;
  }));
  syncSend();
}
async function addFiles(list: FileList | null): Promise<void> {
  const picked = Array.from(list ?? []);
  if (!picked.length || !admits(picked.length, picked.reduce((sum, file) => sum + file.size, 0))) return;
  const read = await Promise.all(picked.map(async file => ({ name: file.name, data_base64: base64Of(new Uint8Array(await file.arrayBuffer())) })));
  files = [...files, ...read];
  renderFiles();
}
// Like the web composer: the mic is the circle while the box is empty and nothing runs; once
// something is typed it becomes a grey glyph beside Send, and beside Stop while the chat works.
function syncSend(): void {
  const typed = composer.value.trim() !== "" || files.length > 0;
  const stopping = running && !typed;
  send.hidden = !typed;
  stop.hidden = !stopping;
  micCircle = !typed && !stopping;
  renderMic();
}
let micCircle = true;
function renderMic(): void {
  const { state, reason } = dictation;
  const transcribing = state === "transcribing";
  const label = state === "recording" ? webT("thread.mic.stop") : transcribing ? webT("thread.mic.cancel") : webT("thread.mic.start");
  mic.className = micCircle ? "composer-send composer-mic-circle" : "composer-mic";
  mic.classList.toggle("is-recording", state === "recording");
  mic.setAttribute("aria-label", label);
  mic.title = reason ?? label;
  mic.setAttribute("aria-pressed", String(state === "recording"));
  // Unavailable dictation keeps its tooltip, so the button is aria-disabled rather than disabled.
  if (state === "unavailable" || state === "checking") mic.setAttribute("aria-disabled", "true"); else mic.removeAttribute("aria-disabled");
  const description = reason ?? (transcribing ? webT("thread.mic.transcribing") : "");
  if (description) mic.setAttribute("aria-description", description); else mic.removeAttribute("aria-description");
  const glyph = icon(transcribing ? "loader-circle" : "mic", 14);
  if (transcribing) glyph.classList.add("composer-spin");
  mic.replaceChildren(glyph);
}
// Spoken text lands at the caret with a space on either side, as in the web composer.
function insertSpoken(spoken: string): void {
  const caret = document.activeElement === composer ? composer.selectionStart : composer.value.length;
  const before = composer.value.slice(0, caret);
  const after = composer.value.slice(caret);
  composer.value = `${before}${before && !/\s$/.test(before) ? " " : ""}${spoken}${after && !/^\s/.test(after) ? " " : ""}${after}`;
  const end = composer.value.length - after.length;
  composer.focus();
  composer.setSelectionRange(end, end);
  syncSend();
}
const dictation = createDictation({
  api: async () => client(await pairing()),
  onText: insertSpoken,
  onError: message => notify(message, "error"),
  onNeedsAccess: () => {
    void chrome.tabs.create({ url: chrome.runtime.getURL("mic-access.html") });
    notify(t("ext.panel.mic.allowInTab"));
  },
  onChange: renderMic,
});
const RING = 14;
const RING_STROKE = 1.5;
const RING_RADIUS = (RING - RING_STROKE) / 2;
const RING_LENGTH = 2 * Math.PI * RING_RADIUS;
// The web composer's context ring (ComposerOptions ContextRing): the open chat's share of its window.
function renderRing(): void {
  const session = sessions.get(picker.value);
  ring.hidden = !session;
  if (!session) return;
  const limit = session.context?.window_tokens ?? null;
  const used = session.context?.used_tokens ?? 0;
  const share = limit ? Math.min(1, used / limit) : 0;
  const text = limit
    ? webT("thread.context.usage", { percent: Math.round(share * 100), used: formatNumber(used, { notation: "compact" }), window: formatNumber(limit, { notation: "compact" }) })
    : webT("thread.context.usageUnknown");
  ring.setAttribute("aria-label", text);
  ring.title = text;
  ring.classList.toggle("is-full", share >= 0.9);
  const NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(NS, "svg");
  for (const [key, value] of Object.entries({ width: `${RING}`, height: `${RING}`, viewBox: `0 0 ${RING} ${RING}`, "aria-hidden": "true" })) svg.setAttribute(key, value);
  const circle = (className: string, extra: Record<string, string>) => {
    const element = document.createElementNS(NS, "circle");
    for (const [key, value] of Object.entries({ class: className, cx: `${RING / 2}`, cy: `${RING / 2}`, r: `${RING_RADIUS}`, "stroke-width": `${RING_STROKE}`, fill: "none", ...extra })) element.setAttribute(key, value);
    return element;
  };
  svg.append(
    circle("composer-ring-track", {}),
    circle("composer-ring-fill", { "stroke-dasharray": `${RING_LENGTH}`, "stroke-dashoffset": `${RING_LENGTH * (1 - share)}`, "stroke-linecap": "round", transform: `rotate(-90 ${RING / 2} ${RING / 2})` }),
  );
  ring.replaceChildren(svg);
}
// A finished turn is what moves the ring; the list of chats does not carry it fresh enough. A
// replayed history ends many turns at once, so the read waits for the stream to go quiet.
let contextTimer: number | undefined;
function scheduleContext(id: string): void {
  window.clearTimeout(contextTimer);
  contextTimer = window.setTimeout(() => { void refreshContext(id).catch(() => undefined); }, 300);
}
async function refreshContext(id: string): Promise<void> {
  const session = await client(await pairing()).session(id);
  if (!sessions.has(id)) return;
  sessions.set(id, session);
  if (picker.value === id) renderRing();
}
function footerButton(name: IconName, label: string, onClick: (button: HTMLButtonElement) => void): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "footer-button";
  button.setAttribute("aria-label", label);
  button.title = label;
  button.append(icon(name, 12));
  button.addEventListener("click", () => onClick(button));
  return button;
}
// The row under a reply, as in the web timeline (ReplyFooter): Copy, Branch and the reply's time.
function replyFooter(text: string, at: string, from: string, forkable: boolean): HTMLElement {
  const footer = document.createElement("div");
  footer.className = "message-footer";
  footer.dataset.from = from;
  if (text) {
    footer.append(footerButton("copy", webT("thread.message.copy"), () => {
      void copyText(text).then(copied => notify(copied ? webT("common.copied") : webT("thread.message.copyFailed"), copied ? "info" : "error"));
    }));
  }
  // Branching copies the conversation up to this reply, so it needs the person's finished message.
  const branchButton = footerButton("git-branch", webT("thread.message.branch"), button => {
    button.disabled = true;
    void branch(from).finally(() => { button.disabled = false; });
  });
  branchButton.classList.add("footer-branch");
  branchButton.hidden = !from || !forkable;
  footer.append(branchButton);
  if (!Number.isNaN(Date.parse(at))) {
    const time = document.createElement("time");
    time.dateTime = at;
    time.textContent = formatDate(new Date(at), { hour: "numeric", minute: "2-digit" });
    footer.append(time);
  }
  return footer;
}
// A new chat holding the conversation up to this reply; it joins this tab's chats and opens here.
async function branch(from: string): Promise<void> {
  const id = picker.value;
  const sourceTab = tabId;
  if (!id || !from || sourceTab === undefined) return;
  try {
    const created = await client(await pairing()).forkSession(id, { from_message: from });
    await rememberSession(sourceTab, created.id);
    if (sourceTab !== tabId) return;
    await refreshSessions();
    picker.value = created.id;
    await watch();
  } catch (cause) {
    notify(cause instanceof ApiError ? (cause.code === "message_not_finished" ? webT("timeline.fork.notFinished") : cause.message) : webT("timeline.fork.failed"), "error");
  }
}
function sessionOptionsShown(): { confirmation: ChatConfirmation; model: string; effort: string } {
  const session = sessions.get(picker.value);
  const model = session?.model ?? draft.model ?? catalog?.default_model ?? "";
  const picked = session ? session.effort ?? null : draft.effort;
  // An unpicked effort shows the level the model then runs at, like the web chip.
  const modelDefault = catalog?.providers.flatMap(provider => provider.models).find(entry => entry.id === model)?.default_effort ?? null;
  const unpicked = modelDefault && EFFORT_OPTIONS.some(option => option.value === modelDefault) ? modelDefault : "";
  return { confirmation: session?.confirmation ?? draft.confirmation ?? defaultConfirmation, model, effort: picked ?? unpicked };
}
function chipIcon(root: HTMLElement, name: IconName): void {
  const glyph = icon(name, 16);
  glyph.classList.add("chip-icon");
  const button = root.querySelector(".picker-button")!;
  const previous = button.querySelector(".chip-icon");
  if (previous) previous.replaceWith(glyph); else button.prepend(glyph);
}
function renderOptions(): void {
  if (!effortPicker) return;
  const shown = sessionOptionsShown();
  confirmationPicker.setItems(CONFIRMATION_OPTIONS.map(option => ({ value: option.value, label: webT(option.labelKey) })), shown.confirmation);
  chipIcon(confirmationRoot, shown.confirmation === "skip" ? "step-forward" : "hand");
  confirmationRoot.title = confirmationPicker.label;
  const models: PickerItem[] = chatModelChoices(catalog).map(choice => ({ value: choice.id, label: choice.name }));
  if (!shown.model) models.unshift({ value: "", label: webT("thread.options.default") });
  else if (!models.some(item => item.value === shown.model)) models.unshift({ value: shown.model, label: modelDisplayName(catalog, shown.model) });
  modelPicker.setItems(models, shown.model);
  modelRoot.title = modelPicker.label;
  const efforts: PickerItem[] = EFFORT_OPTIONS.map(option => ({ value: option.value, label: webT(option.labelKey) }));
  effortPicker.setItems(shown.effort ? efforts : [{ value: "", label: webT("thread.options.default") }, ...efforts], shown.effort);
  renderRing();
}
async function changeOption(change: Partial<ChatOptions>): Promise<void> {
  const id = picker.value;
  if (!id) { draft = { ...draft, ...change }; renderOptions(); return; }
  try {
    sessions.set(id, await client(await pairing()).updateSession(id, optionsPatch(change)));
  } catch {
    notify(webT("thread.composer.error.optionsFailed"), "error");
  }
  renderOptions();
}
// What the toolbar needs from the daemon besides the chat: model names and the confirmation default. Both are optional.
async function loadComposerDefaults(): Promise<void> {
  const api = client(await pairing());
  const [models, preferences] = await Promise.allSettled([api.modelCatalog(), api.preferences()]);
  if (models.status === "fulfilled") catalog = models.value;
  if (preferences.status === "fulfilled") defaultConfirmation = preferences.value.confirmation;
  renderOptions();
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
function setRunning(next: boolean): void {
  running = next;
  syncSend();
}
async function watch(): Promise<void> {
  const epoch = ++generation;
  stream?.close(); transcript.replaceChildren(); connection.textContent = "";
  for (const [id, entry] of ticketCards) if (!entry.anywhere) { entry.mounted.dispose(); entry.host.remove(); ticketCards.delete(id); }
  for (const mounted of presentedCards.splice(0)) mounted.dispose();
  const id = picker.value;
  sessionsRoot.title = picker.label;
  popOut.disabled = !id;
  setRunning(sessions.get(id)?.status === "running");
  renderOptions();
  if (!id) { await refreshTickets(); return; }
  const api = client(await pairing());
  if (epoch !== generation) return;
  let reply: HTMLElement | undefined;
  let replyText = "";
  // Only the newest message carries a delivery note, like the web timeline.
  let lastBubble: HTMLElement | undefined;
  let lastId = "";
  // The person's message the current turn answers; Branch needs it, and only once it has finished.
  let turnFrom = "";
  const finished = new Set<string>();
  let latestReply: HTMLElement | undefined;
  let note: HTMLElement | undefined;
  let typing: HTMLElement | undefined;
  const startReply = () => { reply = bubble("assistant"); replyText = ""; transcript.append(reply); return reply; };
  const setNote = (state?: DeliveryState, text?: string) => { note?.remove(); note = state && lastBubble ? deliveryNote(state, text) : undefined; if (note) lastBubble!.after(note); };
  const setTyping = (on: boolean) => { if (on && !typing) { typing = typingRow(); transcript.append(typing); } else if (!on) { typing?.remove(); typing = undefined; } };
  // Sent and Read go away once the agent answers; a failure or stop note stays until the next message.
  const settleNote = () => { if (note?.dataset.state === "sent" || note?.dataset.state === "read") setNote(); };
  const connectionNotes: Partial<Record<string, string>> = { connecting: t("ext.panel.connecting"), reconnecting: t("ext.panel.reconnecting") };
  // A replaced stream still reports "closed" after watch() moved on, so only the current one writes.
  stream = openSessionStream({ client: api, sessionId: id, onError: fail, onStatus: value => { if (epoch === generation) connection.textContent = connectionNotes[value] ?? ""; }, onEvent: event => {
    if (event.kind === "message.queued") followLatest(() => {
      const item = userBubble(event);
      item.dataset.authorKind = event.author_kind ?? "internal";
      if (event.from_agent) item.dataset.fromAgent = event.from_agent;
      transcript.append(item);
      turnFrom = event.author_kind === "person" ? event.message_id : "";
      if (event.author_kind === "person") {
        lastBubble = item;
        lastId = event.message_id;
        setNote("sent");
      }
    });
    if (event.kind === "message.injected" && event.message_id === lastId) setNote("read");
    if (event.kind === "message.completed") {
      finished.add(event.message_id);
      for (const footer of transcript.querySelectorAll<HTMLElement>(".message-footer")) if (footer.dataset.from === event.message_id) footer.querySelector<HTMLElement>(".footer-branch")!.hidden = false;
    }
    if (event.kind === "message.completed" && event.message_id === lastId) followLatest(() => {
      setTyping(false);
      if (event.outcome === "completed" || event.outcome === "deferred") setNote();
      else setNote(event.outcome === "failed" ? "failed" : "note", outcomeNote(event.outcome, event.reason ?? undefined));
    });
    if (event.kind === "turn.started") followLatest(() => { reply = undefined; replyText = ""; if (note?.dataset.state === "sent") setNote("read"); setTyping(true); setRunning(true); });
    if (event.kind === "tool.started") followLatest(() => setTyping(true));
    if (event.kind === "turn.ended" || event.kind === "session.settled" || event.kind === "session.stopped") followLatest(() => { setTyping(false); settleNote(); setRunning(false); scheduleContext(id); });
    if (event.kind === "assistant.delta" || event.kind === "assistant.final") followLatest(() => {
      setTyping(false);
      settleNote();
      const target = reply ?? startReply();
      replyText = event.kind === "assistant.final" ? event.text : replyText + event.text;
      renderMarkdown(target, replyText);
      if (event.kind === "assistant.final") {
        // Only the newest reply keeps its footer in view; older ones show it on hover, like the web timeline.
        latestReply?.removeAttribute("data-latest");
        latestReply = target;
        target.dataset.latest = "";
        target.append(replyFooter(replyText, event.ts, turnFrom, finished.has(turnFrom)));
      }
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
  const listed = (await client(await pairing()).listSessions()).sessions.filter(session => ids.has(session.id));
  if (currentTab !== tabId) return;
  sessions.clear();
  for (const session of listed) sessions.set(session.id, session);
  picker.setItems([newChat(), ...listed.map(session => ({ value: session.id, label: session.title }))], ids.has(selected) ? selected : "");
  // The picker truncates long titles with an ellipsis; the tooltip carries the full one.
  sessionsRoot.title = picker.label;
  popOut.disabled = !picker.value;
  renderOptions();
}
// Into the chat the picker shows, or a new chat for this tab with the toolbar's options.
async function deliver(sourceTab: number, text: string, context: TabContext | undefined, uploads: Upload[]): Promise<void> {
  const chosen = picker.value;
  if (chosen) {
    await client(await pairing()).submitMessage(chosen, { text: withContext(text, context), mode: "now", message_id: crypto.randomUUID(), ...(uploads.length ? { attachments: uploads } : {}) });
    return;
  }
  const options: ChatOptions = {
    ...(draft.model !== null ? { model: draft.model } : {}),
    ...(draft.effort !== null ? { effort: draft.effort } : {}),
    ...(draft.confirmation !== null ? { confirmation: draft.confirmation } : {}),
  };
  const id = await startChat(sourceTab, text, context, uploads, options);
  if (sourceTab !== tabId) return;
  await refreshSessions(); picker.value = id; await watch();
}
const isPanelAsk = (value: unknown): value is PanelAsk =>
  typeof value === "object" && value !== null && "text" in value && typeof value.text === "string" && "context" in value && typeof value.context === "object" && value.context !== null;
// A lasso, a selection shortcut or "Ask about selection" hands the panel one ask per tab. Each is
// taken once (read, then removed) and asks run one after another, so two wake-ups never send twice.
async function takeAsk(): Promise<void> {
  const sourceTab = tabId;
  if (sourceTab === undefined || document.body.dataset.paired !== "true") return;
  const key = panelAskKey(sourceTab);
  const ask: unknown = (await chrome.storage.session.get(key))[key];
  // An ask for a tab the person has left waits until that tab is current again.
  if (!isPanelAsk(ask) || sourceTab !== tabId) return;
  await chrome.storage.session.remove(key);
  const uploads = ask.attachments ?? [];
  if (ask.text === "") {
    setAttached(ask.context);
    if (uploads.length && admits(uploads.length, uploads.reduce((sum, upload) => sum + uploadBytes(upload), 0))) { files = [...files, ...uploads]; renderFiles(); }
    composer.focus();
    return;
  }
  await deliver(sourceTab, ask.text, ask.context, uploads);
}
let asks = Promise.resolve();
const queueAsk = () => { asks = asks.then(takeAsk).catch(fail); };
function closeMenu(): void {
  plusMenu.hidden = true;
  plus.setAttribute("aria-expanded", "false");
}
function menuItems(): HTMLButtonElement[] { return [...plusMenu.querySelectorAll<HTMLButtonElement>("[role=menuitem]")]; }
plus.addEventListener("click", () => {
  if (!plusMenu.hidden) { closeMenu(); return; }
  plusMenu.hidden = false;
  plus.setAttribute("aria-expanded", "true");
  menuItems()[0]?.focus();
});
plusMenu.addEventListener("keydown", event => {
  const items = menuItems();
  const at = items.indexOf(document.activeElement as HTMLButtonElement);
  if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); items[(at + (event.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus(); }
  if (event.key === "Escape") { event.preventDefault(); closeMenu(); plus.focus(); }
});
plusMenu.parentElement!.addEventListener("focusout", event => { if (!plusMenu.parentElement!.contains(event.relatedTarget as Node | null)) closeMenu(); });
plusMenu.addEventListener("click", event => { if ((event.target as Element).closest("[role=menuitem]")) closeMenu(); });
document.querySelector("#attach")!.addEventListener("click", () => { void attach().catch(failContext); });
document.querySelector("#upload")!.addEventListener("click", () => { fileInput.click(); });
fileInput.addEventListener("change", () => { void addFiles(fileInput.files).catch(() => notify(webT("thread.composer.error.fileUnreadable"), "error")).finally(() => { fileInput.value = ""; }); });
// The page script draws the region; the capture comes back as a panel ask for this tab. A click in
// the side panel grants no activeTab, so on a site without access the person needs a gesture on the
// page itself: the start-lasso shortcut or the right-click Capture item.
async function startCapture(target: number): Promise<void> {
  const reply: unknown = await chrome.runtime.sendMessage({ type: "lasso.start", tabId: target });
  if (typeof reply !== "object" || reply === null || !("error" in reply)) return;
  if (reply.error !== "no_access") { fail(); return; }
  const keys = (await chrome.commands.getAll()).find(entry => entry.name === LASSO_COMMAND)?.shortcut ?? "";
  notify(keys ? t("ext.panel.captureNeedsGesture", { keys }) : t("ext.panel.captureNeedsGestureNoKey"));
}
document.querySelector("#capture")!.addEventListener("click", () => { if (tabId !== undefined) void startCapture(tabId).catch(fail); });
composer.addEventListener("input", () => { syncSend(); if (hasContextMention(composer.value)) void attach().catch(failContext); });
composer.addEventListener("keydown", event => {
  if (event.key === "Enter" && !event.shiftKey && !event.isComposing) { event.preventDefault(); document.querySelector<HTMLFormElement>("#compose")!.requestSubmit(); }
});
document.querySelector("#pair-start")!.addEventListener("click", () => { void chrome.runtime.openOptionsPage(); });
document.querySelector("#new-chat")!.addEventListener("click", () => {
  if (!picker) return;
  picker.value = "";
  void watch().catch(fail);
  composer.focus();
});
popOut.addEventListener("click", () => { if (picker.value) void openConversation(picker.value).catch(fail); });
// sidePanel.close is Chrome 141+ (the manifest's minimum); the bundled @types/chrome predates it.
const sidePanel = chrome.sidePanel as unknown as { close: (options: { windowId: number }) => Promise<void> };
document.querySelector("#close")!.addEventListener("click", () => {
  void chrome.windows.getCurrent().then(current => sidePanel.close({ windowId: current.id! })).catch(fail);
});
mic.addEventListener("click", () => { dictation.toggle(); });
stop.addEventListener("click", () => {
  const id = picker.value;
  if (id) void pairing().then(pair => client(pair).stopSession(id)).catch(fail);
});
document.querySelector<HTMLFormElement>("#compose")!.addEventListener("submit", event => {
  event.preventDefault();
  if (send.disabled || (!composer.value.trim() && !files.length) || tabId === undefined) return;
  const text = composer.value;
  const explicitContext = attached;
  const uploads = files;
  const sourceTab = tabId;
  send.disabled = true;
  void (async () => {
    const context = explicitContext ?? (hasContextMention(text) ? await currentContext(sourceTab) : undefined);
    await deliver(sourceTab, text, context, uploads);
    composer.value = ""; setAttached(undefined); files = []; renderFiles();
  })().catch(failContext).finally(() => { send.disabled = false; });
});
async function initialize(): Promise<void> {
  // Without a pairing every request would fail; show how to connect instead of an error.
  const paired = (await chrome.storage.session.get("pairing")).pairing !== undefined;
  if (!paired) {
    // The desktop launcher hands this browser a pairing; the stored pairing re-runs this when it lands.
    const adopted = await chrome.runtime.sendMessage({ type: ADOPT_LAUNCHER_PAIRING }).catch(() => false) === true;
    if (adopted) return;
  }
  const wasPaired = document.body.dataset.paired === "true";
  document.body.dataset.paired = String(paired);
  if (!paired) {
    generation += 1;
    stream?.close(); transcript.replaceChildren(); ticketBox.replaceChildren(); connection.textContent = "";
    sessions.clear(); catalog = null;
    picker.setItems([newChat()], "");
    popOut.disabled = true;
    return;
  }
  if (!wasPaired) { void loadComposerDefaults().catch(() => undefined); dictation.refresh(); }
  const active = (await chrome.tabs.query({ active: true, currentWindow: true }))[0];
  const changed = tabId !== active?.id;
  tabId = active?.id;
  if (changed) {
    generation += 1;
    stream?.close(); transcript.replaceChildren();
    for (const card of ticketBox.querySelectorAll(":scope > .ticket:not(.ticket-anywhere)")) card.remove();
    setAttached(undefined); files = []; renderFiles();
    picker.value = "";
  }
  try {
    await refreshSessions();
  } catch (error) {
    // Revoked in Devices: forgetting it shows how to pair again (the next launch pairs itself).
    if (await forgetRevokedLauncherPairing(error)) return;
    throw error;
  }
  const ids = tabId === undefined ? [] : await tabSessions(tabId);
  if (!picker.value && ids.length) { picker.value = ids[0]; await watch(); } else if (!picker.value) await refreshTickets();
  queueAsk();
}
chrome.storage.onChanged.addListener((changes, area) => {
  if (area !== "session") return;
  if (changes.pairing || (tabId !== undefined && changes[`sessions:${tabId}`])) void ready.then(initialize).catch(fail);
  else if (tabId !== undefined && changes[panelAskKey(tabId)]?.newValue !== undefined) void ready.then(queueAsk);
});
chrome.tabs.onActivated.addListener(() => { void ready.then(initialize).catch(fail); });
// Code requests from conversations this panel is not streaming arrive by polling. A failed poll
// stays quiet; the stream and the actions report connection problems.
const poll = window.setInterval(() => { if (document.visibilityState === "visible") void ready.then(refreshTickets).catch(() => undefined); }, 5000);
window.addEventListener("pagehide", () => { stream?.close(); window.clearInterval(poll); dictation.dispose(); });
void ready.then(initialize).catch(fail);
