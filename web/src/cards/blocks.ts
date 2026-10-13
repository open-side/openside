import type { Action, Block, BlockOf, Card, CardValue, ListItem } from "../api/types";
import { element, uid } from "./dom";
import { renderField, type FieldRegistry } from "./fields";
import { glyph, providerGlyph } from "./glyphs";
import type { Media } from "./media";
import { avatar, siteMark, thumb } from "./marks";
import { formatMoney, formatRating, initialOf, isSecretField, linkHost, ratingLabel, safeHref } from "./values";

export interface BlockContext {
  card: Card;
  media: Media;
  registry: FieldRegistry;
  /** Runs an action from inside a block (a choice option or a provider button). */
  run: (action: Action, trigger: HTMLElement) => void;
  onEnter: () => void;
  /** Fields drawn elsewhere (the authenticator disclosure). */
  skipFields: ReadonlySet<string>;
}

function anchor(url: string, label: string, className = "oc-link"): HTMLElement {
  const href = safeHref(url);
  if (!href) return element("span", {}, label);
  const link = element("a", { class: className, href, target: "_blank", rel: "noopener noreferrer" }, label);
  return element("span", { class: "oc-linkwrap" }, link, element("span", { class: "oc-host" }, linkHost(href)));
}

export function renderValue(value: CardValue, ctx: BlockContext): Node {
  switch (value.type) {
    case "text":
      return document.createTextNode(value.text);
    case "money":
      return element("span", { class: "oc-money" }, formatMoney(value.money));
    case "link":
      return anchor(value.link.url, value.link.label);
    case "rating":
      return rating(value.rating);
    case "image":
      return thumb(value.image, value.alt, ctx.media, "sm");
  }
}

function rating(value: { milli_value: number; milli_max: number; count?: number | null }): HTMLElement {
  return element("span", { class: "oc-rating", role: "img", "aria-label": ratingLabel(value) }, element("span", { "aria-hidden": "true" }, `★ ${formatRating(value)}`));
}

function kvRows(block: BlockOf<"kv_rows">, ctx: BlockContext): HTMLElement {
  const list = element("dl", { class: "oc-kv" });
  for (const row of block.rows) list.append(element("div", {}, element("dt", {}, row.label), element("dd", {}, renderValue(row.value, ctx))));
  return list;
}

/** The ledger line pinned under a result body, outside any clipping. */
export function renderTotal(block: BlockOf<"kv_rows">, ctx: BlockContext): HTMLElement | null {
  if (!block.total) return null;
  return element("dl", { class: "oc-kv oc-total" }, element("div", {}, element("dt", {}, block.total.label), element("dd", {}, renderValue(block.total.value, ctx))));
}

function listItem(item: ListItem, size: "sm" | "lg" | null, ctx: BlockContext): HTMLElement {
  const name = item.link ? anchor(item.link.url, item.name, "oc-link oc-item-name") : element("span", { class: "oc-item-name" }, item.name);
  const meta = element("p", { class: "oc-item-meta" });
  if (item.site) meta.append(siteMark(item.site, ctx.media, null, 14), element("span", {}, item.site.name));
  for (const fact of item.facts) meta.append(element("span", {}, fact));
  if (item.rating) meta.append(rating(item.rating));
  const main = element("div", { class: "oc-item-main" }, element("div", { class: "oc-item-title" }, name, ...(item.quantity ? [element("span", { class: "oc-qty" }, `×${item.quantity}`)] : [])));
  if (meta.childElementCount > 0) main.append(meta);

  const row = element("li", { class: "oc-item" });
  if (size) row.append(thumb(item.image, item.image_alt, ctx.media, size));
  row.append(main);
  if (item.price) row.append(element("div", { class: "oc-item-side" }, element("span", { class: "oc-money" }, formatMoney(item.price))));
  return row;
}

function listItems(block: BlockOf<"list_items">, ctx: BlockContext): HTMLElement {
  const withImages = block.items.some((item) => item.image);
  const size = !withImages ? null : ctx.card.kind === "order_summary" ? "sm" : "lg";
  const list = element("ul", { class: "oc-list" });
  for (const item of block.items) list.append(listItem(item, size, ctx));
  return list;
}

function isNumeric(rows: CardValue[][], column: number): boolean {
  return rows.length > 0 && rows.every((row) => row[column]?.type === "money" || row[column]?.type === "rating");
}

function table(block: BlockOf<"table">, ctx: BlockContext): HTMLElement {
  const sites = block.row_sites;
  const head = element("tr", {});
  if (sites) head.append(element("th", { scope: "col" }, element("span", { class: "oc-sr" }, "Source")));
  block.columns.forEach((column, index) => {
    const th = element("th", { scope: "col" }, column.label);
    if (isNumeric(block.rows, index)) th.setAttribute("data-align", "end");
    head.append(th);
  });
  const body = element("tbody", {});
  block.rows.forEach((cells, rowIndex) => {
    const tr = element("tr", {});
    const site = sites?.[rowIndex];
    if (sites) tr.append(element("td", { class: "oc-site-cell" }, ...(site ? [siteMark(site, ctx.media, null, 18)] : [])));
    cells.forEach((cell, index) => {
      const td = element("td", {}, renderValue(cell, ctx));
      if (isNumeric(block.rows, index)) td.setAttribute("data-align", "end");
      tr.append(td);
    });
    body.append(tr);
  });
  const scroller = element("div", { class: "oc-table-scroll", role: "region", tabindex: "0", "aria-label": "Results table" }, element("table", { class: "oc-table", "data-layout": block.layout }, element("thead", {}, head), body));
  return scroller;
}

function choiceOptions(block: BlockOf<"choice_options">, ctx: BlockContext): HTMLElement {
  const root = element("fieldset", { class: "oc-choices", "data-layout": block.layout });
  if (block.label) root.append(element("legend", {}, block.label));
  if (block.layout === "providers") {
    const row = element("div", { class: "oc-providers" });
    for (const option of block.options) {
      if (!option.action) continue;
      const action = option.action;
      const name = providerGlyph(option.provider ?? option.label);
      const button = element("button", { type: "button", class: "oc-provider", "aria-label": action.label, title: option.label }, name ? glyph(name, 22) : initialOf(option.label));
      button.addEventListener("click", () => ctx.run(action, button));
      row.append(button);
    }
    root.append(row);
    return root;
  }
  const group = uid("oc-choice");
  for (const option of block.options) {
    const text = [element("span", { class: "oc-option-label" }, option.label), ...(option.description ? [element("span", { class: "oc-option-note" }, option.description)] : [])];
    if (option.action) {
      const action = option.action;
      const button = element("button", { type: "button", class: "oc-option" }, element("span", { class: "oc-option-text" }, ...text), glyph("chevron", 16));
      button.addEventListener("click", () => ctx.run(action, button));
      root.append(button);
    } else {
      const radio = element("input", { type: "radio", name: group, value: option.id });
      root.append(element("label", { class: "oc-radio" }, radio, element("span", { class: "oc-option-text" }, ...text)));
    }
  }
  ctx.registry.registerChoice(block.id, root);
  return root;
}

function inputFields(block: BlockOf<"input_fields">, ctx: BlockContext): HTMLElement | null {
  const fields = block.fields.filter((field) => !ctx.skipFields.has(field.id));
  if (fields.length === 0) return null;
  const root = element("div", { class: "oc-fields" });
  if (fields.some(isSecretField)) root.setAttribute("data-feedback-redact", "");
  const fieldContext = { registry: ctx.registry, cardOrigin: ctx.card.site.origin ?? null, onEnter: ctx.onEnter };
  for (let index = 0; index < fields.length; index++) {
    const field = fields[index]!;
    const next = fields[index + 1];
    // Expiry and CVC are short and always paired; they share one row.
    if (field.card === "expiry" && next?.card === "cvc") {
      root.append(element("div", { class: "oc-field-pair" }, renderField(field, fieldContext), renderField(next, fieldContext)));
      index++;
    } else root.append(renderField(field, fieldContext));
  }
  return root;
}

const TONE_GLYPH = { neutral: null, info: "info", working: null, success: "check", attention: "alert", danger: "alert" } as const;

export function renderNotice(block: BlockOf<"notice">): HTMLElement {
  const mark = block.tone === "working" ? element("span", { class: "oc-spinner", "aria-hidden": "true" }) : TONE_GLYPH[block.tone] ? glyph(TONE_GLYPH[block.tone]!, 16) : null;
  return element("p", { class: "oc-notice", "data-tone": block.tone, "data-placement": block.placement }, ...(mark ? [mark] : []), element("span", {}, block.text));
}

function media(block: BlockOf<"media">, ctx: BlockContext): HTMLElement {
  const figure = element("figure", { class: "oc-media" }, thumb(block.media, block.alt, ctx.media, "wide"));
  if (block.caption) figure.append(element("figcaption", {}, block.caption));
  return figure;
}

function person(value: NonNullable<BlockOf<"people">["from"]>): HTMLElement {
  return element("div", { class: "oc-person" }, avatar(value), element("span", { class: "oc-person-text" }, element("strong", {}, value.name), ...(value.role ? [element("span", {}, value.role)] : [])));
}

function people(block: BlockOf<"people">): HTMLElement {
  const root = element("div", { class: "oc-people" });
  if (block.from) root.append(person(block.from), element("span", { class: "oc-people-arrow", "aria-hidden": "true" }, glyph("arrow-right", 16)));
  root.append(person(block.to));
  return root;
}

/** One block; bubble notices and kv totals are placed by the card shell, not here. */
export function renderBlock(block: Block, ctx: BlockContext): Node | null {
  switch (block.type) {
    case "kv_rows":
      return block.rows.length > 0 ? kvRows(block, ctx) : null;
    case "list_items":
      return listItems(block, ctx);
    case "table":
      return table(block, ctx);
    case "choice_options":
      return choiceOptions(block, ctx);
    case "input_fields":
      return inputFields(block, ctx);
    case "notice":
      return block.placement === "bubble" ? null : renderNotice(block);
    case "media":
      return media(block, ctx);
    case "people":
      return people(block);
  }
}
