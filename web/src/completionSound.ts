import { useEffect, useRef } from "react";
import type { Client } from "./api/client";
import type { Preferences, Session, SessionStatus } from "./api/types";

export const COMPLETION_SOUNDS = ["chime", "pop", "bell"] as const;
export type CompletionSound = (typeof COMPLETION_SOUNDS)[number];

const SOUND_KEY = "openside.completionSound";
/** Fired by Settings › Agents after it saves preferences, so the shell follows without a refetch. */
export const PREFERENCES_EVENT = "openside:preferences";

// Which sound plays belongs to this browser, like the theme; whether one plays is the daemon's
// `completion_sound` preference, shared by every device.
export function loadCompletionSound(): CompletionSound {
  const stored = localStorage.getItem(SOUND_KEY);
  return COMPLETION_SOUNDS.find((sound) => sound === stored) ?? "chime";
}

export function saveCompletionSound(sound: CompletionSound): void {
  localStorage.setItem(SOUND_KEY, sound);
}

export function announcePreferences(preferences: Preferences): void {
  window.dispatchEvent(new CustomEvent<Preferences>(PREFERENCES_EVENT, { detail: preferences }));
}

let audio: AudioContext | null = null;

/** Synthesised rather than shipped as files: three short tones cost no download and no licence. */
export function playCompletionSound(sound: CompletionSound = loadCompletionSound()): void {
  if (typeof window.AudioContext !== "function") return;
  audio ??= new AudioContext();
  const context = audio;
  // A context created before any click starts suspended; the browser resumes it after a gesture.
  void context.resume().catch(() => undefined);
  const start = context.currentTime + 0.01;
  const note = (frequency: number, at: number, length: number, type: OscillatorType, peak: number, glideTo?: number) => {
    const oscillator = context.createOscillator();
    const gain = context.createGain();
    oscillator.type = type;
    oscillator.frequency.setValueAtTime(frequency, start + at);
    if (glideTo) oscillator.frequency.exponentialRampToValueAtTime(glideTo, start + at + length);
    gain.gain.setValueAtTime(0.0001, start + at);
    gain.gain.exponentialRampToValueAtTime(peak, start + at + 0.012);
    gain.gain.exponentialRampToValueAtTime(0.0001, start + at + length);
    oscillator.connect(gain).connect(context.destination);
    oscillator.start(start + at);
    oscillator.stop(start + at + length + 0.02);
  };
  if (sound === "chime") {
    note(1318.5, 0, 0.28, "sine", 0.18);
    note(1760, 0.12, 0.4, "sine", 0.16);
  } else if (sound === "pop") {
    note(880, 0, 0.12, "triangle", 0.25, 440);
  } else {
    note(1046.5, 0, 0.9, "sine", 0.16);
    note(2093, 0, 0.6, "sine", 0.05);
    note(2637, 0, 0.4, "sine", 0.03);
  }
}

/**
 * Plays the completion sound when a chat stops running (it finished, or it now waits for the
 * person) and the `completion_sound` preference is on. Statuses come from the sidebar's poll.
 */
export function useCompletionSound(client: Client, sessions: readonly Session[] | null | undefined): void {
  const enabled = useRef(false);
  const previous = useRef<Map<string, SessionStatus> | null>(null);

  useEffect(() => {
    let alive = true;
    client
      .preferences()
      .then((preferences) => {
        if (alive) enabled.current = preferences.completion_sound;
      })
      .catch(() => undefined);
    const follow = (event: Event) => {
      const detail: unknown = event instanceof CustomEvent ? event.detail : null;
      if (detail && typeof detail === "object" && "completion_sound" in detail && typeof detail.completion_sound === "boolean") enabled.current = detail.completion_sound;
    };
    window.addEventListener(PREFERENCES_EVENT, follow);
    return () => {
      alive = false;
      window.removeEventListener(PREFERENCES_EVENT, follow);
    };
  }, [client]);

  useEffect(() => {
    if (!sessions) return;
    const before = previous.current;
    const now = new Map(sessions.map((session) => [session.id, session.status] as const));
    previous.current = now;
    // The first list only records where every chat stands; nothing has just finished.
    if (!before || !enabled.current) return;
    if (sessions.some((session) => before.get(session.id) === "running" && session.status !== "running")) playCompletionSound();
  }, [sessions]);
}
