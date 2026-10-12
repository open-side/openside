// Assistant prose for the side panel, rendered the way web/src/components/Markdown.tsx renders it:
// the same unified pipeline react-markdown runs (remark-parse + remark-gfm + remark-rehype), raw
// HTML shown as literal text, unsafe URL schemes emptied, images reduced to their alt text and
// code blocks and tables wrapped in the web app's markup. The panel has no React, so the tree is
// turned into DOM nodes here; nothing is ever assigned through innerHTML.
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { unified } from "unified";
import { imageChip, safeUrl } from "../../web/src/components/markdownPolicy";
import { copyText } from "../../web/src/ui/clipboard";
import { icon } from "./ui";

interface HastNode {
  type: string;
  value?: string;
  tagName?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
}

const processor = unified().use(remarkParse).use(remarkGfm).use(remarkRehype, { allowDangerousHtml: true });

function textOf(node: HastNode): string {
  return node.type === "text" ? node.value ?? "" : (node.children ?? []).map(textOf).join("");
}

function attributeName(property: string): string {
  if (property === "className") return "class";
  if (property === "htmlFor") return "for";
  if (/^(aria|data)[A-Z]/.test(property)) return property.replace(/[A-Z]/g, letter => `-${letter.toLowerCase()}`);
  return property.toLowerCase();
}

function element(node: HastNode): HTMLElement {
  const tag = node.tagName!;
  const created = document.createElement(tag);
  for (const [property, raw] of Object.entries(node.properties ?? {})) {
    if (raw === false || raw === null || raw === undefined) continue;
    let value = raw === true ? "" : Array.isArray(raw) ? raw.join(" ") : String(raw);
    if (property === "href" || property === "src") value = safeUrl(value);
    created.setAttribute(attributeName(property), value);
  }
  return created;
}

function copyButton(text: string): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "icon-button small";
  button.setAttribute("aria-label", "Copy code");
  button.title = "Copy code";
  button.append(icon("copy", 16));
  button.addEventListener("click", () => {
    void copyText(text).then(copied => {
      if (!copied) return;
      button.replaceChildren(icon("check", 16));
      button.setAttribute("aria-label", "Copied");
      window.setTimeout(() => { button.replaceChildren(icon("copy", 16)); button.setAttribute("aria-label", "Copy code"); }, 1500);
    });
  });
  return button;
}

function codeBlock(node: HastNode): HTMLElement {
  const code = node.children?.find(child => child.type === "element" && child.tagName === "code");
  const classes = code?.properties?.className;
  const language = (Array.isArray(classes) ? classes : []).map(String).find(name => name.startsWith("language-"))?.slice("language-".length);
  const block = document.createElement("div");
  block.className = "code-block";
  const head = document.createElement("div");
  head.className = "code-block-head";
  const label = document.createElement("span");
  label.textContent = language ?? "text";
  head.append(label, copyButton(code ? textOf(code).replace(/\n$/, "") : ""));
  const pre = element(node);
  pre.tabIndex = 0;
  for (const child of node.children ?? []) pre.append(...render(child));
  block.append(head, pre);
  return block;
}

function image(node: HastNode): HTMLElement {
  const src = typeof node.properties?.src === "string" ? node.properties.src : undefined;
  const alt = typeof node.properties?.alt === "string" ? node.properties.alt : undefined;
  const { label, href } = imageChip(src, alt, window.location.origin);
  const chip = document.createElement(href ? "a" : "span");
  chip.className = "md-image";
  if (chip instanceof HTMLAnchorElement && href) Object.assign(chip, { href, target: "_blank", rel: "noopener noreferrer", title: href });
  else { chip.setAttribute("role", "img"); chip.setAttribute("aria-label", label); }
  const text = document.createElement("span");
  text.textContent = label;
  chip.append(icon("image", 16), text);
  return chip;
}

function render(node: HastNode): Node[] {
  // allowDangerousHtml keeps raw HTML as `raw` nodes only so it can be shown as literal text, as react-markdown does.
  if (node.type === "text" || node.type === "raw") return [document.createTextNode(node.value ?? "")];
  if (node.type === "root") return (node.children ?? []).flatMap(render);
  if (node.type !== "element" || !node.tagName) return [];
  if (node.tagName === "pre") return [codeBlock(node)];
  if (node.tagName === "img") return [image(node)];
  const created = element(node);
  if (created instanceof HTMLAnchorElement) { created.target = "_blank"; created.rel = "noopener noreferrer"; }
  for (const child of node.children ?? []) created.append(...render(child));
  if (node.tagName !== "table") return [created];
  const scroll = document.createElement("div");
  scroll.className = "table-scroll";
  scroll.append(created);
  return [scroll];
}

/** Replaces `target`'s children with `text` rendered as markdown. */
export function renderMarkdown(target: HTMLElement, text: string): void {
  const tree = processor.runSync(processor.parse(text)) as unknown as HastNode;
  target.replaceChildren(...render(tree));
}
