import { t } from "../i18n";

/** The one place a provider namespace becomes a name a person reads; unknown namespaces are capitalised. */
const LABELS: Record<string, string> = {
  anthropic: "Anthropic",
  openai: "OpenAI",
  google: "Google",
  "google-gla": "Google",
  "google-vertex": "Google",
  "google-gemini-cli": "Google Gemini CLI",
  "google-antigravity": "Google Antigravity",
  "github-copilot": "GitHub Copilot",
};

/** Providers the daemon can hold API keys for. */
export const KEY_PROVIDERS = ["anthropic", "openai", "google"] as const;

export function providerLabel(provider: string): string {
  if (provider === "openai-codex") return t("models.provider.openaiCodex");
  return LABELS[provider] ?? provider.charAt(0).toUpperCase() + provider.slice(1);
}
