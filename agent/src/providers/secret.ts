const REDACTED = "CredentialSecret(**********)";

/**
 * Owns one leased token. JavaScript strings cannot be zeroed, so `clear()` drops
 * the only reference this sidecar keeps; every printable form is redacted so a
 * stray log line or error serializer never carries the value.
 */
export class CredentialSecret {
	#value: string;

	constructor(value: string) {
		this.#value = value;
	}

	reveal(): string {
		return this.#value;
	}

	clear(): void {
		this.#value = "";
	}

	toString(): string {
		return REDACTED;
	}

	toJSON(): string {
		return REDACTED;
	}

	[Symbol.for("nodejs.util.inspect.custom")](): string {
		return REDACTED;
	}
}
