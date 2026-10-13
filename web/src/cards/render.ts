// The one card renderer. A card is data (see `Card` in api/types); this turns it into DOM for the
// web app and the extension side panel alike. It is a state machine over three shapes:
//   ask     a compact bubble that expands to a glass card while a person is being asked;
//           after the answer it leaves a receipt bubble that never shows entered values.
//   row     activity and hand-off cards: a bubble with a status mark, expandable for detail.
//   result  read-only results (lists, tables, orders): a framed card, open from the start.
// Typed values live only in the live inputs; every collapse, Escape, terminal update and dispose
// empties them.
import type { Action, Block, Card, CardKind } from "../api/types";
import { dispatchAction, fieldsOf, SaveRefused, type ActionOutcome, type CardPort, type HostActionHandler, type Transient } from "./actions";
import { renderBlock, renderNotice, renderTotal, type BlockContext } from "./blocks";
import { AUTHENTICATOR_SUMMARY, SECURE_NOTE, STATUS_LABEL, UNHANDLED_ACTION, failureMessage } from "./copy";
import { element, setPending, uid } from "./dom";
import { FieldRegistry, renderField } from "./fields";
import { brandMark, glyph, type GlyphName } from "./glyphs";
import { siteMark } from "./marks";
import { Media } from "./media";
import { isSecretField, linkHost, safeHref, verifiedOrigin } from "./values";

export type Settled = "resolved" | "rejected" | "done";

export interface CardMountOptions {
  port: CardPort;
  sessionId?: string;
  /** `false` when this device lacks the approve scope: saving to the vault is then not offered. */
  canApprove?: boolean;
  /** Host-owned targets (follow_up, open_*, team_command, hire edits). Return `false` to decline. */
  onAction?: HostActionHandler;
  /** Hide actions the host cannot perform, e.g. in-app navigation inside the extension. */
  supports?: (action: Action) => boolean;
  onSettled?: (outcome: Settled, card: Card) => void;
  /** Whether opening on mount may take focus: `polite` yields to an input the person is typing in. */
  focus?: "polite" | "force" | "none";
  /** `sheet`: the host already provides the frame and the close control, so the card is always open. */
  layout?: "card" | "sheet";
  defaultOpen?: boolean;
  inert?: boolean;
  /** The child agent (helper task or teammate) that raised this request, when it was not the main agent. */
  asker?: { id: string; label: string } | null;
  openUrl?: (url: string) => void;
}

export interface MountedCard {
  readonly element: HTMLElement;
  update(card: Card): void;
  open(): void;
  collapse(): void;
  setInert(inert: boolean): void;
  dispose(): void;
}

type Variant = "ask" | "row" | "result";

const KIND_GLYPH: Partial<Record<CardKind, GlyphName>> = {
  secure_login: "lock",
  otp: "lock",
  secret_input: "lock",
  payment_card: "lock",
  payment: "lock",
  takeover: "hand",
  hire: "user",
  handoff: "user",
};
const STATUS_GLYPH: Record<Exclude<Card["status"], "pending">, GlyphName> = { done: "check", failed: "alert", expired: "clock" };
const ROW_KINDS: readonly CardKind[] = ["activity", "handoff", "hire"];
// Past this many list items or table rows a result card shows a preview with a labelled disclosure.
const PREVIEW_ROWS = 3;

function variantOf(card: Card): Variant {
  if (card.interaction === "ask") return "ask";
  return ROW_KINDS.includes(card.kind) ? "row" : "result";
}

function resolveAnswerOf(action: Action) {
  return action.target.type === "ticket_resolve" ? action.target.answer : null;
}

function sortActions(actions: Action[]): Action[] {
  // The affirmative action reads last, where the thumb lands.
  return [...actions].sort((a, b) => Number(a.style === "primary") - Number(b.style === "primary"));
}

function countRows(blocks: Block[]): number {
  return blocks.reduce((total, block) => total + (block.type === "list_items" ? block.items.length : block.type === "table" ? block.rows.length : 0), 0);
}

function reducedMotion(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

class CardView implements MountedCard {
  readonly element: HTMLElement;
  private card: Card;
  private signature: string;
  private bodySignature: string;
  private isOpen: boolean;
  private busy = false;
  private submitted = false;
  private previewAll = false;
  private disposed = false;
  private announceTimer: number | undefined;
  private readonly shell = element("section", { class: "oc-card" });
  private readonly bubbles = element("div", { class: "oc-bubbles" });
  private readonly live = element("div", { class: "oc-sr", role: "status", "aria-live": "polite" });
  private readonly bodyId = uid("oc-body");
  private readonly titleId = uid("oc-title");
  private readonly registry = new FieldRegistry();
  private readonly media: Media;
  private readonly transient: Transient = { refusedKey: null };
  private buttons = new Map<string, HTMLElement>();
  private opener: HTMLElement | null = null;
  private bodyNode: HTMLElement | null = null;

  constructor(
    host: HTMLElement,
    card: Card,
    private readonly options: CardMountOptions,
  ) {
    this.card = card;
    this.signature = JSON.stringify(card);
    this.bodySignature = JSON.stringify([card.blocks, card.actions]);
    this.media = new Media(options.port, options.sessionId);
    this.element = element("div", { class: "oc-root" }, this.shell, this.bubbles, this.live);
    this.element.addEventListener("keydown", (event) => this.onKeydown(event));
    this.isOpen = this.initialOpen();
    this.setInert(options.inert === true);
    host.append(this.element);
    this.renderChrome();
    if (this.isOpen) this.renderBody();
    if (this.isOpen && variantOf(card) !== "result") this.claimFocus(options.focus ?? "polite");
  }

  private get variant(): Variant {
    return variantOf(this.card);
  }

  private get pendingAsk(): boolean {
    return this.variant === "ask" && this.card.status === "pending";
  }

  private initialOpen(): boolean {
    if (this.options.layout === "sheet" || this.variant === "result") return true;
    return this.card.status === "pending" && this.options.defaultOpen === true;
  }

  private get closable(): boolean {
    return this.variant !== "result" && this.options.layout !== "sheet";
  }

  /** Actions the person can take now: terminal cards keep only the ones that are not ticket answers. */
  private visibleActions(): Action[] {
    const live = this.card.status === "pending";
    return this.card.actions.filter((action) => {
      if (this.options.supports && !this.options.supports(action)) return false;
      const target = action.target.type;
      const ticketBound = target === "ticket_resolve" || target === "ticket_reject" || target === "ticket_done" || target === "takeover";
      return live || !ticketBound;
    });
  }

  private authenticatorAction(): Action | null {
    if (this.options.canApprove === false || !this.pendingAsk) return null;
    return this.visibleActions().find((action) => resolveAnswerOf(action)?.type === "otp_authenticator") ?? null;
  }

  private receiptBlocks(): Block[] {
    return this.card.status === "pending" ? this.card.blocks : this.card.blocks.filter((block) => block.type !== "input_fields" && block.type !== "choice_options");
  }

  private hasDetail(): boolean {
    return this.receiptBlocks().length > 0 || this.visibleActions().length > 0;
  }

  // ---- chrome: pill or header, bubbles ----------------------------------------------------

  private markFor(size: number): HTMLElement {
    const { card } = this;
    if (card.status !== "pending") {
      return element("span", { class: "oc-status", "data-status": card.status, role: "img", "aria-label": STATUS_LABEL[card.status] }, glyph(STATUS_GLYPH[card.status], size - 4));
    }
    if (card.kind === "activity") return element("span", { class: "oc-status", "data-status": "pending", role: "img", "aria-label": STATUS_LABEL.pending }, element("span", { class: "oc-spinner" }));
    return siteMark(card.site, this.media, card.site.origin || card.site.favicon_ref ? null : (KIND_GLYPH[card.kind] ?? null), size);
  }

  private renderChrome(): void {
    const { card, shell } = this;
    shell.setAttribute("data-card-id", card.id);
    shell.setAttribute("data-kind", card.kind);
    shell.setAttribute("data-status", card.status);
    shell.setAttribute("data-variant", this.variant);
    shell.setAttribute("data-open", String(this.isOpen));
    shell.setAttribute("data-layout", this.options.layout ?? "card");
    if (card.ticket_id) shell.setAttribute("data-ticket", card.ticket_id);
    else shell.removeAttribute("data-ticket");
    if (this.pendingAsk) shell.setAttribute("data-ask", "pending");
    else shell.removeAttribute("data-ask");

    for (const stale of [...shell.children]) if (stale.classList.contains("oc-pill") || stale.classList.contains("oc-head")) stale.remove();
    if (this.isOpen) {
      shell.setAttribute("role", "group");
      shell.setAttribute("aria-labelledby", this.titleId);
      shell.prepend(this.header());
    } else {
      shell.removeAttribute("role");
      shell.removeAttribute("aria-labelledby");
      shell.prepend(this.pill());
    }

    this.bubbles.replaceChildren(...this.card.blocks.flatMap((block) => (block.type === "notice" && block.placement === "bubble" ? [renderNotice(block)] : [])));
  }

  /** "<helper> asked": which child agent the request came from; the label is the host's, never the card's. */
  private askerLine(tag: "p" | "span"): HTMLElement {
    const { asker } = this.options;
    return element(tag, { class: "oc-asker", "data-subagent-id": asker?.id ?? "", title: asker?.id ?? "" }, `${asker?.label ?? ""} asked`);
  }

  private pill(): HTMLElement {
    const { card } = this;
    const label = element("span", { class: "oc-pill-text" }, card.collapsed_summary);
    const content = [this.markFor(20), ...(this.options.asker ? [this.askerLine("span")] : []), label];
    if (card.status !== "pending" && !this.hasDetail()) return element("div", { class: "oc-pill", "data-static": "" }, ...content);
    const pill = element("button", { type: "button", class: "oc-pill", "aria-expanded": "false", "aria-controls": this.bodyId }, ...content, element("span", { class: "oc-pill-chevron", "aria-hidden": "true" }, glyph("chevron-down", 14)));
    pill.addEventListener("click", () => this.setOpen(true, "force"));
    this.opener = pill;
    return pill;
  }

  private header(): HTMLElement {
    const { card } = this;
    const origin = verifiedOrigin(card.site.origin);
    const text = element("div", { class: "oc-head-text" });
    if (this.options.asker) text.append(this.askerLine("p"));
    text.append(element("h2", { class: "oc-title", id: this.titleId }, card.title));
    if (card.subtitle) text.append(element("p", { class: "oc-subtitle" }, card.subtitle));
    if (origin) text.append(element("p", { class: "oc-origin", "data-feedback-allow": "" }, origin));
    const head = element("header", { class: "oc-head" }, this.markFor(28), text);
    if (this.closable) {
      const close = element("button", { type: "button", class: "oc-close", "aria-label": "Collapse", "aria-expanded": "true", "aria-controls": this.bodyId }, glyph("chevron-down", 16));
      close.addEventListener("click", () => this.collapse());
      head.append(close);
    }
    return head;
  }

  // ---- body -------------------------------------------------------------------------------

  private teardownBody(): void {
    this.registry.reset();
    this.transient.refusedKey = null;
    this.bodyNode?.remove();
    this.bodyNode = null;
    this.buttons = new Map();
  }

  private renderBody(): void {
    this.teardownBody();
    const { card } = this;
    const visible = this.visibleActions();
    const authAction = this.authenticatorAction();
    const authAnswer = authAction ? resolveAnswerOf(authAction) : null;
    const skip = new Set<string>(authAnswer?.type === "otp_authenticator" ? [authAnswer.field_id, authAnswer.save_field_id] : []);
    const ctx: BlockContext = { card, media: this.media, registry: this.registry, run: (action, trigger) => void this.run(action, trigger), onEnter: () => this.submitPrimary(), skipFields: skip };

    const body = element("div", { class: "oc-body", id: this.bodyId });
    const blocks = this.receiptBlocks();
    const inner = element("div", { class: "oc-blocks" });
    for (const block of blocks) {
      const node = renderBlock(block, ctx);
      if (node) inner.append(node);
    }
    const clipped = this.variant === "result" && countRows(blocks) > PREVIEW_ROWS;
    const scroller = element("div", { class: "oc-scroll", id: uid("oc-scroll") }, inner);
    if (clipped) {
      // The first three rows across lists and tables stay; the rest are hidden until "Show all".
      const rows = inner.querySelectorAll(".oc-list > .oc-item, .oc-table tbody > tr");
      rows.forEach((row, index) => row.toggleAttribute("data-preview-hidden", index >= PREVIEW_ROWS));
      scroller.toggleAttribute("data-clipped", !this.previewAll);
    }
    const form = this.pendingAsk && blocks.some((block) => block.type === "input_fields") ? element("form", { class: "oc-form", novalidate: "" }) : element("div", { class: "oc-form" });
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      this.submitPrimary();
    });
    form.append(scroller);

    if (authAction && authAnswer?.type === "otp_authenticator") form.append(this.authenticatorSection(authAction, authAnswer.field_id, authAnswer.save_field_id));

    if (clipped) {
      const more = element("button", { type: "button", class: "oc-more", "aria-expanded": String(this.previewAll), "aria-controls": scroller.id }, this.previewAll ? "Show less" : `Show all ${countRows(blocks)}`);
      more.addEventListener("click", () => {
        this.previewAll = !this.previewAll;
        scroller.toggleAttribute("data-clipped", !this.previewAll);
        more.setAttribute("aria-expanded", String(this.previewAll));
        more.textContent = this.previewAll ? "Show less" : `Show all ${countRows(blocks)}`;
      });
      form.append(more);
    }
    for (const block of blocks) {
      if (block.type !== "kv_rows") continue;
      const total = renderTotal(block, ctx);
      if (total) form.append(element("div", { class: "oc-foot" }, total));
    }

    form.append(element("div", { class: "oc-slot", "data-slot": "error" }));
    const actions = this.actionsRow(visible.filter((action) => action !== authAction));
    if (actions) form.append(actions);
    if (this.pendingAsk && blocks.some((block) => block.type === "input_fields" && block.fields.some(isSecretField))) {
      form.append(element("p", { class: "oc-secure" }, glyph("lock", 12), element("span", {}, SECURE_NOTE), brandMark(12), element("span", {}, "openside")));
    }
    body.append(form);
    this.shell.append(body);
    this.bodyNode = body;
    this.applyBusy(null);
  }

  private authenticatorSection(action: Action, keyId: string, saveId: string): HTMLElement {
    const fields = fieldsOf(this.card, this.card.blocks.map((block) => block.id));
    const key = fields.find((field) => field.id === keyId);
    const save = fields.find((field) => field.id === saveId);
    const section = element("details", { class: "oc-details" }, element("summary", {}, AUTHENTICATOR_SUMMARY));
    const run = element("button", { type: "button", class: "oc-btn oc-btn--secondary" }, action.label);
    run.addEventListener("click", () => void this.run(action, run));
    this.buttons.set(action.id, run);
    const fieldContext = { registry: this.registry, cardOrigin: this.card.site.origin ?? null, onEnter: () => run.click() };
    const box = element("div", { class: "oc-fields", "data-feedback-redact": "" });
    if (key) box.append(renderField(key, fieldContext));
    if (save) box.append(renderField(save, fieldContext));
    section.append(box, run);
    return section;
  }

  private actionsRow(actions: Action[]): HTMLElement | null {
    // A link whose address is not plain http(s) is not drawn at all, so no context-menu path reaches it.
    const shown = actions.filter((action) => !this.isHostedElsewhere(action) && (action.target.type !== "link" || safeHref(action.target.url) !== null));
    if (shown.length === 0) return null;
    const row = element("div", { class: "oc-actions", "data-count": String(shown.length) });
    for (const action of sortActions(shown)) {
      const classes = `oc-btn oc-btn--${action.style}`;
      let node: HTMLElement;
      const href = action.target.type === "link" ? safeHref(action.target.url) : null;
      if (action.target.type === "link" && href) {
        node = element("a", { class: classes, href, target: "_blank", rel: "noopener noreferrer" }, action.label, glyph("external", 14), element("span", { class: "oc-host" }, linkHost(href)));
        node.addEventListener("click", (event) => {
          event.preventDefault();
          void this.run(action, node);
        });
      } else {
        node = element("button", { type: "button", class: classes, "data-action": action.id }, action.label);
        node.addEventListener("click", () => void this.run(action, node));
      }
      this.buttons.set(action.id, node);
      row.append(node);
    }
    return row;
  }

  /** Choice options carry their own action; the same action is not also a footer button. */
  private isHostedElsewhere(action: Action): boolean {
    return this.card.blocks.some((block) => block.type === "choice_options" && block.options.some((option) => option.action?.id === action.id));
  }

  // ---- behaviour --------------------------------------------------------------------------

  /** Enter in a field presses the card's affirmative ticket action. */
  private submitPrimary(): void {
    const primary = this.visibleActions().find((action) => action.style === "primary" && action.target.type === "ticket_resolve" && resolveAnswerOf(action)?.type !== "otp_authenticator");
    const button = primary ? this.buttons.get(primary.id) : null;
    button?.click();
  }

  private applyBusy(trigger: HTMLElement | null): void {
    const locked = this.busy || this.submitted;
    for (const control of this.bodyNode?.querySelectorAll<HTMLElement>(".oc-btn, .oc-option, .oc-provider") ?? []) {
      if (control instanceof HTMLButtonElement) control.disabled = locked;
      else control.toggleAttribute("aria-disabled", locked);
    }
    if (trigger) setPending(trigger, this.busy);
  }

  private setError(message: string | null): void {
    const slot = this.bodyNode?.querySelector('[data-slot="error"]');
    if (!slot) return;
    slot.replaceChildren(...(message ? [element("p", { class: "oc-error", role: "alert" }, message)] : []));
  }

  private async run(action: Action, trigger: HTMLElement): Promise<void> {
    if (this.busy || this.submitted || this.disposed) return;
    this.setError(null);
    this.busy = true;
    this.applyBusy(trigger);
    let outcome: ActionOutcome | null = null;
    try {
      outcome = await dispatchAction(action, {
        card: this.card,
        port: this.options.port,
        fields: this.registry,
        host: this.options.onAction,
        canApprove: this.options.canApprove,
        transient: this.transient,
        ...(this.options.openUrl ? { openUrl: this.options.openUrl } : {}),
      });
    } catch (cause) {
      if (this.disposed) return;
      if (cause instanceof SaveRefused && cause.fieldId) {
        for (const box of this.bodyNode?.querySelectorAll<HTMLElement>(".oc-check") ?? []) box.hidden = true;
      }
      this.setError(failureMessage(cause));
    }
    this.busy = false;
    if (this.disposed) return;
    if (outcome?.kind === "settled") {
      // The daemon's updated card arrives through the host; until then the card stays locked.
      this.submitted = true;
      this.options.onSettled?.(outcome.outcome, this.card);
    } else if (outcome?.kind === "invalid") {
      this.setError(outcome.message);
      if (outcome.focus) this.registry.focus(outcome.focus);
    } else if (outcome?.kind === "unhandled") this.setError(UNHANDLED_ACTION);
    this.applyBusy(trigger);
    if (!this.submitted && document.activeElement === document.body) trigger.focus?.();
  }

  private onKeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape" || !this.isOpen || !this.closable) return;
    event.preventDefault();
    event.stopPropagation();
    this.collapse();
  }

  private claimFocus(mode: "polite" | "force" | "none"): void {
    if (mode === "none") return;
    const active = document.activeElement;
    const typing = active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement || (active instanceof HTMLElement && active.isContentEditable);
    if (mode === "polite" && typing && !this.element.contains(active)) return;
    const target = this.registry.firstControl() ?? this.bodyNode?.querySelector<HTMLElement>("button:not(:disabled), a[href]") ?? null;
    target?.focus({ preventScroll: mode === "polite" });
  }

  private morph(change: () => void): void {
    const before = this.shell.getBoundingClientRect();
    change();
    const after = this.shell.getBoundingClientRect();
    if (typeof this.shell.animate !== "function" || reducedMotion() || before.width === 0 || (before.width === after.width && before.height === after.height)) return;
    this.shell.classList.add("oc-morphing");
    const animation = this.shell.animate(
      [
        { width: `${before.width}px`, height: `${before.height}px` },
        { width: `${after.width}px`, height: `${after.height}px` },
      ],
      { duration: 220, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" },
    );
    const done = () => this.shell.classList.remove("oc-morphing");
    animation.addEventListener("finish", done);
    animation.addEventListener("cancel", done);
  }

  private setOpen(open: boolean, focus: "polite" | "force" | "none"): void {
    if (open === this.isOpen || this.disposed) return;
    const hadFocus = this.element.contains(document.activeElement);
    const opener = this.opener;
    this.morph(() => {
      this.isOpen = open;
      if (!open) this.teardownBody();
      this.renderChrome();
      if (open) this.renderBody();
    });
    if (open) this.claimFocus(focus);
    else if (hadFocus) (this.element.querySelector<HTMLElement>(".oc-pill") ?? opener)?.focus();
  }

  open(): void {
    this.setOpen(true, "force");
  }

  collapse(): void {
    if (this.closable) this.setOpen(false, "none");
  }

  update(next: Card): void {
    if (this.disposed) return;
    const signature = JSON.stringify(next);
    if (signature === this.signature) return;
    const previous = this.card;
    this.card = next;
    this.signature = signature;
    this.submitted = false;
    const bodySignature = JSON.stringify([next.blocks, next.actions]);
    const settledNow = previous.status === "pending" && next.status !== "pending";
    const rebuild = settledNow || bodySignature !== this.bodySignature || variantOf(previous) !== variantOf(next);
    this.bodySignature = bodySignature;

    if (settledNow) {
      // Whatever was typed is gone before the receipt shows, and the open card folds into it.
      this.registry.clearAll();
      this.transient.refusedKey = null;
      this.announce(next.collapsed_summary);
      if (this.variant === "ask" && this.options.layout !== "sheet") this.isOpen = false;
    }
    this.morph(() => {
      if (rebuild) this.teardownBody();
      this.renderChrome();
      if (this.isOpen && rebuild) this.renderBody();
    });
    if (!rebuild) this.applyBusy(null);
  }

  /** The polite live region speaks once, then empties, so the same words never sit in the page twice (receipt pill and region). */
  private announce(text: string): void {
    this.live.textContent = text;
    window.clearTimeout(this.announceTimer);
    this.announceTimer = window.setTimeout(() => {
      this.live.textContent = "";
    }, 1500);
  }

  setInert(inert: boolean): void {
    this.element.toggleAttribute("inert", inert);
    if (inert) this.element.setAttribute("aria-hidden", "true");
    else this.element.removeAttribute("aria-hidden");
  }

  dispose(): void {
    if (this.disposed) return;
    this.registry.clearAll();
    this.transient.refusedKey = null;
    this.disposed = true;
    window.clearTimeout(this.announceTimer);
    this.media.dispose();
    this.element.remove();
  }
}

export function mountCard(host: HTMLElement, card: Card, options: CardMountOptions): MountedCard {
  return new CardView(host, card, options);
}
