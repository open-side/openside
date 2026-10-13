import type { InputField } from "../api/types";
import { attachSuggestions } from "../ui/suggest";
import { fieldAutocomplete, fieldLabel } from "./copy";
import { element, uid } from "./dom";
import { glyph } from "./glyphs";
import type { FieldAccess } from "./actions";

type Control = HTMLInputElement | HTMLTextAreaElement;

/** Live inputs of one mounted card. Values exist only in these elements; the registry holds no copies. */
export class FieldRegistry implements FieldAccess {
  private readonly controls = new Map<string, { field: InputField; control: Control }>();
  private readonly groups = new Map<string, HTMLElement>();

  register(field: InputField, control: Control): void {
    this.controls.set(field.id, { field, control });
  }

  registerChoice(blockId: string, group: HTMLElement): void {
    this.groups.set(blockId, group);
  }

  peek(fieldId: string): string | boolean | undefined {
    const entry = this.controls.get(fieldId);
    if (!entry) return undefined;
    return entry.control instanceof HTMLInputElement && entry.control.type === "checkbox" ? entry.control.checked : entry.control.value;
  }

  clear(fieldIds: string[]): void {
    for (const id of fieldIds) {
      const control = this.controls.get(id)?.control;
      if (control && !(control instanceof HTMLInputElement && control.type === "checkbox")) control.value = "";
    }
  }

  /** Empties every text input; used on collapse, Escape, a terminal state and unmount. */
  clearAll(): void {
    this.clear([...this.controls.keys()]);
  }

  restore(values: Record<string, string | boolean>): void {
    for (const [id, value] of Object.entries(values)) {
      const control = this.controls.get(id)?.control;
      if (control && typeof value === "string") control.value = value;
    }
  }

  choice(blockId: string): string | null {
    return this.groups.get(blockId)?.querySelector<HTMLInputElement>("input[type=radio]:checked")?.value ?? null;
  }

  focus(fieldId: string): void {
    this.controls.get(fieldId)?.control.focus();
  }

  /** Called after the body is torn down; the elements are about to be dropped. */
  reset(): void {
    this.clearAll();
    this.controls.clear();
    this.groups.clear();
  }

  /** The first field that has a control, in DOM order. */
  firstControl(): Control | null {
    for (const { control } of this.controls.values()) if (control.isConnected) return control;
    return null;
  }
}

function digits(value: string, max: number): string {
  return value.replace(/\D/g, "").slice(0, max);
}

function formatNumber(value: string): string {
  return digits(value, 19).replace(/(\d{4})(?=\d)/g, "$1 ");
}

function formatExpiry(value: string): string {
  const typed = digits(value, 4);
  // A first digit above 1 can only be a month with a leading zero.
  const padded = typed.length === 1 && typed > "1" ? `0${typed}` : typed;
  return padded.length > 2 ? `${padded.slice(0, 2)}/${padded.slice(2)}` : padded;
}

const FORMATTERS = {
  number: formatNumber,
  expiry: formatExpiry,
  cvc: (value: string) => digits(value, 4),
} as const;

const INPUT_MODE = { number: "numeric", expiry: "numeric", cvc: "numeric", postal_code: "text", name: "text" } as const;

export interface FieldContext {
  registry: FieldRegistry;
  /** Origin the card is verified for; a field aimed elsewhere says so. */
  cardOrigin: string | null;
  onEnter?: () => void;
}

function fieldName(field: InputField, autocomplete: string): string {
  if (autocomplete === "current-password") return "password";
  return autocomplete === "off" ? `field-${field.id}` : autocomplete;
}

function controlFor(field: InputField, id: string, ctx: FieldContext): Control {
  const autocomplete = fieldAutocomplete(field);
  if (field.kind === "textarea") {
    const area = element("textarea", { id, rows: "3", name: fieldName(field, autocomplete) });
    area.autocomplete = "off";
    area.setAttribute("autocapitalize", "off");
    area.spellcheck = false;
    return area;
  }
  const type = field.kind === "password" ? "password" : field.kind === "email" ? "email" : field.kind === "tel" ? "tel" : "text";
  const input = element("input", { id, type, name: fieldName(field, autocomplete), autocomplete });
  input.setAttribute("autocapitalize", "off");
  input.setAttribute("autocorrect", "off");
  input.spellcheck = false;
  if (field.placeholder) input.placeholder = field.placeholder;
  if (field.card) {
    input.inputMode = INPUT_MODE[field.card];
    if (field.card === "number" || field.card === "expiry" || field.card === "cvc") {
      const format = FORMATTERS[field.card];
      input.addEventListener("input", () => {
        const formatted = format(input.value);
        if (formatted !== input.value) input.value = formatted;
      });
    }
  } else if (field.max_length > 0) input.maxLength = field.max_length;
  if (field.kind === "tel") input.inputMode = "tel";
  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.isComposing) {
      event.preventDefault();
      ctx.onEnter?.();
    }
  });
  return input;
}

/** One labelled input. Secrets and codes are never pre-filled, whatever the card says. */
export function renderField(field: InputField, ctx: FieldContext): HTMLElement {
  const id = uid("oc-field");
  if (field.kind === "checkbox") {
    const box = element("input", { id, type: "checkbox", name: `field-${field.id}` });
    box.checked = field.initial === true;
    ctx.registry.register(field, box);
    return element("label", { class: "oc-check", for: id }, box, element("span", {}, field.label));
  }

  const control = controlFor(field, id, ctx);
  control.setAttribute("aria-required", String(field.required));
  // Passwords, codes and card data are never pre-filled or suggested; a secret identifier (an
  // email) may carry the daemon's saved suggestions but is still redacted from feedback capture.
  const neverFilled = field.otp || !!field.card || field.kind === "password";
  if (!neverFilled && typeof field.initial === "string") control.value = field.initial;
  if (field.secret || neverFilled) control.setAttribute("data-feedback-redact", "");
  ctx.registry.register(field, control);

  const wrap = element("div", { class: "oc-control" }, control);
  if (field.kind === "password" && !field.otp) {
    const eye = element("button", { type: "button", class: "oc-eye", "aria-label": "Show password", "aria-pressed": "false" }, glyph("eye", 18));
    eye.addEventListener("click", () => {
      const shown = (control as HTMLInputElement).type === "text";
      (control as HTMLInputElement).type = shown ? "password" : "text";
      eye.setAttribute("aria-pressed", String(!shown));
      eye.setAttribute("aria-label", shown ? "Show password" : "Hide password");
      eye.replaceChildren(glyph(shown ? "eye" : "eye-off", 18));
    });
    wrap.append(eye);
  }

  const root = element("div", { class: "oc-field", "data-card-field": field.card ?? "", "data-otp": String(field.otp) }, element("label", { for: id }, fieldLabel(field)), wrap);
  if (field.target_origin && field.target_origin !== ctx.cardOrigin) {
    const hintId = `${id}-to`;
    root.append(element("p", { class: "oc-hint", id: hintId }, `Sent to ${field.target_origin}`));
    control.setAttribute("aria-describedby", hintId);
  }
  if (!neverFilled && field.suggestions.length > 0) attachSuggestions(control, field.suggestions);
  return root;
}
