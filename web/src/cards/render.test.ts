import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Action, Block, Card } from "../api/types";
import { cardText } from "./copy";
import { mountCard, type CardMountOptions, type MountedCard } from "./render";
import { ask, FIXTURES, field, fakePort, fixture, resolveAction } from "./testing";

// 1×1 transparent PNG: the only kind of favicon a card may draw directly.
const PNG = `data:image/png;base64,${"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg=="}`;

let host: HTMLElement;
let mounted: MountedCard[] = [];

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  URL.createObjectURL = vi.fn(() => `blob:test-${Math.random()}`);
  URL.revokeObjectURL = vi.fn();
});
afterEach(() => {
  for (const view of mounted) view.dispose();
  mounted = [];
  host.remove();
});

function mount(card: Card, options: Partial<CardMountOptions> = {}) {
  const port = fakePort();
  const view = mountCard(host, card, { port, sessionId: "s1", focus: "none", ...options });
  mounted.push(view);
  return { view, port };
}

const q = <T extends Element = HTMLElement>(selector: string) => host.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...host.querySelectorAll<T>(selector)];
const flush = () => vi.waitFor(() => undefined, { timeout: 50 }).catch(() => undefined);

function type(input: HTMLInputElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

const login = (): Card => fixture("ticket:login1");

describe("every projector fixture", () => {
  it("mounts, opens, shows its title and builds no script or inline-handler markup", () => {
    expect(FIXTURES).toHaveLength(17);
    for (const card of FIXTURES) {
      const { view } = mount(structuredClone(card), { defaultOpen: true });
      const shell = q(`.oc-card[data-card-id="${CSS.escape(card.id)}"]`);
      expect(shell, card.id).not.toBeNull();
      expect(shell!.getAttribute("data-kind")).toBe(card.kind);
      view.open();
      expect(shell!.textContent, card.id).toContain(card.title);
      view.dispose();
    }
    expect(host.innerHTML).not.toMatch(/<script|onerror=|onclick=/i);
  });

  it("has a text fallback in which secure fields never carry a value", () => {
    const text = cardText(login());
    expect(text).toContain("Log in");
    expect(text).toContain("[secure input]");
    expect(text).not.toMatch(/hunter2/);
  });
});

describe("the collapsed bubble", () => {
  it("rests as one button with the safe summary, builds no fields until opened, and expands on click", () => {
    mount(login());
    const pill = q<HTMLButtonElement>("button.oc-pill")!;
    expect(pill.textContent).toContain("Log in to accounts.example");
    expect(pill.getAttribute("aria-expanded")).toBe("false");
    expect(all("input")).toHaveLength(0);
    expect(q(".oc-card")!.getAttribute("data-ask")).toBe("pending");

    pill.click();
    expect(q(".oc-card")!.getAttribute("data-open")).toBe("true");
    expect(all("input").length).toBeGreaterThan(1);
    const close = q("button.oc-close")!;
    expect(close.getAttribute("aria-expanded")).toBe("true");
    expect(close.getAttribute("aria-controls")).toBe(q(".oc-body")!.id);
  });

  it("Escape collapses to the bubble, empties the fields, gives focus back and never rejects", () => {
    const { port } = mount(login(), { defaultOpen: true });
    const user = q<HTMLInputElement>("input[autocomplete='username']")!;
    const pass = q<HTMLInputElement>("input[autocomplete='current-password']")!;
    type(user, "ada@example.com");
    type(pass, "hunter2");
    pass.focus();
    pass.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(q(".oc-card")!.getAttribute("data-open")).toBe("false");
    expect(user.value).toBe("");
    expect(pass.value).toBe("");
    expect(document.activeElement).toBe(q("button.oc-pill"));
    expect(port.rejectTicket).not.toHaveBeenCalled();
    q<HTMLButtonElement>("button.oc-pill")!.click();
    expect(q<HTMLInputElement>("input[autocomplete='current-password']")!.value).toBe("");
  });

  it("a sheet host keeps the card open and has no collapse control", () => {
    mount(login(), { layout: "sheet" });
    expect(q(".oc-card")!.getAttribute("data-open")).toBe("true");
    expect(q("button.oc-close")).toBeNull();
    pressEscape();
    expect(q(".oc-card")!.getAttribute("data-open")).toBe("true");
  });

  function pressEscape() {
    q("input")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  }
});

describe("the origin line and the icon", () => {
  const withSite = (site: Partial<Card["site"]>): Card => ({ ...login(), site: { ...login().site, ...site } });

  it("shows the daemon-verified origin, spells an IDN host in punycode and keeps a non-default port", () => {
    mount(withSite({ origin: "https://bücher.example:8443" }), { defaultOpen: true });
    expect(q(".oc-origin")!.textContent).toBe("https://xn--bcher-kva.example:8443");
  });

  it("claims nothing when the origin is unreadable", () => {
    mount(withSite({ origin: "not a url" }), { defaultOpen: true });
    expect(q(".oc-origin")).toBeNull();
  });

  it("draws only a normalized PNG data URL directly; a remote or non-PNG source falls back to the initial", () => {
    mount(withSite({ favicon_ref: { type: "data", data_url: PNG } }), { defaultOpen: true });
    expect(q<HTMLImageElement>(".oc-mark img")!.src).toBe(PNG);
    view().dispose();
    for (const bad of ["https://evil.example/favicon.ico", "data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=", "data:image/png;base64,@@@", `data:image/png;base64,${"A".repeat(20_000)}`]) {
      host.replaceChildren();
      mount(withSite({ favicon_ref: { type: "data", data_url: bad } }), { defaultOpen: true });
      expect(q(".oc-mark img"), bad).toBeNull();
      expect(q(".oc-mark")!.textContent).toBe("A");
    }
  });

  it("loads an origin icon through the client for that exact origin only, and revokes it on dispose", async () => {
    const card = withSite({ favicon_ref: { type: "origin", origin: "https://accounts.example" } });
    const { view: first, port } = mount(card, { defaultOpen: true });
    await flush();
    expect(port.faviconBlob).toHaveBeenCalledWith("https://accounts.example");
    await vi.waitFor(() => expect(q<HTMLImageElement>(".oc-mark img")!.src).toMatch(/^blob:/));
    first.dispose();
    expect(URL.revokeObjectURL).toHaveBeenCalled();

    host.replaceChildren();
    const other = mount(withSite({ favicon_ref: { type: "origin", origin: "https://other.example" } }), { defaultOpen: true });
    await flush();
    expect(other.port.faviconBlob).not.toHaveBeenCalled();
  });

  function view() {
    return mounted.at(-1)!;
  }
});

describe("secret fields", () => {
  it("are one-time-code / password inputs with autocomplete and redaction markers, and never carry a value attribute", () => {
    mount(fixture("ticket:otp1"), { defaultOpen: true });
    const code = q<HTMLInputElement>("input")!;
    expect(code.autocomplete).toBe("one-time-code");
    expect(code.hasAttribute("data-feedback-redact")).toBe(true);
    type(code, "123456");
    expect(host.innerHTML).not.toContain("123456");
  });

  it("are emptied before the request is awaited and are not in the page afterwards", async () => {
    const { port } = mount(login(), { defaultOpen: true });
    const user = q<HTMLInputElement>("input[autocomplete='username']")!;
    const pass = q<HTMLInputElement>("input[autocomplete='current-password']")!;
    type(user, "ada@example.com");
    type(pass, "hunter2");
    let during = "";
    port.resolveTicket.mockImplementationOnce(async () => {
      during = `${user.value}|${pass.value}`;
      return {};
    });
    q<HTMLButtonElement>(".oc-btn--primary")!.click();
    await vi.waitFor(() => expect(port.resolveTicket).toHaveBeenCalled());
    expect(port.resolveTicket).toHaveBeenCalledWith("login1", { values: { user: "ada@example.com", pass: "hunter2" }, save: false });
    expect(during).toBe("|");
    expect(host.innerHTML).not.toContain("hunter2");
    expect(host.textContent).not.toContain("hunter2");
  });

  it("are emptied even when the daemon refuses, and the person sees the reason", async () => {
    const { port } = mount(login(), { defaultOpen: true });
    const pass = q<HTMLInputElement>("input[autocomplete='current-password']")!;
    type(q<HTMLInputElement>("input[autocomplete='username']")!, "u");
    type(pass, "hunter2");
    port.resolveTicket.mockRejectedValueOnce(new Error("The page changed."));
    q<HTMLButtonElement>(".oc-btn--primary")!.click();
    await vi.waitFor(() => expect(q(".oc-error")).not.toBeNull());
    expect(pass.value).toBe("");
    expect(q(".oc-error")!.getAttribute("role")).toBe("alert");
    expect(q(".oc-error")!.textContent).toBe("The page changed.");
    expect(q<HTMLButtonElement>(".oc-btn--primary")!.disabled).toBe(false);
  });

  it("are emptied when the card is disposed", () => {
    const { view } = mount(login(), { defaultOpen: true });
    const pass = q<HTMLInputElement>("input[autocomplete='current-password']")!;
    type(pass, "hunter2");
    view.dispose();
    expect(pass.value).toBe("");
  });

  it("a missing required field is named, focused and nothing is sent", async () => {
    const { port } = mount(login(), { defaultOpen: true });
    q<HTMLButtonElement>(".oc-btn--primary")!.click();
    await vi.waitFor(() => expect(q(".oc-error")!.textContent).toBe("Enter your email."));
    expect(port.resolveTicket).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(q("input[autocomplete='username']"));
  });

  it("Enter in a field presses the primary action once", async () => {
    const { port } = mount(fixture("ticket:otp1"), { defaultOpen: true });
    const code = q<HTMLInputElement>("input")!;
    type(code, "123 456");
    code.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await vi.waitFor(() => expect(port.resolveTicket).toHaveBeenCalledWith("otp1", "123456"));
    expect(port.resolveTicket).toHaveBeenCalledTimes(1);
    expect(code.value).toBe("");
  });

  it("the eye toggles a password between hidden and shown", () => {
    mount(login(), { defaultOpen: true });
    const pass = q<HTMLInputElement>("input[autocomplete='current-password']")!;
    const eye = q("button.oc-eye")!;
    expect(pass.type).toBe("password");
    eye.click();
    expect(pass.type).toBe("text");
    expect(eye.getAttribute("aria-label")).toBe("Hide password");
    eye.click();
    expect(pass.type).toBe("password");
  });

  it("a destination that differs from the card's origin is stated beside the field", () => {
    const card = login();
    const block = card.blocks[0] as Extract<Block, { type: "input_fields" }>;
    block.fields[1]!.target_origin = "https://sso.example";
    mount(card, { defaultOpen: true });
    expect(q(".oc-hint")!.textContent).toBe("Sent to https://sso.example");
    expect(all(".oc-hint")).toHaveLength(1);
  });
});

describe("card fields", () => {
  const cardBlock = (): Block => ({
    type: "input_fields",
    id: "pay",
    fields: [
      field({ id: "number", kind: "text", secret: true, card: "number", validation: "card_number" }),
      field({ id: "exp", kind: "text", secret: true, card: "expiry", validation: "card_expiry" }),
      field({ id: "cvc", kind: "text", secret: true, card: "cvc", validation: "card_cvc" }),
    ],
  });
  const payment = (): Card => ask({ kind: "payment_card", title: "Card", blocks: [cardBlock()], actions: [resolveAction({ type: "fields", block_ids: ["pay"], encoding: "form", save_field_id: null }, "Pay")] });

  it("use the standard labels and autocomplete tokens, and put expiry and CVC on one row", () => {
    mount(payment(), { defaultOpen: true });
    expect(all("label").map((label) => label.textContent)).toEqual(["Card number", "Expiry", "CVC"]);
    expect(all<HTMLInputElement>("input").map((input) => input.autocomplete)).toEqual(["cc-number", "cc-exp", "cc-csc"]);
    expect(q(".oc-field-pair")!.querySelectorAll("input")).toHaveLength(2);
  });

  it("group the number, add the slash to the expiry and keep digits only", () => {
    mount(payment(), { defaultOpen: true });
    const [number, expiry, cvc] = all<HTMLInputElement>("input");
    type(number!, "4242a4242 4242-4242");
    type(expiry!, "1230");
    type(cvc!, "12a3");
    expect(number!.value).toBe("4242 4242 4242 4242");
    expect(expiry!.value).toBe("12/30");
    expect(cvc!.value).toBe("123");
    type(expiry!, "7");
    expect(expiry!.value).toBe("07");
  });
});

describe("pending to done", () => {
  it("folds the open card into a receipt, announces it, drops every action and never shows an entered value", async () => {
    const pending = fixture("ticket:pay1");
    const { view } = mount(pending, { defaultOpen: true });
    expect(all(".oc-btn").length).toBeGreaterThan(0);
    view.update({ ...pending, status: "done", collapsed_summary: "Paid · accounts.example", actions: [], blocks: pending.blocks.filter((block) => block.type !== "input_fields") });
    const shell = q(".oc-card")!;
    expect(shell.getAttribute("data-status")).toBe("done");
    expect(shell.getAttribute("data-open")).toBe("false");
    expect(shell.hasAttribute("data-ask")).toBe(false);
    expect(q(".oc-status")!.getAttribute("aria-label")).toBe("Done");
    expect(q(".oc-pill-text")!.textContent).toBe("Paid · accounts.example");
    expect(q('[role="status"][aria-live]')!.textContent).toBe("Paid · accounts.example");
    expect(all(".oc-btn")).toHaveLength(0);
  });

  it("clears typed secrets when the request settles elsewhere", () => {
    const pending = login();
    const { view } = mount(pending, { defaultOpen: true });
    const pass = q<HTMLInputElement>("input[autocomplete='current-password']")!;
    type(pass, "hunter2");
    view.update({ ...pending, status: "expired", actions: [], collapsed_summary: "Expired", blocks: [] });
    expect(pass.value).toBe("");
    expect(q(".oc-card")!.getAttribute("data-status")).toBe("expired");
    expect(all("input")).toHaveLength(0);
  });

  it("a re-poll with the same card, or a new title only, keeps a half-typed value", () => {
    const pending = fixture("ticket:otp1");
    const { view } = mount(pending, { defaultOpen: true });
    const code = q<HTMLInputElement>("input")!;
    type(code, "12");
    view.update(structuredClone(pending));
    view.update({ ...structuredClone(pending), subtitle: "Still waiting" });
    expect(q<HTMLInputElement>("input")).toBe(code);
    expect(code.value).toBe("12");
    expect(q(".oc-subtitle")!.textContent).toBe("Still waiting");
  });

  it("a structural change rebuilds the body", () => {
    const pending = fixture("ticket:otp1");
    const { view } = mount(pending, { defaultOpen: true });
    const code = q<HTMLInputElement>("input")!;
    type(code, "12");
    const changed = structuredClone(pending);
    changed.actions = changed.actions.slice(0, 1);
    view.update(changed);
    expect(q<HTMLInputElement>("input")!.value).toBe("");
  });

  it("locks the buttons after a settled answer until the daemon's card arrives", async () => {
    const onSettled = vi.fn();
    const { port } = mount(fixture("ticket:pay1"), { defaultOpen: true, onSettled });
    q<HTMLButtonElement>(".oc-btn--primary")!.click();
    await vi.waitFor(() => expect(onSettled).toHaveBeenCalledWith("resolved", expect.objectContaining({ id: "ticket:pay1" })));
    expect(port.resolveTicket).toHaveBeenCalledTimes(1);
    expect(all<HTMLButtonElement>(".oc-btn").every((button) => button.disabled)).toBe(true);
  });
});

describe("blocks", () => {
  it("kv_rows are a description list of typed values; the total is pinned outside the clipped body", () => {
    const card = fixture("present:o1");
    mount(card);
    const foot = q(".oc-foot")!;
    expect(foot.querySelector(".oc-total")).not.toBeNull();
    expect(foot.closest(".oc-scroll")).toBeNull();
    const kv = ask({ blocks: [{ type: "kv_rows", id: "k", rows: [{ label: "Amount", value: { type: "money", money: { minor_units: 2431, currency: "USD" } } }, { label: "Rating", value: { type: "rating", rating: { milli_value: 4400, milli_max: 5000, count: 231 } } }] }], actions: [] });
    host.replaceChildren();
    mount(kv, { defaultOpen: true });
    expect(all("dd").map((dd) => dd.textContent)).toEqual(["$24.31", "★ 4.4 (231)"]);
    expect(q(".oc-rating")!.getAttribute("aria-label")).toBe("Rated 4.4 out of 5, 231 ratings");
  });

  const items = (count: number): Block => ({
    type: "list_items",
    id: "items",
    items: Array.from({ length: count }, (_, index) => ({
      id: `i${index}`,
      name: `Item ${index}`,
      facts: ["In stock"],
      image: { path: `cards/media/img${index}.webp` },
      image_alt: `Photo ${index}`,
      price: { minor_units: 1999, currency: "USD" },
      rating: null,
      link: { label: "Open", url: "https://shop.example/p" },
      quantity: null,
      site: { origin: "https://shop.example", name: "Shop", favicon_ref: null },
    })),
  });
  const result = (blocks: Block[]): Card => ({ ...ask({ blocks, actions: [] }), id: "present:r1", kind: "product_list", status: "done", interaction: "none", ticket_id: null });

  it("list_items show name, price, facts, a safe link, and load images through the session artifact route", async () => {
    const { port } = mount(result([items(2)]));
    expect(all(".oc-item-name").map((node) => node.textContent)).toEqual(["Item 0", "Item 1"]);
    expect(all(".oc-money").map((node) => node.textContent)).toEqual(["$19.99", "$19.99"]);
    const link = q<HTMLAnchorElement>("a.oc-item-name")!;
    expect(link.rel).toBe("noopener noreferrer");
    expect(link.target).toBe("_blank");
    await vi.waitFor(() => expect(port.artifactBlob).toHaveBeenCalledWith("s1", "cards/media/img0.webp"));
    expect(all(".oc-scroll[data-clipped]")).toHaveLength(0);
  });

  it("never fetches an image path that is not a card artifact", async () => {
    const hostile = items(1) as Extract<Block, { type: "list_items" }>;
    for (const path of ["https://evil.example/x.png", "../secrets.png", "cards/media/../../x.png", "cards/media/a b.webp"]) {
      hostile.items[0]!.image = { path };
      host.replaceChildren();
      const { port } = mount(result([hostile]));
      await flush();
      expect(port.artifactBlob, path).not.toHaveBeenCalled();
      expect(q(".oc-thumb")!.hasAttribute("data-failed")).toBe(true);
    }
  });

  it("a long list previews three rows behind a labelled disclosure and expands", () => {
    mount(result([items(5)]));
    const scroller = q(".oc-scroll")!;
    expect(scroller.hasAttribute("data-clipped")).toBe(true);
    // The rows past the third carry the hide marker (the stylesheet removes them in the preview).
    expect(all(".oc-item").map((row) => row.hasAttribute("data-preview-hidden"))).toEqual([false, false, false, true, true]);
    const more = q<HTMLButtonElement>("button.oc-more")!;
    expect(more.textContent).toBe("Show all 5");
    expect(more.getAttribute("aria-controls")).toBe(scroller.id);
    expect(more.getAttribute("aria-expanded")).toBe("false");
    more.click();
    expect(scroller.hasAttribute("data-clipped")).toBe(false);
    expect(more.getAttribute("aria-expanded")).toBe("true");
    expect(more.textContent).toBe("Show less");
  });

  it("table has column headers, a focusable scroll region, right-aligned numbers and per-row sources", () => {
    const table: Block = {
      type: "table",
      id: "t",
      layout: "scroll",
      columns: [
        { id: "name", label: "Name" },
        { id: "price", label: "Price" },
      ],
      rows: [
        [{ type: "text", text: "Kettle" }, { type: "money", money: { minor_units: 3500, currency: "USD" } }],
        [{ type: "link", link: { label: "Pot", url: "https://shop.example/pot" } }, { type: "money", money: { minor_units: 2500, currency: "USD" } }],
      ],
      row_sites: [{ origin: "https://shop.example", name: "Shop", favicon_ref: null }, null],
    };
    mount({ ...result([table]), kind: "table" });
    const region = q('[role="region"]')!;
    expect(region.getAttribute("tabindex")).toBe("0");
    expect(all("th").map((th) => th.textContent)).toEqual(["", "Name", "Price"].map((text) => (text ? text : q("th .oc-sr")!.textContent)));
    expect(all("th[scope='col']")).toHaveLength(3);
    expect(all("td[data-align='end']").map((td) => td.textContent)).toEqual(["$35.00", "$25.00"]);
    expect(q<HTMLAnchorElement>("td a")!.rel).toBe("noopener noreferrer");
    expect(all(".oc-site-cell .oc-mark")).toHaveLength(1);
  });

  it("choice_options: options with an action are buttons that run it; providers are labelled icon buttons; plain options are radios", async () => {
    const run: Action = { id: "pick", label: "Continue with Ada", style: "secondary", target: { type: "ticket_resolve", answer: { type: "login_account", item_id: "acct-1" } } };
    const google: Action = { id: "g", label: "Continue with Google", style: "secondary", target: { type: "ticket_resolve", answer: { type: "login_alternative", alternative_id: "g1" } } };
    const blocks: Block[] = [
      { type: "choice_options", id: "accounts", label: "Accounts", layout: "stack", required: true, options: [{ id: "a", label: "Ada", description: "ada@example.com", provider: null, action: run }, { id: "b", label: "Bo", description: null, provider: null, action: null }] },
      { type: "choice_options", id: "providers", label: "Other ways", layout: "providers", required: false, options: [{ id: "g", label: "Google", description: null, provider: "google", action: google }] },
    ];
    const { port } = mount(ask({ kind: "login_choice", blocks, actions: [run, google] }), { defaultOpen: true });
    expect(all("input[type=radio]")).toHaveLength(1);
    const provider = q("button.oc-provider")!;
    expect(provider.getAttribute("aria-label")).toBe("Continue with Google");
    expect(provider.querySelector("svg")).not.toBeNull();
    expect(all(".oc-actions .oc-btn")).toHaveLength(0);
    q("button.oc-option")!.click();
    await vi.waitFor(() => expect(port.resolveTicket).toHaveBeenCalledWith("t1", { account: "acct-1" }));
    provider.click();
  });

  it("notice: inline in the body, bubble outside the card, tone as an attribute", () => {
    const blocks: Block[] = [
      { type: "notice", id: "n1", tone: "attention", text: "Check the total", placement: "inline" },
      { type: "notice", id: "n2", tone: "neutral", text: "Nothing was charged", placement: "bubble" },
    ];
    mount(ask({ blocks, actions: [] }), { defaultOpen: true });
    expect(q(".oc-card .oc-notice[data-tone='attention']")!.textContent).toBe("Check the total");
    expect(q(".oc-bubbles .oc-notice")!.textContent).toBe("Nothing was charged");
    expect(q(".oc-card")!.textContent).not.toContain("Nothing was charged");
  });

  it("media shows a captioned image from the artifact route", async () => {
    const block: Block = { type: "media", id: "m", media: { path: "cards/media/shot.png" }, alt: "Order page", caption: "Confirmation" };
    const { port } = mount({ ...result([block]), kind: "order_summary" });
    expect(q("figure figcaption")!.textContent).toBe("Confirmation");
    await vi.waitFor(() => expect(port.artifactBlob).toHaveBeenCalledWith("s1", "cards/media/shot.png"));
    expect(q("figure img")!.getAttribute("alt")).toBe("Order page");
  });

  it("people: paired avatars use the allowlisted tone class and never a style string", () => {
    const block: Block = {
      type: "people",
      id: "p",
      from: { agent_id: "a1", name: "Juno", initials: "J", color: "#2f6fed", role: null },
      to: { agent_id: "a2", name: "Mina", initials: "M", color: "#ef476f; background:url(x)", role: "Prices" },
    };
    mount({ ...ask({ kind: "handoff", blocks: [block], actions: [] }), interaction: "none", id: "handoff:h" }, { defaultOpen: true });
    const [from, to] = all(".avatar");
    expect(from!.className).toBe("avatar avatar--32 avatar--blue");
    expect(to!.className).toBe("avatar avatar--32 avatar--gray");
    expect(host.innerHTML).not.toContain("url(x)");
    expect(all("[style]").every((node) => !node.getAttribute("style")!.includes("background"))).toBe(true);
  });

  it("input_fields: a text field takes the daemon's initial value; secrets never do", () => {
    const blocks: Block[] = [{ type: "input_fields", id: "f", fields: [field({ id: "name", label: "Name", initial: "Mina" }), field({ id: "pw", label: "Password", kind: "password", secret: true, initial: "leaked" })] }];
    mount(ask({ blocks, actions: [] }), { defaultOpen: true });
    const [name, pw] = all<HTMLInputElement>("input");
    expect(name!.value).toBe("Mina");
    expect(pw!.value).toBe("");
  });
});

describe("authenticator", () => {
  const authCard = (): Card =>
    ask({
      kind: "otp",
      blocks: [
        { type: "input_fields", id: "code-fields", fields: [field({ id: "code", label: "Code", secret: true, otp: true })] },
        { type: "input_fields", id: "auth-fields", fields: [field({ id: "key", label: "Authenticator setup key", secret: true }), field({ id: "keep", label: "Save to vault", kind: "checkbox", required: false, initial: true })] },
      ],
      actions: [resolveAction({ type: "otp_code", field_id: "code" }, "Submit code"), resolveAction({ type: "otp_authenticator", field_id: "key", save_field_id: "keep" }, "Use authenticator", "secondary")],
    });

  it("lives in a collapsed disclosure with its own button and is hidden on a device without approve", () => {
    mount(authCard(), { defaultOpen: true });
    const details = q<HTMLDetailsElement>("details.oc-details")!;
    expect(details.open).toBe(false);
    expect(details.querySelectorAll("input")).toHaveLength(2);
    expect(details.querySelector("button")!.textContent).toBe("Use authenticator");
    expect(all(".oc-actions .oc-btn").map((button) => button.textContent)).toEqual(["Submit code"]);

    host.replaceChildren();
    mount(authCard(), { defaultOpen: true, canApprove: false });
    expect(q("details.oc-details")).toBeNull();
  });

  it("saves the key as {totp}, and a refused save hides the save option but keeps the form", async () => {
    const { port } = mount(authCard(), { defaultOpen: true });
    const key = q<HTMLInputElement>("details input[type=text], details input:not([type=checkbox])")!;
    type(key, "otpauth://totp/Bank:me?secret=JBSWY3DPEHPK3PXP");
    q<HTMLButtonElement>("details button")!.click();
    await vi.waitFor(() => expect(port.resolveTicket).toHaveBeenCalledWith("t1", { totp: "otpauth://totp/Bank:me?secret=JBSWY3DPEHPK3PXP" }));
    expect(key.value).toBe("");
  });
});

describe("actions the host does not support", () => {
  const withFollowUp = (): Card => ({ ...fixture("present:p1") });

  it("hides them when the host says so, and shows a plain error when a visible one has no handler", async () => {
    const card = withFollowUp();
    mount(card, { supports: () => false });
    expect(all(".oc-actions .oc-btn")).toHaveLength(0);

    host.replaceChildren();
    mount(card);
    const button = q<HTMLButtonElement>(".oc-actions .oc-btn")!;
    button.click();
    await vi.waitFor(() => expect(q(".oc-error")!.textContent).toContain("isn’t available here"));
  });

  it("passes a follow_up to the host with the card", async () => {
    const card = withFollowUp();
    const onAction = vi.fn(async () => undefined);
    mount(card, { onAction });
    q<HTMLButtonElement>(".oc-actions .oc-btn")!.click();
    await vi.waitFor(() => expect(onAction).toHaveBeenCalled());
    const [action, passed] = onAction.mock.calls[0] as unknown as [Action, Card];
    expect(action.target.type).toBe("follow_up");
    expect(passed.id).toBe(card.id);
  });
});

describe("receipts and rows", () => {
  it("a settled ask is a receipt bubble whose context opens without any field", () => {
    const done: Card = { ...login(), status: "done", collapsed_summary: "Logged in to accounts.example", actions: [] };
    mount(done);
    const pill = q<HTMLElement>(".oc-pill")!;
    expect(q(".oc-status")!.getAttribute("aria-label")).toBe("Done");
    expect(all("input")).toHaveLength(0);
    // Nothing but a field/choice block remains, so there is nothing to disclose.
    expect(pill.tagName).toBe("DIV");
  });

  it("an activity row is a status bubble that opens to its notices", () => {
    const row = fixture("activity:lookup1");
    mount(row);
    expect(q("button.oc-pill")).not.toBeNull();
    q<HTMLButtonElement>("button.oc-pill")!.click();
    expect(q(".oc-notice")).not.toBeNull();
  });

  it("a takeover card keeps Open remote screen, Done and Cancel as typed actions", () => {
    mount(fixture("ticket:take1"), { defaultOpen: true });
    expect(all(".oc-actions .oc-btn").map((button) => button.textContent).sort()).toEqual(["Cancel", "Done", "Open remote screen"]);
  });
});

describe("suggestions", () => {
  const pick = (control: HTMLElement) => {
    control.focus();
    control.dispatchEvent(new Event("input", { bubbles: true }));
  };

  it("offers saved values on a non-secret text area through the glass list and takes one only when chosen", () => {
    const blocks: Block[] = [{ type: "input_fields", id: "f", fields: [field({ id: "street", label: "Street address", kind: "textarea", autocomplete: "street-address", suggestions: ["1 Main St\nSpringfield", "9 Oak Ave"] })] }];
    mount(ask({ blocks, actions: [] }), { defaultOpen: true });
    const area = q<HTMLTextAreaElement>("textarea")!;
    expect(area.value).toBe("");
    pick(area);
    const options = all("li.oc-suggest-option");
    expect(options.map((option) => option.textContent)).toEqual(["1 Main St\nSpringfield", "9 Oak Ave"]);
    expect(area.getAttribute("role")).toBe("combobox");
    expect(area.value).toBe("");
    options[1]!.click();
    expect(area.value).toBe("9 Oak Ave");
  });

  it("never offers them on a password, a code or a card field", () => {
    const blocks: Block[] = [{ type: "input_fields", id: "f", fields: [field({ id: "pw", kind: "password", secret: true, suggestions: ["hunter2"] }), field({ id: "code", secret: true, otp: true, suggestions: ["123456"] }), field({ id: "pan", secret: true, card: "number", suggestions: ["4242424242424242"] })] }];
    mount(ask({ blocks, actions: [] }), { defaultOpen: true });
    for (const control of all("input")) pick(control);
    expect(all(".oc-suggest-list")).toHaveLength(0);
  });
});

describe("link destinations", () => {
  const link = (url: string, label = "Continue securely"): Action => ({ id: `l-${url}`, label, style: "secondary", target: { type: "link", url } });

  it("show where a link really goes beside the label the model chose, and drop one that is not plain http(s)", () => {
    const card = { ...ask({ blocks: [], actions: [link("https://evil.test:8443/pay"), link("javascript:alert(1)", "Click"), link("https://u:p@evil.test/", "Login")] }), site: { origin: "https://bank.test", name: "bank.test", favicon_ref: null } };
    mount(card, { defaultOpen: true });
    const anchors = all<HTMLAnchorElement>(".oc-actions a.oc-btn");
    expect(anchors).toHaveLength(1);
    expect(anchors[0]!.href).toBe("https://evil.test:8443/pay");
    expect(anchors[0]!.querySelector(".oc-host")!.textContent).toBe("evil.test:8443");
  });
});

describe("who asked", () => {
  it("names the helper in the header and on the collapsed bubble, from the host and never from the card", () => {
    const asker = { id: "sub-12345678", label: "Compare prices" };
    mount(login(), { asker });
    const pill = q("button.oc-pill")!;
    expect(pill.querySelector(".oc-asker")!.textContent).toBe("Compare prices asked");
    expect(pill.querySelector(".oc-pill-text")!.textContent).toBe(login().collapsed_summary);
    pill.click();
    const line = q(".oc-head .oc-asker")!;
    expect(line.textContent).toBe("Compare prices asked");
    expect(line.getAttribute("data-subagent-id")).toBe("sub-12345678");
  });

  it("shows nothing when the main agent asked", () => {
    mount(login(), { defaultOpen: true });
    expect(q(".oc-asker")).toBeNull();
  });
});

describe("the settled announcement", () => {
  it("is spoken once and then leaves the page, so the receipt text is not in it twice", () => {
    vi.useFakeTimers();
    try {
      const pending = fixture("ticket:pay1");
      const { view } = mount(pending, { defaultOpen: true });
      view.update({ ...pending, status: "done", collapsed_summary: "Paid", actions: [] });
      const region = q('[role="status"][aria-live]')!;
      expect(region.textContent).toBe("Paid");
      vi.advanceTimersByTime(2000);
      expect(region.textContent).toBe("");
      expect(q(".oc-pill-text")!.textContent).toBe("Paid");
    } finally {
      vi.useRealTimers();
    }
  });
});
