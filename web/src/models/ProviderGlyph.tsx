import { Aperture, Asterisk, Box, KeyRound, Orbit, SquareTerminal, Sparkles, type LucideIcon } from "lucide-react";
import { ICON_STROKE } from "../ui";

/*
 * Generic marks, not vendor logos: each provider gets a lucide shape that is easy to tell apart
 * in a list. Namespaces are matched by family so `openai-codex` and `openai` share one mark.
 */
const GLYPHS: [prefix: string, icon: LucideIcon][] = [
  ["anthropic", Asterisk],
  ["openai", Aperture],
  ["google-antigravity", Orbit],
  ["google", Sparkles],
  ["github-copilot", SquareTerminal],
];

export function providerGlyph(provider: string): LucideIcon {
  return GLYPHS.find(([prefix]) => provider === prefix || provider.startsWith(`${prefix}-`))?.[1] ?? Box;
}

export function ProviderGlyph({ provider, size = 16 }: { provider: string; size?: number }) {
  const Icon = providerGlyph(provider);
  return <Icon size={size} strokeWidth={ICON_STROKE} aria-hidden="true" />;
}

export function ApiKeyGlyph({ size = 16 }: { size?: number }) {
  return <KeyRound size={size} strokeWidth={ICON_STROKE} aria-hidden="true" />;
}
