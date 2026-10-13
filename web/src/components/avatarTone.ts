/**
 * The daemon's accent allowlist mapped to avatar tone classes (`.avatar--<tone>` in team.css),
 * so a colour reaches the DOM as a class and never as a style string. Shared by the React
 * avatar and the plain-DOM card renderer.
 */
const TONE: Record<string, string> = {
  "#2f6fed": "blue",
  "#a34dff": "violet",
  "#00a240": "green",
  "#f0b429": "amber",
  "#f97316": "orange",
  "#ef476f": "rose",
  "#00a6a6": "teal",
  "#888888": "gray",
};

/** Mirrors the daemon's `ACCENT_COLORS`; the profile's colour choices. */
export const ACCENT_COLORS = Object.keys(TONE);

export function avatarTone(color: string | null | undefined): string {
  return TONE[(color ?? "").toLowerCase()] ?? "gray";
}
