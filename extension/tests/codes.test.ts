import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Ticket } from "../src/api";
import { announceCodes } from "../src/codes";

const session: Record<string, unknown> = {};
const chromeStub = {
  action: { setBadgeBackgroundColor: vi.fn(async () => {}), setBadgeText: vi.fn(async () => {}) },
  storage: { session: { get: vi.fn(async (key: string) => ({ [key]: session[key] })), set: vi.fn(async (items: Record<string, unknown>) => { Object.assign(session, items); }) } },
  notifications: { create: vi.fn(), clear: vi.fn(async () => true) },
};
vi.stubGlobal("chrome", chromeStub);

// The announcement text is the card's own safe summary; only the fields the notifier reads are given.
const code = { id: "k1", session_id: "s", kind: "secret_input", summary: "Code", instructions: null, options: [], origin: {}, target: { origin: "https://shop.example", tab_id: "t", field_ref: "e1" }, otp: { channel: "email", can_register: false }, status: "pending", card: { id: "ticket:k1", collapsed_summary: "Code from your email for shop.example" } } as unknown as Ticket;
const question = { id: "k2", session_id: "s", kind: "info", summary: "Pick one", instructions: null, options: [], origin: {}, target: null, otp: null, status: "pending", card: { id: "ticket:k2", collapsed_summary: "Pick one" } } as unknown as Ticket;

beforeEach(() => {
  vi.clearAllMocks();
  for (const key of Object.keys(session)) delete session[key];
});

describe("announceCodes", () => {
  it("counts code requests on the badge and announces each one once, never other tickets", async () => {
    await announceCodes([code, question]);
    expect(chromeStub.action.setBadgeText).toHaveBeenLastCalledWith({ text: "1" });
    expect(chromeStub.notifications.create).toHaveBeenCalledExactlyOnceWith("code:k1", expect.objectContaining({ title: "Code needed", message: "Code from your email for shop.example" }));
    await announceCodes([code, question]);
    expect(chromeStub.notifications.create).toHaveBeenCalledOnce();
  });

  it("clears the badge and the notification once the request is answered", async () => {
    await announceCodes([code]);
    await announceCodes([]);
    expect(chromeStub.action.setBadgeText).toHaveBeenLastCalledWith({ text: "" });
    expect(chromeStub.notifications.clear).toHaveBeenCalledWith("code:k1");
  });
});
