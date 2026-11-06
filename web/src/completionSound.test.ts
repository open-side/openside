import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Preferences, Session } from "./api/types";
import { announcePreferences, useCompletionSound } from "./completionSound";
import { fakeClient, json } from "./test/fakeDaemon";

let tones = 0;

/** Just enough Web Audio to count the notes a sound schedules. */
class FakeAudioContext {
  currentTime = 0;
  destination = {};
  resume = async () => undefined;
  createOscillator() {
    tones += 1;
    const param = { setValueAtTime: () => undefined, exponentialRampToValueAtTime: () => undefined };
    return { type: "sine", frequency: param, connect: <T>(next: T) => next, start: () => undefined, stop: () => undefined };
  }
  createGain() {
    const param = { setValueAtTime: () => undefined, exponentialRampToValueAtTime: () => undefined };
    return { gain: param, connect: <T>(next: T) => next };
  }
}

const session = (status: Session["status"]): Session => ({ id: "s1", status }) as Session;

beforeEach(() => {
  tones = 0;
  vi.stubGlobal("AudioContext", FakeAudioContext);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("useCompletionSound", () => {
  it("plays when a running chat stops, and stays quiet once the preference is off", async () => {
    const preferences = { completion_sound: true } as Preferences;
    const fake = fakeClient(() => json(200, preferences));
    const { rerender } = renderHook(({ sessions }) => useCompletionSound(fake.client, sessions), { initialProps: { sessions: [session("running")] } });
    await waitFor(() => expect(fake.calls).toHaveLength(1));
    await act(async () => undefined);

    rerender({ sessions: [session("idle")] });
    expect(tones).toBeGreaterThan(0);

    act(() => announcePreferences({ ...preferences, completion_sound: false }));
    tones = 0;
    rerender({ sessions: [session("running")] });
    rerender({ sessions: [session("waiting_human")] });
    expect(tones).toBe(0);
  });

  it("does not play for chats that were already idle when the list first loaded", async () => {
    const fake = fakeClient(() => json(200, { completion_sound: true }));
    const { rerender } = renderHook(({ sessions }) => useCompletionSound(fake.client, sessions), { initialProps: { sessions: [session("idle")] } });
    await waitFor(() => expect(fake.calls).toHaveLength(1));
    rerender({ sessions: [session("idle")] });
    expect(tones).toBe(0);
  });

  it("stays quiet while the preference is still loading, then plays once it says on", async () => {
    const pending = Promise.withResolvers<Response>();
    const fake = fakeClient(() => pending.promise);
    const { rerender } = renderHook(({ sessions }) => useCompletionSound(fake.client, sessions), { initialProps: { sessions: [session("running")] } });
    rerender({ sessions: [session("idle")] });
    expect(tones).toBe(0);
    await act(async () => pending.resolve(json(200, { completion_sound: true })));
    rerender({ sessions: [session("running")] });
    rerender({ sessions: [session("idle")] });
    expect(tones).toBeGreaterThan(0);
  });

  it("does not play when a running chat disappears from the list", async () => {
    const fake = fakeClient(() => json(200, { completion_sound: true }));
    const { rerender } = renderHook(({ sessions }) => useCompletionSound(fake.client, sessions), { initialProps: { sessions: [session("running")] } });
    await waitFor(() => expect(fake.calls).toHaveLength(1));
    await act(async () => undefined);
    rerender({ sessions: [] });
    expect(tones).toBe(0);
  });
});
