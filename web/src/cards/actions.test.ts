import { describe, expect, it, vi } from "vitest";
import { ApiError } from "../api/client";
import type { Action, Card } from "../api/types";
import { dispatchAction, SaveRefused, type ActionEnv, type ActionOutcome, type FieldAccess } from "./actions";
import { ask, field, fakePort, resolveAction } from "./testing";

type Values = Record<string, string | boolean>;

function inputs(initial: Values, picked: Record<string, string> = {}) {
  const state: Values = { ...initial };
  const access = {
    peek: (id: string) => state[id],
    clear: (ids: string[]) => {
      for (const id of ids) if (typeof state[id] === "string") state[id] = "";
    },
    restore: (values: Values) => void Object.assign(state, values),
    choice: (blockId: string) => picked[blockId] ?? null,
  } satisfies FieldAccess;
  return { access, state };
}

function env(card: Card, values: Values = {}, over: Partial<ActionEnv> = {}, picked: Record<string, string> = {}) {
  const port = fakePort();
  const box = inputs(values, picked);
  const full: ActionEnv = { card, port, fields: box.access, transient: { refusedKey: null }, ...over };
  return { port, state: box.state, env: full };
}

const refusal = (code: string, status = 403) => new ApiError(status, code, "refused");
const settled = (outcome: "resolved" | "rejected" | "done"): ActionOutcome => ({ kind: "settled", outcome });

function card(blocks: Card["blocks"], actions: Action[]): Card {
  return ask({ blocks, actions });
}

describe("ticket_resolve", () => {
  it("confirm sends the decision as a boolean", async () => {
    const approve = resolveAction({ type: "confirm", approved: true });
    const { port, env: e } = env(card([], [approve]));
    expect(await dispatchAction(approve, e)).toEqual(settled("resolved"));
    expect(port.resolveTicket).toHaveBeenCalledWith("t1", true);
    const decline = resolveAction({ type: "confirm", approved: false }, "Not now", "secondary");
    await dispatchAction(decline, e);
    expect(port.resolveTicket).toHaveBeenLastCalledWith("t1", false);
  });

  it("information sends the trimmed answer, empties the field first and names a missing one", async () => {
    const action = resolveAction({ type: "information", field_id: "answer" });
    const blocks: Card["blocks"] = [{ type: "input_fields", id: "fields", fields: [field({ id: "answer", label: "Answer" })] }];
    const empty = env(card(blocks, [action]), { answer: "  " });
    expect(await dispatchAction(action, empty.env)).toMatchObject({ kind: "invalid", focus: "answer" });
    expect(empty.port.resolveTicket).not.toHaveBeenCalled();

    const filled = env(card(blocks, [action]), { answer: " blue " });
    await dispatchAction(action, filled.env);
    expect(filled.port.resolveTicket).toHaveBeenCalledWith("t1", "blue");
    expect(filled.state.answer).toBe("");
  });

  it("selected_choice sends {option} for a page option and the bare id for an answer", async () => {
    const page = resolveAction({ type: "selected_choice", block_id: "choices", encoding: "page_option" });
    const a = env(card([], [page]), {}, {}, { choices: "opt-2" });
    await dispatchAction(page, a.env);
    expect(a.port.resolveTicket).toHaveBeenCalledWith("t1", { option: "opt-2" });

    const info = resolveAction({ type: "selected_choice", block_id: "choices", encoding: "info_answer" });
    const b = env(card([], [info]), {}, {}, { choices: "red" });
    await dispatchAction(info, b.env);
    expect(b.port.resolveTicket).toHaveBeenCalledWith("t1", "red");

    const none = env(card([], [info]));
    expect(await dispatchAction(info, none.env)).toMatchObject({ kind: "invalid" });
    expect(none.port.resolveTicket).not.toHaveBeenCalled();
  });

  const login = field({ id: "user", label: "Email", kind: "email", secret: true });
  const password = field({ id: "pass", label: "Password", kind: "password", secret: true });
  const save = field({ id: "save", label: "Save", kind: "checkbox", required: false });
  const credentials: Card["blocks"] = [{ type: "input_fields", id: "credentials", fields: [login, password, save] }];

  it("fields/login sends {values, save}, keeps the save box out of values and empties every text field before the request", async () => {
    const action = resolveAction({ type: "fields", block_ids: ["credentials"], encoding: "login", save_field_id: "save" });
    const { port, state, env: e } = env(card(credentials, [action]), { user: "a@b.c", pass: "hunter2", save: true });
    let during: Values | null = null;
    port.resolveTicket.mockImplementationOnce(async () => {
      during = { ...state };
      return {};
    });
    expect(await dispatchAction(action, e)).toEqual(settled("resolved"));
    expect(port.resolveTicket).toHaveBeenCalledWith("t1", { values: { user: "a@b.c", pass: "hunter2" }, save: true });
    expect(during).toMatchObject({ user: "", pass: "" });
    expect(state).toMatchObject({ user: "", pass: "" });
  });

  it("does not save on a device without approve, whatever the box says", async () => {
    const action = resolveAction({ type: "fields", block_ids: ["credentials"], encoding: "login", save_field_id: "save" });
    const { port, env: e } = env(card(credentials, [action]), { user: "u", pass: "p", save: true }, { canApprove: false });
    await dispatchAction(action, e);
    expect(port.resolveTicket).toHaveBeenCalledWith("t1", { values: { user: "u", pass: "p" }, save: false });
  });

  it("names a missing required field and sends nothing", async () => {
    const action = resolveAction({ type: "fields", block_ids: ["credentials"], encoding: "login", save_field_id: "save" });
    const { port, env: e } = env(card(credentials, [action]), { user: "u", pass: "", save: false });
    expect(await dispatchAction(action, e)).toMatchObject({ kind: "invalid", message: "Enter your password.", focus: "pass" });
    expect(port.resolveTicket).not.toHaveBeenCalled();
  });

  it("a 403 forbidden on save puts the details back and refuses saving; any other failure puts nothing back", async () => {
    const action = resolveAction({ type: "fields", block_ids: ["credentials"], encoding: "login", save_field_id: "save" });
    const a = env(card(credentials, [action]), { user: "u", pass: "p", save: true });
    a.port.resolveTicket.mockRejectedValueOnce(refusal("forbidden"));
    await expect(dispatchAction(action, a.env)).rejects.toBeInstanceOf(SaveRefused);
    expect(a.state).toMatchObject({ user: "u", pass: "p" });

    const b = env(card(credentials, [action]), { user: "u", pass: "p", save: true });
    b.port.resolveTicket.mockRejectedValueOnce(refusal("target_unavailable", 409));
    await expect(dispatchAction(action, b.env)).rejects.toMatchObject({ code: "target_unavailable" });
    expect(b.state).toMatchObject({ user: "", pass: "" });
  });

  it("fields/form sends {values}; fields/scalar sends the one string", async () => {
    const name = field({ id: "name" });
    const blocks: Card["blocks"] = [{ type: "input_fields", id: "f", fields: [name, field({ id: "opt", required: false })] }];
    const form = resolveAction({ type: "fields", block_ids: ["f"], encoding: "form", save_field_id: null });
    const a = env(card(blocks, [form]), { name: "Ada", opt: "" });
    await dispatchAction(form, a.env);
    expect(a.port.resolveTicket).toHaveBeenCalledWith("t1", { values: { name: "Ada", opt: "" } });

    const scalar = resolveAction({ type: "fields", block_ids: ["s"], encoding: "scalar", save_field_id: null });
    const one: Card["blocks"] = [{ type: "input_fields", id: "s", fields: [field({ id: "value", secret: true })] }];
    const b = env(card(one, [scalar]), { value: "sk-live" });
    await dispatchAction(scalar, b.env);
    expect(b.port.resolveTicket).toHaveBeenCalledWith("t1", "sk-live");
    expect(b.state.value).toBe("");
  });

  it("login_account and login_alternative send their own bodies", async () => {
    const account = resolveAction({ type: "login_account", item_id: "acct-1" });
    const a = env(card([], [account]));
    await dispatchAction(account, a.env);
    expect(a.port.resolveTicket).toHaveBeenCalledWith("t1", { account: "acct-1" });
    const other = resolveAction({ type: "login_alternative", alternative_id: "google" });
    await dispatchAction(other, a.env);
    expect(a.port.resolveTicket).toHaveBeenLastCalledWith("t1", { alternative: "google" });
  });

  it("otp_code drops the separators sites print and keeps letters of an emailed code", async () => {
    const action = resolveAction({ type: "otp_code", field_id: "code" });
    const numeric = env(card([], [action]), { code: "123 456" });
    await dispatchAction(action, numeric.env);
    expect(numeric.port.resolveTicket).toHaveBeenCalledWith("t1", "123456");
    expect(numeric.state.code).toBe("");
    const dashed = env(card([], [action]), { code: "123-456" });
    await dispatchAction(action, dashed.env);
    expect(dashed.port.resolveTicket).toHaveBeenCalledWith("t1", "123456");
    const lettered = env(card([], [action]), { code: "ab-12" });
    await dispatchAction(action, lettered.env);
    expect(lettered.port.resolveTicket).toHaveBeenCalledWith("t1", "ab-12");
    const empty = env(card([], [action]), { code: "" });
    expect(await dispatchAction(action, empty.env)).toMatchObject({ kind: "invalid", focus: "code" });
  });

  describe("otp_authenticator", () => {
    const KEY = "otpauth://totp/Bank:me?secret=JBSWY3DPEHPK3PXP";
    const action = resolveAction({ type: "otp_authenticator", field_id: "key", save_field_id: "keep" }, "Use authenticator", "secondary");

    it("with keep checked saves the key as {totp} and clears the field", async () => {
      const { port, state, env: e } = env(card([], [action]), { key: KEY, keep: true });
      expect(await dispatchAction(action, e)).toEqual(settled("resolved"));
      expect(port.resolveTicket).toHaveBeenCalledWith("t1", { totp: KEY });
      expect(state.key).toBe("");
    });

    it("without keep fills one code made here and never sends the key", async () => {
      const { port, env: e } = env(card([], [action]), { key: KEY, keep: false });
      await dispatchAction(action, e);
      const [, value] = port.resolveTicket.mock.calls[0]!;
      expect(value).toMatch(/^\d{6}$/);
      expect(JSON.stringify(port.resolveTicket.mock.calls)).not.toContain("JBSWY3DPEHPK3PXP");
    });

    it("rejects text that is not a setup key without sending", async () => {
      const { port, env: e } = env(card([], [action]), { key: "not a key!", keep: false });
      expect(await dispatchAction(action, e)).toMatchObject({ kind: "invalid", message: "That is not a setup key or otpauth:// link." });
      expect(port.resolveTicket).not.toHaveBeenCalled();
    });

    it("a refused save keeps the key only for one explicit fill-once, then forgets it", async () => {
      const { port, env: e } = env(card([], [action]), { key: KEY, keep: true });
      port.resolveTicket.mockRejectedValueOnce(refusal("forbidden"));
      await expect(dispatchAction(action, e)).rejects.toBeInstanceOf(SaveRefused);
      expect(e.transient.refusedKey).not.toBeNull();
      // The next press, with the field empty, fills one code from the retained key.
      expect(await dispatchAction(action, e)).toEqual(settled("resolved"));
      expect(port.resolveTicket).toHaveBeenLastCalledWith("t1", expect.stringMatching(/^\d{6}$/));
      expect(e.transient.refusedKey).toBeNull();
    });

    it("any other failure drops the retained key", async () => {
      const { port, env: e } = env(card([], [action]), { key: KEY, keep: true });
      port.resolveTicket.mockRejectedValueOnce(refusal("internal", 500));
      await expect(dispatchAction(action, e)).rejects.toMatchObject({ status: 500 });
      expect(e.transient.refusedKey).toBeNull();
    });
  });

  describe("hire", () => {
    const hire = (decision: string, edited: string | null) => resolveAction({ type: "hire", decision, edited_block_id: edited }, "Hire");
    const blocks: Card["blocks"] = [{ type: "input_fields", id: "hire-edit", fields: [field({ id: "name" }), field({ id: "role" })] }];

    it("hands the edited values to the host, which owns the request", async () => {
      const action = hire("hire", "hire-edit");
      const host = vi.fn(async () => undefined);
      const { port, env: e } = env(card(blocks, [action]), { name: "Mina", role: "Prices" }, { host });
      expect(await dispatchAction(action, e)).toEqual(settled("resolved"));
      expect(host).toHaveBeenCalledWith(action, e.card, { values: { name: "Mina", role: "Prices" } });
      expect(port.resolveTicket).not.toHaveBeenCalled();
    });

    it("a declining host with edits is unhandled; without edits the decision is sent plainly", async () => {
      const edited = env(card(blocks, [hire("hire", "hire-edit")]), { name: "M", role: "R" }, { host: () => false });
      expect(await dispatchAction(hire("hire", "hire-edit"), edited.env)).toEqual({ kind: "unhandled" });
      const later = env(card(blocks, [hire("not_now", null)]));
      await dispatchAction(hire("not_now", null), later.env);
      expect(later.port.resolveTicket).toHaveBeenCalledWith("t1", "not_now");
    });
  });
});

describe("other targets", () => {
  it("ticket_reject and ticket_done call their own endpoints", async () => {
    const reject: Action = { id: "cancel", label: "Cancel", style: "secondary", target: { type: "ticket_reject" } };
    const done: Action = { id: "done", label: "Done", style: "primary", target: { type: "ticket_done" } };
    const { port, env: e } = env(card([], [reject, done]));
    expect(await dispatchAction(reject, e)).toEqual(settled("rejected"));
    expect(port.rejectTicket).toHaveBeenCalledWith("t1");
    expect(await dispatchAction(done, e)).toEqual(settled("done"));
    expect(port.ticketDone).toHaveBeenCalledWith("t1");
  });

  it("a ticket target on a card without a ticket is an error, not a request", async () => {
    const reject: Action = { id: "cancel", label: "Cancel", style: "secondary", target: { type: "ticket_reject" } };
    const { port, env: e } = env({ ...card([], [reject]), ticket_id: null });
    await expect(dispatchAction(reject, e)).rejects.toThrow("no request");
    expect(port.rejectTicket).not.toHaveBeenCalled();
  });

  it("link opens an http(s) URL without opener or referrer and refuses everything else", async () => {
    const open = vi.fn();
    const link = (url: string): Action => ({ id: "l", label: "Open", style: "secondary", target: { type: "link", url } });
    const { env: e } = env(card([], []), {}, { openUrl: open });
    expect(await dispatchAction(link("https://shop.example/o/1"), e)).toEqual({ kind: "handled" });
    expect(open).toHaveBeenCalledWith("https://shop.example/o/1");
    for (const bad of ["javascript:alert(1)", "data:text/html,x", "https://u:p@shop.example/", "/relative"]) {
      expect(await dispatchAction(link(bad), e)).toMatchObject({ kind: "invalid" });
    }
    expect(open).toHaveBeenCalledTimes(1);
  });

  it("the default opener passes noopener and noreferrer", async () => {
    const win = vi.spyOn(window, "open").mockReturnValue(null);
    const link: Action = { id: "l", label: "Open", style: "secondary", target: { type: "link", url: "https://shop.example/" } };
    await dispatchAction(link, env(card([], [])).env);
    expect(win).toHaveBeenCalledWith("https://shop.example/", "_blank", "noopener,noreferrer");
    win.mockRestore();
  });

  const hostTargets: Action["target"][] = [
    { type: "follow_up", text: "Compare these" },
    { type: "open_thread", agent_id: "agent-1", anchor: null },
    { type: "open_ticket", ticket_id: "t9" },
    { type: "open_connection", connection_id: "c1" },
    { type: "team_command", agent_id: "agent-1", command: "retire" },
    { type: "takeover" },
  ];
  for (const target of hostTargets) {
    it(`${target.type} goes to the host, and is unhandled when the host declines or is missing`, async () => {
      const action: Action = { id: "x", label: "Go", style: "secondary", target };
      const host = vi.fn(async () => undefined);
      const handled = env(card([], [action]), {}, { host });
      expect(await dispatchAction(action, handled.env)).toEqual({ kind: "handled" });
      expect(host).toHaveBeenCalledWith(action, handled.env.card, {});
      const declined = env(card([], [action]), {}, { host: () => false });
      expect(await dispatchAction(action, declined.env)).toEqual({ kind: "unhandled" });
      expect(await dispatchAction(action, env(card([], [action])).env)).toEqual({ kind: "unhandled" });
    });
  }
});
