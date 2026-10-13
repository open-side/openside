import type { ModelCatalog, ModelProvider } from "../api/types";
import type { SubscriptionSource } from "./accounts";

const LABELS: Record<string, string> = { anthropic: "Anthropic", openai: "OpenAI", google: "Google" };

/** Providers the daemon can hold API keys for. */
export const KEY_PROVIDERS = new Set(["anthropic", "openai", "google"]);

/** The CLI subscription each provider can be signed in with. */
export const PROVIDER_SUBSCRIPTION: Record<string, SubscriptionSource> = { anthropic: "claude-code", openai: "codex-cli" };

export const SUBSCRIPTION_LABELS: Record<SubscriptionSource, string> = { "claude-code": "Claude Code", "codex-cli": "Codex CLI" };

export function providerLabel(provider: string): string {
  return LABELS[provider] ?? provider.charAt(0).toUpperCase() + provider.slice(1);
}

export function providerReady(provider: ModelProvider): boolean {
  return provider.models.some((model) => model.available);
}

export function anyModelReady(catalog: ModelCatalog): boolean {
  return catalog.providers.some(providerReady);
}
