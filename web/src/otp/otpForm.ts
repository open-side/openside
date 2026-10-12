// The one-time-code form shared by the web app (wrapped by components/OtpForm.tsx) and the
// extension side panel. Plain DOM so both hosts render the exact same form; each host passes
// its own button classes. The code and the setup key never enter any app state: they live
// only in the input until submit, which empties the input before the request is sent.
import { ApiError, type Client } from "../api/client";
import type { Ticket } from "../api/types";
import { parseAuthenticator, totpCode } from "./totp";

type Otp = NonNullable<Ticket["otp"]>;
export type OtpTicket = Ticket & { otp: Otp };
export type OtpButton = "primary" | "secondary" | "ghost";

const SOURCE: Record<Otp["channel"], string | null> = {
  authenticator: "your authenticator app",
  sms: "a text message",
  email: "your email",
  backup_code: "your backup codes",
  unknown: null,
};

export function isOtpTicket(ticket: Ticket): ticket is OtpTicket {
  return ticket.kind === "secret_input" && !!ticket.otp;
}

/** Host shown to the person: `https://www.example.com:443` → `example.com`. */
function siteOf(origin: string | null | undefined): string {
  try {
    return new URL(origin ?? "").host.replace(/^www\./, "") || "this site";
  } catch {
    return "this site";
  }
}

export interface OtpRegistration {
  origin: string;
  issuer: string | null;
  account: string | null;
}

/**
 * Approval asked after the agent read a two-factor setup QR code: `confirm` tickets carrying
 * `otp_register` (never the seed). Read defensively until the field is in the generated schema.
 */
export function otpRegistrationOf(ticket: Ticket): OtpRegistration | null {
  if (ticket.kind !== "confirm" || !("otp_register" in ticket)) return null;
  const raw = ticket.otp_register;
  if (!raw || typeof raw !== "object" || !("origin" in raw)) return null;
  const { origin } = raw;
  const issuer = "issuer" in raw ? raw.issuer : null;
  const account = "account" in raw ? raw.account : null;
  if (typeof origin !== "string") return null;
  return {
    origin,
    issuer: typeof issuer === "string" && issuer ? issuer : null,
    account: typeof account === "string" && account ? account : null,
  };
}

export function registrationQuestion(info: OtpRegistration): string {
  const label = [info.issuer, info.account].filter(Boolean).join(" · ");
  return `Save the authenticator for ${siteOf(info.origin)}${label ? ` (${label})` : ""} to your vault?`;
}

export function registrationOutcome(resolution: unknown): string | null {
  if (!resolution || typeof resolution !== "object" || !("status" in resolution)) return null;
  const { status } = resolution;
  if (status === "registered") return "Authenticator saved";
  if (status === "declined") return "Declined";
  if (status === "setup_key_lost") return "Setup key expired";
  return null;
}

/** "Code from your email for example.com": where to find the code, for the form and notifications. */
export function codeHelp(ticket: OtpTicket): string {
  const site = siteOf(ticket.target?.origin);
  const source = SOURCE[ticket.otp.channel];
  return source ? `Code from ${source} for ${site}` : `Enter the code ${site} sent you`;
}

export interface OtpFormOptions {
  ticket: OtpTicket;
  client: Pick<Client, "resolveTicket" | "rejectTicket">;
  buttonClass: (variant: OtpButton) => string;
  /** `sheet` is the phone screen a push notification opens: bigger field and button. */
  layout?: "card" | "sheet";
  /**
   * `polite` focuses the code field unless the person is typing in another field (the composer);
   * `force` always does, for the sheet the person opened on purpose.
   */
  focus?: "polite" | "force" | "none";
  /**
   * Whether this device's token holds `approve`, which saving an authenticator needs. `false`
   * hides the save section; `undefined` (scopes unknown) shows it and falls back on a 403.
   */
  canSave?: boolean;
  onSubmitted?: () => void;
}

let mounts = 0;

function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attributes: Record<string, string> = {},
  ...children: (Node | string)[]
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) node.setAttribute(name, value);
  node.append(...children);
  return node;
}

/** Renders the code form into `host`; returns the cleanup that removes it. */
export function mountOtpForm(host: HTMLElement, options: OtpFormOptions): () => void {
  const { ticket, client, buttonClass, layout = "card" } = options;
  const id = `otp-${++mounts}`;
  const site = siteOf(ticket.target?.origin);
  const digits = ticket.otp.channel === "authenticator" || ticket.otp.channel === "sms";

  const input = element("input", {
    id: `${id}-code`,
    class: `otp-input${digits ? " otp-input--digits" : ""}`,
    type: "text",
    name: "one-time-code",
    autocomplete: "one-time-code",
    inputmode: digits ? "numeric" : "text",
    autocapitalize: "off",
    autocorrect: "off",
    spellcheck: "false",
    enterkeyhint: "done",
    "aria-describedby": `${id}-help`,
    ...(digits ? { pattern: "[0-9]*" } : {}),
  });
  const field = digits
    ? element(
        "div",
        { class: "otp-digits" },
        element("div", { class: "otp-cells", "aria-hidden": "true" }, ...Array.from({ length: 6 }, () => element("span"))),
        input,
      )
    : input;
  const error = element("p", { class: "otp-error", role: "alert" });
  error.hidden = true;
  const submit = element("button", { type: "submit", class: buttonClass("primary") }, "Submit");
  const reject = element("button", { type: "button", class: buttonClass("ghost") }, "Reject");

  const form = element(
    "form",
    { class: `otp-form otp-form--${layout}`, "aria-labelledby": `${id}-title`, "data-ticket": ticket.id, novalidate: "" },
    element(
      "div",
      { class: "otp-head" },
      element("span", { class: "otp-avatar", "aria-hidden": "true" }, site === "this site" ? "?" : site[0]!.toUpperCase()),
      element(
        "div",
        { class: "otp-heading" },
        element("strong", { id: `${id}-title`, class: "otp-title" }, "Verification code"),
        element("span", { class: "otp-site" }, site),
      ),
    ),
    ...(ticket.summary ? [element("p", { class: "otp-summary" }, ticket.summary)] : []),
    element("label", { class: "otp-label", for: `${id}-code` }, "Code"),
    field,
    element(
      "p",
      { id: `${id}-help`, class: "otp-help" },
      codeHelp(ticket),
    ),
    error,
    element("div", { class: "otp-actions" }, submit, reject),
  );

  const controls: (HTMLInputElement | HTMLButtonElement)[] = [input, submit, reject];
  const setBusy = (busy: boolean) => {
    for (const control of controls) control.disabled = busy;
  };
  const fail = (cause: unknown, retry: HTMLElement) => {
    error.textContent = cause instanceof ApiError ? cause.message : cause instanceof Error ? cause.message : "Request failed";
    error.hidden = false;
    setBusy(false);
    retry.focus();
  };
  const run = async (action: () => Promise<unknown>, retry: HTMLElement, onError = fail) => {
    setBusy(true);
    error.hidden = true;
    try {
      await action();
      options.onSubmitted?.();
    } catch (cause) {
      onError(cause, retry);
    }
  };

  if (digits) {
    // Pasted or autofilled codes often arrive as "123 456" or "123-456".
    input.addEventListener("input", () => {
      const clean = input.value.replace(/\D/g, "");
      if (clean !== input.value) input.value = clean;
    });
  }
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const code = input.value.replace(/\s/g, "");
    input.value = "";
    if (!code) {
      input.focus();
      return;
    }
    void run(() => client.resolveTicket(ticket.id, code), input);
  });
  reject.addEventListener("click", () => void run(() => client.rejectTicket(ticket.id), input));

  if (ticket.otp.can_register && options.canSave !== false) {
    const key = element("input", {
      id: `${id}-key`,
      class: "otp-key",
      type: "password",
      name: "openside-totp",
      autocomplete: "off",
      autocapitalize: "off",
      spellcheck: "false",
      placeholder: "Setup key or otpauth:// link",
    });
    const keep = element("input", { type: "checkbox", class: "otp-keep" });
    keep.checked = true;
    const keepLabel = element("label", { class: "otp-check" }, keep, "Don't ask again for this site");
    const note = element("p", { class: "otp-note" });
    const save = element("button", { type: "button", class: buttonClass("secondary") });
    // After a refused save, the key the person already pasted stays here (never in the page)
    // so "Fill this code" works without pasting it again.
    let refusedKey: ReturnType<typeof parseAuthenticator> = null;
    const describe = () => {
      note.textContent = keep.checked
        ? `openside keeps it in the vault for ${site}, fills this code now and fills later codes without asking.`
        : "openside uses it for this code only and keeps nothing.";
      save.textContent = keep.checked ? "Save and fill" : "Fill this code";
    };
    describe();
    keep.addEventListener("change", describe);
    controls.push(key, keep, save);
    const fillOnce = (params: ReturnType<typeof parseAuthenticator>) =>
      void run(async () => {
        if (!params) throw new Error("That is not a setup key or otpauth:// link.");
        return client.resolveTicket(ticket.id, await totpCode(params));
      }, key);
    // 403 `forbidden`: the token lacks `approve`; 403 `vault_denied`: the vault policy keeps this
    // site out. Either way only saving is refused, so the same key can still fill this one code.
    const refusedSave = (cause: unknown, retry: HTMLElement) => {
      if (!(cause instanceof ApiError && cause.status === 403)) {
        refusedKey = null;
        return fail(cause, retry);
      }
      keep.checked = false;
      keepLabel.hidden = true;
      describe();
      const reason = cause.code === "forbidden" ? "Saving an authenticator needs approval rights on this device." : cause.message;
      fail(new Error(`${reason} You can still fill this one code.`), refusedKey ? save : key);
      keep.disabled = true;
    };
    const register = () => {
      const value = key.value.trim();
      key.value = "";
      if (!value) {
        if (refusedKey) {
          const params = refusedKey;
          refusedKey = null;
          fillOnce(params);
          return;
        }
        key.focus();
        return;
      }
      const params = parseAuthenticator(value);
      if (keep.checked) {
        refusedKey = params;
        void run(() => client.resolveTicket(ticket.id, { totp: value }), key, refusedSave);
        return;
      }
      refusedKey = null;
      fillOnce(params);
    };
    save.addEventListener("click", register);
    // The key field sits inside the code form; Enter there saves the key instead of submitting
    // an empty code.
    key.addEventListener("keydown", (event) => {
      if (event.key !== "Enter") return;
      event.preventDefault();
      register();
    });
    form.append(
      element(
        "details",
        { class: "otp-register" },
        element("summary", {}, "Save this site's authenticator"),
        element("p", { class: "otp-note" }, "Paste the setup key or otpauth:// link from this site's two-factor setup."),
        element("label", { class: "otp-label", for: `${id}-key` }, "Setup key or otpauth:// link"),
        key,
        keepLabel,
        note,
        element("div", { class: "otp-actions" }, save),
      ),
    );
  }

  host.append(form);
  const focus = options.focus ?? "polite";
  const claimFocus = () => {
    const active = document.activeElement;
    const typingElsewhere =
      active instanceof HTMLElement && !host.contains(active) && active.matches("input, textarea, select, [contenteditable=true]");
    if (focus === "force" || (focus === "polite" && !typingElsewhere)) input.focus({ preventScroll: true });
  };
  // A host built off-document (the side panel renders cards before inserting them) can take
  // focus only once it is attached.
  if (host.isConnected) claimFocus();
  else queueMicrotask(claimFocus);
  return () => form.remove();
}
