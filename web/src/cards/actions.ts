// The typed action dispatcher. A card's appearance never decides what an action does: each target
// maps to one fixed request, built from the card's own metadata and the values currently typed
// into its fields. Secrets are read once, the inputs are emptied, and only then is the request
// awaited; nothing is kept in app state.
import { ApiError } from "../api/client";
import type { Action, Card, InputField, TicketAnswer } from "../api/types";
import { BAD_SETUP_KEY, KEY_REFUSED, SAVE_REFUSED, requiredMessage } from "./copy";
import { parseAuthenticator, totpCode, type TotpParams } from "./totp";
import { safeHref } from "./values";

export interface CardPort {
  resolveTicket(id: string, value: unknown): Promise<unknown>;
  rejectTicket(id: string, reason?: string): Promise<unknown>;
  ticketDone(id: string): Promise<unknown>;
  faviconBlob?(origin: string): Promise<Blob>;
  artifactBlob?(sessionId: string, path: string): Promise<Blob>;
}

export interface ActionPayload {
  /** Non-secret field values edited in the card, keyed by field id (hire name, …). */
  values?: Record<string, string | boolean>;
}

/** `false` = this host does not own the target; anything else (including a resolved promise) = done. */
export type HostActionHandler = (action: Action, card: Card, payload: ActionPayload) => boolean | void | Promise<boolean | void>;

/** The live inputs of one mounted card, as the dispatcher sees them. */
export interface FieldAccess {
  peek(fieldId: string): string | boolean | undefined;
  /** Empties text inputs (checkboxes keep their state). */
  clear(fieldIds: string[]): void;
  restore(values: Record<string, string | boolean>): void;
  /** The option picked in a radio group block. */
  choice(blockId: string): string | null;
}

/** Field-local buffers that must not outlive collapse, unmount or a terminal state. */
export interface Transient {
  refusedKey: TotpParams | null;
}

export interface ActionEnv {
  card: Card;
  port: CardPort;
  fields: FieldAccess;
  host?: HostActionHandler | undefined;
  /** `false`: this device cannot approve, so saving to the vault is not offered (`undefined`: unknown, try). */
  canApprove?: boolean | undefined;
  transient: Transient;
  openUrl?: (url: string) => void;
}

export type ActionOutcome =
  | { kind: "settled"; outcome: "resolved" | "rejected" | "done" }
  | { kind: "handled" }
  | { kind: "invalid"; message: string; focus?: string }
  | { kind: "unhandled" };

/** A save that was refused before anything was filled; the card hides its save option. */
export class SaveRefused extends Error {
  constructor(
    message: string,
    readonly fieldId: string | null,
  ) {
    super(message);
  }
}

export function fieldsOf(card: Card, blockIds: readonly string[]): InputField[] {
  return card.blocks.flatMap((block) => (block.type === "input_fields" && blockIds.includes(block.id) ? block.fields : []));
}

function requireTicket(card: Card): string {
  if (!card.ticket_id) throw new Error("This card has no request to answer.");
  return card.ticket_id;
}

function stripCode(raw: string): string {
  const compact = raw.replace(/\s/g, "");
  // "123 456" and "123-456" are how codes are written down; letters keep their dashes.
  return /^[\d-]+$/.test(compact) ? compact.replace(/-/g, "") : compact;
}

function text(env: ActionEnv, field: InputField): string {
  const value = env.fields.peek(field.id);
  return typeof value === "string" ? value : "";
}

function missing(env: ActionEnv, fields: readonly InputField[]): InputField | undefined {
  return fields.find((field) => field.required && field.kind !== "checkbox" && !text(env, field).trim());
}

function isForbidden(cause: unknown): boolean {
  return cause instanceof ApiError && cause.status === 403 && cause.code === "forbidden";
}

async function submitFields(answer: Extract<TicketAnswer, { type: "fields" }>, env: ActionEnv, ticketId: string): Promise<ActionOutcome> {
  const fields = fieldsOf(env.card, answer.block_ids);
  const empty = missing(env, fields.filter((field) => field.id !== answer.save_field_id));
  if (empty) return { kind: "invalid", message: requiredMessage(empty), focus: empty.id };

  const values: Record<string, string | boolean> = {};
  for (const field of fields) {
    if (field.id === answer.save_field_id) continue;
    const value = env.fields.peek(field.id);
    values[field.id] = field.kind === "checkbox" ? value === true : typeof value === "string" ? value : "";
  }
  const saving = answer.encoding === "login" && !!answer.save_field_id && env.canApprove !== false && env.fields.peek(answer.save_field_id) === true;
  const textIds = fields.filter((field) => field.kind !== "checkbox" && field.id !== answer.save_field_id).map((field) => field.id);

  let body: unknown;
  if (answer.encoding === "login") body = { values, save: saving };
  else if (answer.encoding === "form") body = { values };
  else {
    const only = fields.find((field) => field.kind !== "checkbox");
    body = only ? values[only.id] : "";
  }
  env.fields.clear(textIds);
  try {
    await env.port.resolveTicket(ticketId, body);
  } catch (cause) {
    // 403 `forbidden` is a refusal to save, decided before the page was touched, so the details
    // go back into their fields and one more press logs in without saving. Any other failure may
    // have reached the page: nothing is restored and nothing is retried.
    if (saving && isForbidden(cause)) {
      env.fields.restore(values);
      throw new SaveRefused(SAVE_REFUSED, answer.save_field_id ?? null);
    }
    throw cause;
  }
  return { kind: "settled", outcome: "resolved" };
}

async function submitAuthenticator(answer: Extract<TicketAnswer, { type: "otp_authenticator" }>, env: ActionEnv, ticketId: string): Promise<ActionOutcome> {
  const key = typeof env.fields.peek(answer.field_id) === "string" ? (env.fields.peek(answer.field_id) as string).trim() : "";
  const keep = env.fields.peek(answer.save_field_id) === true;
  env.fields.clear([answer.field_id]);

  const fillOnce = async (params: TotpParams | null): Promise<ActionOutcome> => {
    if (!params) return { kind: "invalid", message: BAD_SETUP_KEY, focus: answer.field_id };
    // The key is used for one computation in this page and never reaches the daemon.
    await env.port.resolveTicket(ticketId, await totpCode(params));
    return { kind: "settled", outcome: "resolved" };
  };

  if (!key) {
    const stored = env.transient.refusedKey;
    if (!stored) return { kind: "invalid", message: "Paste the setup key or otpauth:// link.", focus: answer.field_id };
    env.transient.refusedKey = null;
    return fillOnce(stored);
  }
  const params = parseAuthenticator(key);
  if (!keep) {
    env.transient.refusedKey = null;
    return fillOnce(params);
  }
  env.transient.refusedKey = params;
  try {
    await env.port.resolveTicket(ticketId, { totp: key });
  } catch (cause) {
    // 403 `forbidden` (no approve scope) and `vault_denied` (vault policy) refuse only the saving;
    // the key stays for the explicit "fill this one code" option and is dropped on any other error.
    if (cause instanceof ApiError && cause.status === 403) {
      throw new SaveRefused(cause.code === "forbidden" ? KEY_REFUSED : `${cause.message} You can still fill this one code.`, answer.save_field_id);
    }
    env.transient.refusedKey = null;
    throw cause;
  }
  env.transient.refusedKey = null;
  return { kind: "settled", outcome: "resolved" };
}

async function resolveAnswer(answer: TicketAnswer, action: Action, env: ActionEnv): Promise<ActionOutcome> {
  const { card, port, fields } = env;
  if (answer.type === "hire") {
    const edited = fieldsOf(card, answer.edited_block_id ? [answer.edited_block_id] : []);
    const values: Record<string, string | boolean> = {};
    for (const field of edited) {
      const value = fields.peek(field.id);
      if (value !== undefined) values[field.id] = value;
    }
    const handled = env.host ? await env.host(action, card, { values }) : false;
    if (handled !== false) return { kind: "settled", outcome: "resolved" };
    if (edited.length > 0) return { kind: "unhandled" };
    await port.resolveTicket(requireTicket(card), answer.decision);
    return { kind: "settled", outcome: "resolved" };
  }

  const ticketId = requireTicket(card);
  switch (answer.type) {
    case "confirm":
      await port.resolveTicket(ticketId, answer.approved);
      return { kind: "settled", outcome: "resolved" };
    case "information": {
      const field = fieldsOf(card, card.blocks.map((block) => block.id)).find((candidate) => candidate.id === answer.field_id);
      const typed = typeof fields.peek(answer.field_id) === "string" ? (fields.peek(answer.field_id) as string).trim() : "";
      if (!typed) return { kind: "invalid", message: field ? requiredMessage(field) : "Enter an answer.", focus: answer.field_id };
      fields.clear([answer.field_id]);
      await port.resolveTicket(ticketId, typed);
      return { kind: "settled", outcome: "resolved" };
    }
    case "selected_choice": {
      const picked = fields.choice(answer.block_id);
      if (!picked) return { kind: "invalid", message: "Choose an option to continue." };
      await port.resolveTicket(ticketId, answer.encoding === "page_option" ? { option: picked } : picked);
      return { kind: "settled", outcome: "resolved" };
    }
    case "fields":
      return submitFields(answer, env, ticketId);
    case "login_account":
      await port.resolveTicket(ticketId, { account: answer.item_id });
      return { kind: "settled", outcome: "resolved" };
    case "login_alternative":
      await port.resolveTicket(ticketId, { alternative: answer.alternative_id });
      return { kind: "settled", outcome: "resolved" };
    case "otp_code": {
      const typed = typeof fields.peek(answer.field_id) === "string" ? stripCode(fields.peek(answer.field_id) as string) : "";
      if (!typed) return { kind: "invalid", message: "Enter the code.", focus: answer.field_id };
      fields.clear([answer.field_id]);
      await port.resolveTicket(ticketId, typed);
      return { kind: "settled", outcome: "resolved" };
    }
    case "otp_authenticator":
      return submitAuthenticator(answer, env, ticketId);
  }
}

export async function dispatchAction(action: Action, env: ActionEnv): Promise<ActionOutcome> {
  const target = action.target;
  switch (target.type) {
    case "ticket_resolve":
      return resolveAnswer(target.answer, action, env);
    case "ticket_reject":
      await env.port.rejectTicket(requireTicket(env.card));
      return { kind: "settled", outcome: "rejected" };
    case "ticket_done":
      await env.port.ticketDone(requireTicket(env.card));
      return { kind: "settled", outcome: "done" };
    case "link": {
      const href = safeHref(target.url);
      if (!href) return { kind: "invalid", message: "That link can’t be opened." };
      // No opener and no referrer: the destination learns nothing about this page or the daemon.
      (env.openUrl ?? ((url: string) => void window.open(url, "_blank", "noopener,noreferrer")))(href);
      return { kind: "handled" };
    }
    default: {
      const handled = env.host ? await env.host(action, env.card, {}) : false;
      return handled === false ? { kind: "unhandled" } : { kind: "handled" };
    }
  }
}
