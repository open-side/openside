// The secure login card, the login-step choice card and the phone-approval wait card
// (`request_login` / `request_choice` / `wait_for_approval` tickets), shared by the web app
// (components/AuthForm.tsx) and the extension side panel.
// Values the person types live only in the inputs: submit reads them, empties every input and
// only then sends; nothing is kept in app state, props or storage.
import { ApiError, type Client } from "../api/client";
import type { Ticket } from "../api/types";
import { authFoot, authHead, element, setPending, siteOf, type AuthButton } from "./authCard";
import { glyph, providerGlyph, type GlyphName } from "./glyphs";

type Login = NonNullable<Ticket["login"]>;
type LoginField = Login["fields"][number];
export type LoginTicket = Ticket & { login: Login };
export type ChoiceTicket = Ticket & { choice: NonNullable<Ticket["choice"]> };
export type ApprovalTicket = Ticket & { approval: NonNullable<Ticket["approval"]> };

export function isLoginTicket(ticket: Ticket): ticket is LoginTicket {
  return ticket.kind === "login" && !!ticket.login;
}

export function isChoiceTicket(ticket: Ticket): ticket is ChoiceTicket {
  return ticket.kind === "choice" && !!ticket.choice;
}

export function isApprovalTicket(ticket: Ticket): ticket is ApprovalTicket {
  return ticket.kind === "device_approval" && !!ticket.approval;
}

/** The daemon-verified site of any ticket that targets one, as a host (never the page's own name). */
export function ticketSite(ticket: Ticket): string {
  return siteOf(ticket.login?.origin ?? ticket.choice?.origin ?? ticket.approval?.origin ?? ticket.target?.origin);
}

/**
 * The person-facing result of a finished login or choice ticket, for the event chip; `failed`
 * when the page showed an error after the answer.
 */
export function authOutcome(ticket: Ticket): { text: string; failed: boolean } | null {
  if (ticket.status !== "resolved" || (ticket.kind !== "login" && ticket.kind !== "choice" && !isApprovalTicket(ticket))) return null;
  const resolution: Record<string, unknown> =
    ticket.resolution && typeof ticket.resolution === "object" ? { ...ticket.resolution } : {};
  const site = ticketSite(ticket);
  if (resolution.next === "error") {
    const detail = typeof resolution.error_text === "string" && resolution.error_text ? `: ${resolution.error_text}` : "";
    return { text: `${site} showed an error${detail}`, failed: true };
  }
  if (ticket.kind === "choice") {
    const picked = ticket.choice?.options.find((option) => option.id === resolution.option);
    return { text: picked ? `Chose “${picked.label}”` : "Choice sent", failed: false };
  }
  if (ticket.approval) return { text: `Sign-in to ${site} approved on your phone`, failed: false };
  if (resolution.status === "filled_from_vault") return { text: `Signed in to ${site} with a saved account`, failed: false };
  if (resolution.status === "alternative") return { text: `Continued to another sign-in for ${site}`, failed: false };
  return { text: resolution.saved === true ? `${site} login saved to the vault` : `${site} login sent`, failed: false };
}

const ALTERNATIVE_GLYPH: Record<Login["alternatives"][number]["kind"], GlyphName> = {
  provider: "key",
  passkey: "passkey",
  qr: "qr",
  other: "key",
};

export interface AuthFormOptions<T> {
  ticket: T;
  client: Pick<Client, "resolveTicket" | "rejectTicket">;
  buttonClass: (variant: AuthButton) => string;
  /** `sheet` is the phone screen a push notification opens. */
  layout?: "card" | "sheet";
  focus?: "polite" | "force" | "none";
  onSubmitted?: () => void;
}

export interface LoginFormOptions extends AuthFormOptions<LoginTicket> {
  /**
   * Whether this device's token holds `approve`, which saving a login needs. `false` hides the
   * save option; `undefined` (scopes unknown) shows it and falls back on a 403.
   */
  canSave?: boolean;
}

let mounts = 0;

function failure(cause: unknown): string {
  if (cause instanceof ApiError) {
    if (cause.code === "target_unavailable" || cause.code === "origin_mismatch")
      return "The sign-in page changed before your details could be filled. Ask the agent to open it again.";
    return cause.message;
  }
  return cause instanceof Error ? cause.message : "Request failed";
}

/** Focuses `target` unless the person is typing somewhere else (`polite`). */
function claimFocus(host: HTMLElement, target: HTMLElement | undefined, focus: "polite" | "force" | "none") {
  const run = () => {
    const active = document.activeElement;
    const typingElsewhere =
      active instanceof HTMLElement && !host.contains(active) && active.matches("input, textarea, select, [contenteditable=true]");
    if (focus === "force" || (focus === "polite" && !typingElsewhere)) target?.focus({ preventScroll: true });
  };
  // A host built off-document (the side panel renders cards before inserting them) can take
  // focus only once it is attached.
  if (host.isConnected) run();
  else queueMicrotask(run);
}

function autocompleteOf(field: LoginField): string {
  if (field.kind === "identifier") return "username";
  if (field.kind === "password") return "current-password";
  return field.autocomplete || "off";
}

/** Renders the secure login card into `host`; returns the cleanup that removes it. */
export function mountLoginForm(host: HTMLElement, options: LoginFormOptions): () => void {
  const { ticket, client, buttonClass, layout = "card" } = options;
  const login = ticket.login;
  const id = `login-${++mounts}`;
  // Every name in the card comes from the verified origin; the page's own `site_name` appears
  // only as a labelled claim in the header.
  const site = siteOf(login.origin);

  const controls: (HTMLInputElement | HTMLButtonElement)[] = [];
  const error = element("p", { class: "auth-error", role: "alert" });
  error.hidden = true;
  const submit = element("button", { type: "submit", class: `${buttonClass("primary")} auth-primary` }, login.submit_label || "Log in");

  const setBusy = (busy: boolean, pending?: HTMLButtonElement) => {
    for (const control of controls) control.disabled = busy;
    if (pending) setPending(pending, busy);
  };
  const run = async (body: unknown, pending: HTMLButtonElement, onError?: (cause: unknown) => boolean) => {
    setBusy(true, pending);
    error.hidden = true;
    try {
      await client.resolveTicket(ticket.id, body);
      options.onSubmitted?.();
    } catch (cause) {
      setBusy(false, pending);
      if (onError?.(cause)) return;
      error.textContent = failure(cause);
      error.hidden = false;
    }
  };

  const { head, close } = authHead({
    titleId: `${id}-title`,
    title: "Log in",
    site,
    origin: login.origin,
    pageName: login.site_name,
    icon: login.icon,
    closeLabel: "Cancel login",
    onClose: () => {
      setBusy(true);
      void client.rejectTicket(ticket.id).then(
        () => options.onSubmitted?.(),
        (cause: unknown) => {
          setBusy(false);
          error.textContent = failure(cause);
          error.hidden = false;
        },
      );
    },
  });
  controls.push(close);

  const form = element(
    "form",
    { class: `auth-card auth-card--${layout} login-form`, "aria-labelledby": `${id}-title ${id}-title-origin`, "data-ticket": ticket.id, novalidate: "" },
    head,
    element(
      "p",
      { class: "auth-lede" },
      `Enter your ${site} details. They go straight into the page${login.can_save && options.canSave !== false ? " and, if you choose, to your encrypted vault" : ""}, never into the chat.`,
    ),
  );
  if (login.error_text) {
    form.append(element("p", { class: "auth-page-error", role: "status" }, glyph("alert", 18), element("span", {}, `${site} says: ${login.error_text}`)));
  }

  const accounts = login.accounts ?? [];
  if (accounts.length) {
    const list = element("div", { class: "auth-accounts", role: "group", "aria-label": "Saved accounts" });
    for (const account of accounts) {
      const row = element(
        "button",
        { type: "button", class: "auth-row auth-account", "data-account": account.item_id },
        element("span", { class: "auth-account-avatar", "aria-hidden": "true" }, (account.username_hint[0] ?? "?").toUpperCase()),
        element("span", { class: "auth-row-text" }, element("span", { class: "auth-row-label" }, account.username_hint), element("span", { class: "auth-row-desc" }, "Saved in your vault")),
        glyph("chevron", 18),
      );
      row.addEventListener("click", () => void run({ account: account.item_id }, row));
      controls.push(row);
      list.append(row);
    }
    form.append(element("p", { class: "auth-section" }, "Use a saved account"), list, element("p", { class: "auth-section auth-or" }, "or enter other details"));
  }

  const inputs = new Map<string, HTMLInputElement>();
  const checkboxes = new Map<string, HTMLInputElement>();
  for (const [index, field] of login.fields.entries()) {
    const fieldId = `${id}-f${index}`;
    if (field.kind === "checkbox") {
      const box = element("input", { id: fieldId, type: "checkbox", class: "auth-checkbox" });
      checkboxes.set(field.id, box);
      controls.push(box);
      form.append(element("label", { class: "auth-check", for: fieldId }, box, field.label));
      continue;
    }
    const placeholder = field.placeholder && field.placeholder !== field.label ? field.placeholder : field.label;
    const input = element("input", {
      id: fieldId,
      class: "auth-input",
      type: field.masked ? "password" : field.kind === "identifier" && field.autocomplete === "email" ? "email" : "text",
      name: field.kind === "identifier" ? "username" : field.kind === "password" ? "password" : `field-${index}`,
      autocomplete: autocompleteOf(field),
      autocapitalize: "off",
      autocorrect: "off",
      spellcheck: "false",
      placeholder,
      ...(field.required ? { "aria-required": "true" } : {}),
    });
    inputs.set(field.id, input);
    controls.push(input);
    const wrap = element("div", { class: "auth-input-wrap" }, input);
    if (field.masked) {
      const eye = element("button", { type: "button", class: "auth-eye", "aria-label": `Show ${field.label}`, "aria-pressed": "false", "aria-controls": fieldId });
      eye.append(glyph("eye", 20));
      eye.addEventListener("click", () => {
        const shown = input.type === "password";
        input.type = shown ? "text" : "password";
        eye.setAttribute("aria-pressed", String(shown));
        eye.setAttribute("aria-label", `${shown ? "Hide" : "Show"} ${field.label}`);
        eye.replaceChildren(glyph(shown ? "eye-off" : "eye", 20));
      });
      controls.push(eye);
      wrap.append(eye);
      wrap.classList.add("auth-input-wrap--eye");
    }
    form.append(element("div", { class: "auth-field" }, element("label", { class: "auth-label", for: fieldId }, field.label), wrap));
  }

  let save: HTMLInputElement | null = null;
  let saveLabel: HTMLLabelElement | null = null;
  if (login.can_save && options.canSave !== false) {
    save = element("input", { id: `${id}-save`, type: "checkbox", class: "auth-checkbox" });
    save.checked = true;
    saveLabel = element("label", { class: "auth-check auth-save", for: `${id}-save` }, save, "Save to vault");
    controls.push(save);
    form.append(saveLabel);
  }

  form.append(error);

  const alternatives = element("div", { class: "auth-alts", role: "group", "aria-label": "Other ways to sign in" });
  for (const alternative of login.alternatives) {
    const name = alternative.provider ?? providerGlyph(alternative.label) ?? ALTERNATIVE_GLYPH[alternative.kind];
    const button = element("button", { type: "button", class: "auth-alt", "aria-label": alternative.label, title: alternative.label, "data-alternative": alternative.id }, glyph(name, 22));
    button.addEventListener("click", () => void run({ alternative: alternative.id }, button));
    controls.push(button);
    alternatives.append(button);
  }
  controls.push(submit);
  form.append(element("div", { class: "auth-actions" }, alternatives, submit), authFoot());

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const missing = login.fields.find((field) => field.required && field.kind !== "checkbox" && !inputs.get(field.id)?.value);
    if (missing) {
      error.textContent = `Enter your ${missing.label.toLowerCase()}.`;
      error.hidden = false;
      inputs.get(missing.id)?.focus();
      return;
    }
    const values: Record<string, string | boolean> = {};
    for (const [fieldId, input] of inputs) {
      values[fieldId] = input.value;
      input.value = "";
      if (input.getAttribute("autocomplete") === "current-password") input.type = "password";
    }
    for (const [fieldId, box] of checkboxes) values[fieldId] = box.checked;
    const saving = !!save?.checked;
    void run({ values, save: saving }, submit, (cause) => {
      // 403 `forbidden`: this device lacks `approve`, which saving needs. Nothing was filled, so
      // the details go back into their fields (they were on screen a moment ago) and the person
      // can log in without saving by pressing the button once more.
      if (!(saving && cause instanceof ApiError && cause.status === 403 && cause.code === "forbidden")) return false;
      for (const [fieldId, input] of inputs) input.value = String(values[fieldId] ?? "");
      if (save) save.checked = false;
      if (saveLabel) saveLabel.hidden = true;
      error.textContent = "Saving to the vault needs approval rights on this device. Log in again to continue without saving.";
      error.hidden = false;
      submit.focus();
      return true;
    });
  });

  host.append(form);
  const first = [...inputs.values()][0];
  claimFocus(host, accounts.length ? form.querySelector<HTMLElement>(".auth-account") ?? first : first, options.focus ?? "polite");
  return () => form.remove();
}

/** Renders the choice card (pick one option the site offers) into `host`. */
export function mountChoiceForm(host: HTMLElement, options: AuthFormOptions<ChoiceTicket>): () => void {
  const { ticket, client, buttonClass, layout = "card" } = options;
  const choice = ticket.choice;
  const id = `choice-${++mounts}`;
  const site = siteOf(choice.origin);
  const error = element("p", { class: "auth-error", role: "alert" });
  error.hidden = true;
  const submit = element("button", { type: "submit", class: `${buttonClass("primary")} auth-primary`, disabled: "" }, "Continue");
  const radios: HTMLInputElement[] = [];

  const setBusy = (busy: boolean) => {
    for (const control of [...radios, submit, close]) control.disabled = busy;
    setPending(submit, busy);
    if (!busy) submit.disabled = !radios.some((radio) => radio.checked);
  };
  const fail = (cause: unknown) => {
    setBusy(false);
    error.textContent = failure(cause);
    error.hidden = false;
  };
  const { head, close } = authHead({
    titleId: `${id}-title`,
    title: "Choose an option",
    site,
    origin: choice.origin,
    pageName: choice.site_name,
    icon: choice.icon,
    closeLabel: "Dismiss",
    onClose: () => {
      setBusy(true);
      void client.rejectTicket(ticket.id).then(() => options.onSubmitted?.(), fail);
    },
  });

  const list = element("div", { class: "auth-options", role: "radiogroup", "aria-labelledby": `${id}-question` });
  for (const [index, option] of choice.options.entries()) {
    const radio = element("input", { id: `${id}-o${index}`, type: "radio", name: id, value: option.id, class: "auth-radio" });
    radio.addEventListener("change", () => {
      submit.disabled = false;
    });
    radios.push(radio);
    const text = element("span", { class: "auth-row-text" }, element("span", { class: "auth-row-label" }, option.label));
    if (option.description) text.append(element("span", { class: "auth-row-desc" }, option.description));
    list.append(element("label", { class: "auth-row auth-option", for: radio.id }, radio, text));
  }

  const form = element(
    "form",
    { class: `auth-card auth-card--${layout} choice-form`, "aria-labelledby": `${id}-title ${id}-title-origin`, "data-ticket": ticket.id, novalidate: "" },
    head,
    element("p", { id: `${id}-question`, class: "auth-question" }, choice.question),
    list,
    error,
    element("div", { class: "auth-actions" }, element("span", { class: "auth-alts" }), submit),
    authFoot(),
  );
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const picked = radios.find((radio) => radio.checked);
    if (!picked) return;
    setBusy(true);
    error.hidden = true;
    client.resolveTicket(ticket.id, { option: picked.value }).then(() => options.onSubmitted?.(), fail);
  });

  host.append(form);
  claimFocus(host, radios[0], options.focus ?? "polite");
  return () => form.remove();
}

function elapsed(seconds: number): string {
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}

/**
 * The wait card for a sign-in the site asks the person to approve on their phone. Nothing is
 * answered here: the daemon resolves the ticket once the page moves on, so the card only shows
 * the live wait and lets the person cancel it.
 */
export function mountApprovalCard(host: HTMLElement, options: AuthFormOptions<ApprovalTicket>): () => void {
  const { ticket, client, buttonClass, layout = "card" } = options;
  const approval = ticket.approval;
  const id = `approval-${++mounts}`;
  const site = siteOf(approval.origin);
  const error = element("p", { class: "auth-error", role: "alert" });
  error.hidden = true;
  const cancel = element("button", { type: "button", class: `${buttonClass("secondary")} auth-primary` }, "Cancel");
  const fail = (cause: unknown) => {
    cancel.disabled = false;
    close.disabled = false;
    setPending(cancel, false);
    error.textContent = failure(cause);
    error.hidden = false;
  };
  const stop = () => {
    cancel.disabled = true;
    close.disabled = true;
    setPending(cancel, true);
    void client.rejectTicket(ticket.id).then(() => options.onSubmitted?.(), fail);
  };
  const { head, close } = authHead({
    titleId: `${id}-title`,
    title: "Approve on your phone",
    site,
    origin: approval.origin,
    pageName: approval.site_name,
    icon: approval.icon,
    closeLabel: "Cancel sign-in",
    onClose: stop,
  });
  cancel.addEventListener("click", stop);

  const started = Date.parse(ticket.created_at);
  // Hidden from assistive tech: inside the live status it would be announced every second.
  const clock = element("span", { class: "auth-wait-clock", "aria-hidden": "true" });
  const tick = () => {
    clock.textContent = elapsed(Math.max(0, Math.floor((Date.now() - (Number.isNaN(started) ? Date.now() : started)) / 1000)));
  };
  tick();
  const timer = window.setInterval(tick, 1000);

  const visual = element("div", { class: "auth-wait-visual", "aria-hidden": "true" }, element("span", { class: "auth-wait-ring" }), glyph("phone", 30));
  const body = element("div", { class: "auth-wait" }, visual);
  if (approval.number) {
    body.append(
      element("p", { class: "auth-wait-hint" }, "Tap this number on your phone"),
      element("p", { class: "auth-wait-number", "aria-label": `Number ${approval.number}` }, approval.number),
    );
  }
  body.append(element("p", { class: "auth-question" }, `${site} sent a sign-in request to your phone. Approve it there and openside continues by itself.`));
  if (approval.message) body.append(element("p", { class: "auth-wait-page" }, approval.message));
  const status = element("p", { class: "auth-wait-status", role: "status", "aria-live": "polite" }, element("span", { class: "auth-wait-dot", "aria-hidden": "true" }), "Waiting for approval", clock);

  const card = element(
    "section",
    { class: `auth-card auth-card--${layout} approval-card`, "aria-labelledby": `${id}-title ${id}-title-origin`, "data-ticket": ticket.id },
    head,
    body,
    status,
    error,
    element("div", { class: "auth-actions" }, element("span", { class: "auth-alts" }), cancel),
    authFoot(),
  );
  host.append(card);
  return () => {
    window.clearInterval(timer);
    card.remove();
  };
}
