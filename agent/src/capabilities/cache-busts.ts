// Observation only: a changed prompt prefix between requests discards the provider's
// prompt cache. Only a fingerprint is kept, never prompt text.
export class CacheBustWarning {
	#previous: string | undefined;

	observe(systemPrompt: readonly string[], tools: readonly string[]): void {
		const print = new Bun.CryptoHasher("sha256").update(JSON.stringify([systemPrompt, tools])).digest("hex");
		if (this.#previous !== undefined && this.#previous !== print) {
			process.stderr.write("Prompt prefix changed between model requests; provider cache was invalidated\n");
		}
		this.#previous = print;
	}
}
