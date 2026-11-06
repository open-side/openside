import type { AskDetails } from "./asks";
import { initLocale, localizeDocument, t } from "./i18n";
import { icon, mountIcons, type IconName } from "./ui";

/*
 * The popover a lasso or a selection dot opens on the page (an iframe of this extension page). The
 * person picks a shortcut or types a question; the side panel opens inside that click, since
 * Chrome only opens it in response to the person's own action.
 */
const token = location.hash.slice(1);
const list = document.querySelector<HTMLElement>("#shortcuts")!;
const form = document.querySelector<HTMLFormElement>("#ask")!;
const input = document.querySelector<HTMLInputElement>("#question")!;
const send = document.querySelector<HTMLButtonElement>("#send")!;
const status = document.querySelector<HTMLElement>("#status")!;
const ICON_OF: Partial<Record<string, IconName>> = { summarize: "text-align-start", translate: "languages", "copy-code": "code", "search-image": "image" };
let details: AskDetails | undefined;
let busy = false;

function setBusy(value: boolean): void {
  busy = value;
  for (const control of document.querySelectorAll<HTMLButtonElement | HTMLInputElement>("button, input")) control.disabled = value;
  if (!value) send.disabled = !input.value.trim();
}

function choose(choice: { shortcut: string } | { prompt: string }): void {
  if (!details || busy) return;
  setBusy(true);
  // Must start before any await: the click is what lets the extension open its side panel.
  const opened = chrome.sidePanel.open({ tabId: details.tabId }).then(() => true, () => false);
  void opened.then(panel => chrome.runtime.sendMessage({ type: "lasso.ask", token, ...choice, panel })).then((reply: { error?: string } | undefined) => {
    if (reply?.error) throw new Error(reply.error);
  }).catch(() => {
    setBusy(false);
    // The popover has no room for a message line; the empty question box shows it, and the status line reads it out.
    const failed = t("ext.lasso.popover.failed");
    input.placeholder = failed;
    status.textContent = failed;
  });
}

function cancel(): void { void chrome.runtime.sendMessage({ type: "lasso.cancel", token }).catch(() => undefined); }

void initLocale(() => { location.reload(); }).then(async () => {
  localizeDocument();
  mountIcons();
  const reply = await chrome.runtime.sendMessage({ type: "lasso.details", token }) as Partial<AskDetails> | undefined;
  if (!reply?.shortcuts || reply.tabId === undefined || !reply.kind) { cancel(); return; }
  details = reply as AskDetails;
  input.placeholder = t(details.kind === "text" ? "ext.lasso.popover.placeholderText" : "ext.lasso.popover.placeholderRegion");
  list.replaceChildren(...details.shortcuts.map(shortcut => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "ask-row";
    const label = document.createElement("span");
    label.textContent = shortcut.label;
    row.append(icon(ICON_OF[shortcut.id] ?? "zap", 15), label);
    row.addEventListener("click", () => choose({ shortcut: shortcut.id }));
    return row;
  }));
  input.focus();
}).catch(cancel);

input.addEventListener("input", () => { send.disabled = !input.value.trim(); });
form.addEventListener("submit", event => {
  event.preventDefault();
  const prompt = input.value.trim();
  if (prompt) choose({ prompt });
});
document.addEventListener("keydown", event => {
  if (event.key === "Escape") { event.preventDefault(); cancel(); return; }
  if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
  const controls = [...document.querySelectorAll<HTMLElement>(".ask-row, #question")];
  const at = controls.indexOf(document.activeElement as HTMLElement);
  const next = controls[(at + (event.key === "ArrowDown" ? 1 : controls.length - 1)) % controls.length];
  if (next) { event.preventDefault(); next.focus(); }
});
