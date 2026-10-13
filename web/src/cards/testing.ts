// Test support only: the daemon's own card fixtures plus small builders. The fixtures are the Rust
// projector's output (crates/openside-proto/src/cards_fixtures.json), so these tests break when
// the wire contract and the renderer drift apart.
import { vi } from "vitest";
import raw from "../../../crates/openside-proto/src/cards_fixtures.json";
import type { Action, Block, Card, InputField } from "../api/types";
import type { CardPort } from "./actions";

export const FIXTURES = raw as unknown as Card[];

export function fixture(id: string): Card {
  const found = FIXTURES.find((card) => card.id === id);
  if (!found) throw new Error(`no card fixture ${id}`);
  return structuredClone(found);
}

export function field(partial: Partial<InputField> & Pick<InputField, "id">): InputField {
  return {
    label: partial.id,
    kind: "text",
    secret: false,
    otp: false,
    card: null,
    required: true,
    autocomplete: null,
    placeholder: null,
    validation: "text",
    max_length: 256,
    initial: null,
    suggestions: [],
    target_origin: null,
    ...partial,
  };
}

export function ask(partial: Partial<Card> & { blocks: Block[]; actions: Action[] }): Card {
  return {
    id: "ticket:t1",
    kind: "form",
    site: { origin: "https://shop.example", name: "shop.example", favicon_ref: null },
    title: "Question",
    subtitle: null,
    status: "pending",
    interaction: "ask",
    ticket_id: "t1",
    collapsed_summary: "Question for shop.example",
    ...partial,
  };
}

export function resolveAction(answer: Extract<Action["target"], { type: "ticket_resolve" }>["answer"], label = "Send", style: Action["style"] = "primary"): Action {
  return { id: `a-${answer.type}`, label, style, target: { type: "ticket_resolve", answer } };
}

/** A port whose requests all succeed; the mocks record what the card sent. */
export function fakePort() {
  const port = {
    resolveTicket: vi.fn(async (_id: string, _value: unknown) => ({})),
    rejectTicket: vi.fn(async (_id: string, _reason?: string) => ({})),
    ticketDone: vi.fn(async (_id: string) => ({})),
    faviconBlob: vi.fn(async (_origin: string) => new Blob(["x"], { type: "image/png" })),
    artifactBlob: vi.fn(async (_session: string, _path: string) => new Blob(["x"], { type: "image/webp" })),
  };
  return port satisfies CardPort;
}
