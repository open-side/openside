// Wording owned by the client, never by the model or the page: standard secure-field labels and
// autocomplete tokens (a PAN cannot be relabelled as a harmless field), status words, and the
// text fallback used for accessibility and export.
import { ApiError } from "../api/client";
import type { Block, Card, CardStatus, InputField } from "../api/types";
import { formatMoney, formatRating, valueText } from "./values";

export const CARD_LABEL = {
  name: "Name on card",
  number: "Card number",
  expiry: "Expiry",
  cvc: "CVC",
  postal_code: "Postal code",
} as const;

export const CARD_AUTOCOMPLETE = {
  name: "cc-name",
  number: "cc-number",
  expiry: "cc-exp",
  cvc: "cc-csc",
  postal_code: "postal-code",
} as const;

export const STATUS_LABEL: Record<CardStatus, string> = {
  pending: "Waiting",
  done: "Done",
  failed: "Didn’t go through",
  expired: "Expired",
};

export const SECURE_NOTE = "Handled by";
export const SECRET_PLACEHOLDER = "[secure input]";
export const AUTHENTICATOR_SUMMARY = "Save this site’s authenticator";
export const UNHANDLED_ACTION = "This action isn’t available here. Open openside to continue.";
export const GENERIC_FAILURE = "Request failed";
export const SAVE_REFUSED = "Saving to the vault needs approval rights on this device. Send it again to continue without saving.";
export const KEY_REFUSED = "Saving an authenticator needs approval rights on this device. You can still fill this one code.";
export const BAD_SETUP_KEY = "That is not a setup key or otpauth:// link.";

export function fieldLabel(field: InputField): string {
  return field.card ? CARD_LABEL[field.card] : field.label;
}

export function fieldAutocomplete(field: InputField): string {
  if (field.card) return CARD_AUTOCOMPLETE[field.card];
  if (field.otp) return "one-time-code";
  return field.autocomplete || "off";
}

export function requiredMessage(field: InputField): string {
  return `Enter your ${fieldLabel(field).toLowerCase()}.`;
}

/** Errors reach the person as sanitized sentences; a changed page gets the way forward. */
export function failureMessage(cause: unknown): string {
  if (cause instanceof ApiError) {
    if (cause.code === "target_unavailable" || cause.code === "origin_mismatch")
      return "The page changed before your details could be filled. Ask the agent to open it again.";
    return cause.message;
  }
  return cause instanceof Error && cause.message ? cause.message : GENERIC_FAILURE;
}

function blockText(block: Block): string[] {
  switch (block.type) {
    case "kv_rows":
      return [...block.rows, ...(block.total ? [block.total] : [])].map((row) => `${row.label}: ${valueText(row.value)}`);
    case "list_items":
      return block.items.map((item) =>
        [item.quantity ? `${item.quantity}×` : null, item.name, item.price ? formatMoney(item.price) : null, item.rating ? formatRating(item.rating) : null, ...item.facts]
          .filter(Boolean)
          .join(" · "),
      );
    case "table":
      return [block.columns.map((column) => column.label).join(" | "), ...block.rows.map((row) => row.map(valueText).join(" | "))];
    case "choice_options":
      return block.options.map((option) => `- ${option.label}${option.description ? ` (${option.description})` : ""}`);
    case "input_fields":
      return block.fields.map((field) => `${fieldLabel(field)}: ${field.secret || field.otp || field.card ? SECRET_PLACEHOLDER : "(input)"}`);
    case "notice":
      return [block.text];
    case "media":
      return [block.caption ? `${block.alt} — ${block.caption}` : block.alt];
    case "people":
      return [`${block.from ? `${block.from.name} → ` : ""}${block.to.name}${block.to.role ? ` (${block.to.role})` : ""}`];
  }
}

/** Plain-text equivalent of a card; entered values never appear, secure fields read `[secure input]`. */
export function cardText(card: Card): string {
  const lines = [card.title, ...(card.subtitle ? [card.subtitle] : []), `Status: ${STATUS_LABEL[card.status]}`];
  for (const block of card.blocks) lines.push(...blockText(block));
  return lines.join("\n");
}
