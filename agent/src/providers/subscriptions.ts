/** Standalone diagnostics detect CLI installation, never credential-file contents. */
export interface SubscriptionStatus {
	id: "claude-code" | "codex-cli";
	provider: "anthropic" | "openai-codex";
	installed: boolean;
	logged_in: false;
	available: false;
	reason: string;
}

const CLIS = [
	{ id: "claude-code", executable: "claude", provider: "anthropic" },
	{ id: "codex-cli", executable: "codex", provider: "openai-codex" },
] as const;

/** Executable lookup; tests replace it to simulate installed CLIs. */
export const cliLookup = {
	which: (executable: string): string | null => Bun.which(executable),
};

export function statuses(): SubscriptionStatus[] {
	return CLIS.map(({ id, executable, provider }) => ({
		id,
		provider,
		installed: cliLookup.which(executable) !== null,
		logged_in: false,
		available: false,
		reason: "Login readiness is owned by the daemon",
	}));
}
