export type Haptic = "approve" | "reject" | "attention";

/** Short, distinct pulses: a confirmation, a refusal, and a nudge that something needs the person. */
const PATTERNS: Record<Haptic, number[]> = {
  approve: [14],
  reject: [10, 40, 10],
  attention: [18, 70, 18],
};

/** Android Chrome only; iOS Safari has no Vibration API, so this is a silent no-op there. */
export function haptic(kind: Haptic): void {
  if (typeof navigator.vibrate !== "function") return;
  navigator.vibrate(PATTERNS[kind]);
}
