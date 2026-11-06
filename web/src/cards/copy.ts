// Wording owned by the client, never by the model or the page: standard secure-field labels and
// autocomplete tokens (a PAN cannot be relabelled as a harmless field), status words, and the
// text fallback used for accessibility and export.
import { ApiError } from "../api/client";
import type { Block, Card, CardStatus, InputField } from "../api/types";
import { t } from "../i18n";
import { formatMoney, formatRating, valueText } from "./values";

const CARD_AUTOCOMPLETE = {
  name: "cc-name",
  number: "cc-number",
  expiry: "cc-exp",
  cvc: "cc-csc",
  postal_code: "postal-code",
} as const;

export function statusLabel(status: CardStatus): string {
  return t(`cards.status.${status}`);
}

export function fieldLabel(field: InputField): string {
  return field.card ? t(`cards.field.card.${field.card}`) : field.label;
}

export function fieldAutocomplete(field: InputField): string {
  if (field.card) return CARD_AUTOCOMPLETE[field.card];
  if (field.otp) return "one-time-code";
  return field.autocomplete || "off";
}

export function requiredMessage(field: InputField): string {
  return t("cards.field.required", { label: fieldLabel(field).toLowerCase() });
}

/** Errors reach the person as sanitized sentences; a changed page gets the way forward. */
export function failureMessage(cause: unknown): string {
  if (cause instanceof ApiError) {
    if (cause.code === "target_unavailable" || cause.code === "origin_mismatch") return t("cards.error.pageChanged");
    return cause.message;
  }
  return cause instanceof Error && cause.message ? cause.message : t("cards.error.generic");
}

/** "Expires in 4:05", "Expires in 2 h 30 min"; `short` drops the lead-in for the collapsed bubble. */
export function expiresIn(ms: number, short = false): string {
  if (ms <= 0) return short ? "0:00" : t("cards.expiry.timeUp");
  const total = Math.ceil(ms / 1000);
  let left: string;
  if (total < 3600) left = `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
  else {
    const hours = Math.floor(total / 3600);
    const minutes = Math.floor((total % 3600) / 60);
    left = minutes ? t("cards.expiry.hoursMinutes", { hours, minutes }) : t("cards.expiry.hours", { hours });
  }
  return short ? left : t("cards.expiry.in", { time: left });
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
      return block.fields.map((field) => `${fieldLabel(field)}: ${field.secret || field.otp || field.card ? t("cards.text.secureInput") : t("cards.text.input")}`);
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
  const lines = [card.title, ...(card.subtitle ? [card.subtitle] : []), t("cards.text.status", { status: statusLabel(card.status) })];
  for (const block of card.blocks) lines.push(...blockText(block));
  return lines.join("\n");
}
