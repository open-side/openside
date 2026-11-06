/** The sources a person manages in the UI; the rest (broker, env, command, builtin) are configured in config.toml. */
export const SUBSCRIPTION_SOURCES = ["claude-code", "codex-cli"] as const;

/** Accounts openside signed in to itself (contract source `pi-oauth`). */
export const OAUTH_SOURCE = "pi-oauth";
