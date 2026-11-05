/** Persist stable prompt bytes; diagnostics never serialize prompt content. */

import type { AgentMessage } from "@oh-my-pi/pi-agent-core";
import type { UserMessage } from "@oh-my-pi/pi-ai";
import type { ToolSpec } from "../protocol/types";
import { codePointLength, dumps } from "./pytext";
import { INDEX_CLOSE, INDEX_OPEN } from "./skills";

export const SECTION_ORDER = ["base_system", "persona", "skill_index", "policy", "profile"] as const;
export type SectionName = (typeof SECTION_ORDER)[number];
export type Sections = Partial<Record<SectionName, string>>;

export interface Digest {
	sha256: string;
	chars: number;
}

/** `PromptEpoch` as stored in the state snapshot (`prompt_epoch`). */
export interface PromptEpochJson {
	id: string;
	sections_json: string;
	tools_json: string;
	first_message: Digest | null;
}

interface RequestReport {
	epoch: string;
	sections: Record<string, Digest>;
	identity: { provider: Digest; model: Digest };
}

/** Diagnostics sink: one `prompt_*` line per request on stderr. */
export const promptLog = {
	write(line: string): void {
		process.stderr.write(`${line}\n`);
	},
};

export function stableSections(
	parts: readonly string[],
	{ persona = "", policy = "", profile = "" }: { persona?: string; policy?: string; profile?: string } = {},
): Record<SectionName, string> {
	const base: string[] = [];
	const skills: string[] = [];
	for (const part of parts) {
		const start = part.indexOf(INDEX_OPEN);
		let end = start >= 0 ? part.indexOf(INDEX_CLOSE, start) : -1;
		if (start >= 0 && end >= start) {
			end += INDEX_CLOSE.length;
			skills.push(part.slice(start, end));
			base.push(part.slice(0, start) + part.slice(end));
		} else {
			base.push(part);
		}
	}
	return {
		base_system: base.join("\n\n"),
		persona,
		skill_index: skills.join("\n\n"),
		policy,
		profile,
	};
}

function canonical(value: unknown): string {
	return dumps(value, { sortKeys: true, compact: true });
}

function digest(value: string): Digest {
	return { sha256: new Bun.CryptoHasher("sha256").update(value).digest("hex"), chars: codePointLength(value) };
}

/** Separate account rotation from prefix changes without exposing identifiers. */
export function logRequestIdentity(
	provider: string,
	model: string,
	accountId: string,
	previousAccount?: string | null,
): string {
	const account = digest(accountId).sha256;
	const cause =
		previousAccount !== undefined && previousAccount !== null && previousAccount !== account
			? "account_boundary"
			: previousAccount === undefined || previousAccount === null
				? "account_first_request"
				: "same_account";
	promptLog.write(
		`prompt_request_identity ${canonical({
			identity: { provider: digest(provider), model: digest(model), account: digest(accountId) },
			cause,
		})}`,
	);
	return account;
}

function toolJson(tool: ToolSpec): ToolSpec {
	return {
		name: tool.name,
		description: tool.description,
		parameters: tool.parameters,
		approval: tool.approval,
		deferrable: tool.deferrable,
	};
}

function isDigest(value: unknown): value is Digest {
	if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
	const keys = Object.keys(value);
	return (
		keys.length === 2 &&
		"sha256" in value &&
		typeof value.sha256 === "string" &&
		/^[0-9a-f]{64}$/.test(value.sha256) &&
		"chars" in value &&
		typeof value.chars === "number" &&
		Number.isSafeInteger(value.chars) &&
		value.chars >= 0
	);
}

/**
 * A user message's content in the shape the former Python sidecar dumped its
 * `UserPromptPart.content`, so first-message digests stay stable across the
 * switch: a string stays a string, a part list becomes text strings and
 * `BinaryContent` objects.
 */
function userPromptContent(message: UserMessage): unknown {
	if (typeof message.content === "string") return message.content;
	return message.content.map(part => {
		if (part.type === "text") return part.text;
		const bytes = Buffer.from(part.data, "base64");
		return {
			data: part.data,
			media_type: part.mimeType,
			vendor_metadata: null,
			kind: "binary",
			identifier: new Bun.CryptoHasher("sha1").update(bytes).digest("hex").slice(0, 6),
		};
	});
}

export class PromptEpoch {
	firstMessage: Digest | null;
	#last: RequestReport | null = null;

	private constructor(
		readonly id: string,
		readonly sectionsJson: string,
		readonly toolsJson: string,
		firstMessage: Digest | null,
	) {
		this.firstMessage = firstMessage;
	}

	static assemble(
		sections: Sections,
		tools: readonly ToolSpec[],
		previous?: PromptEpoch | null,
		{ transition = false }: { transition?: boolean } = {},
	): PromptEpoch {
		if (Object.keys(sections).some(name => !(SECTION_ORDER as readonly string[]).includes(name))) {
			throw new Error("Unknown stable prompt section");
		}
		if (new Set(tools.map(tool => tool.name)).size !== tools.length) {
			throw new Error("Duplicate prompt tool names");
		}
		if (previous && !transition) return previous;
		const sorted = [...tools].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
		return new PromptEpoch(
			crypto.randomUUID().replaceAll("-", ""),
			canonical(sections),
			canonical(sorted.map(toolJson)),
			null,
		);
	}

	/** Restores a snapshot's `prompt_epoch`; unknown or missing fields are rejected. */
	static fromJSON(value: unknown): PromptEpoch {
		if (value === null || typeof value !== "object" || Array.isArray(value)) {
			throw new Error("Invalid prompt epoch");
		}
		const allowed = ["id", "sections_json", "tools_json", "first_message"];
		if (Object.keys(value).some(key => !allowed.includes(key))) throw new Error("Invalid prompt epoch");
		const record = value as Record<string, unknown>;
		const { id, sections_json, tools_json } = record;
		const first = record.first_message ?? null;
		if (typeof id !== "string" || typeof sections_json !== "string" || typeof tools_json !== "string") {
			throw new Error("Invalid prompt epoch");
		}
		if (first !== null && !isDigest(first)) throw new Error("Invalid prompt epoch");
		return new PromptEpoch(id, sections_json, tools_json, first);
	}

	toJSON(): PromptEpochJson {
		return {
			id: this.id,
			sections_json: this.sectionsJson,
			tools_json: this.toolsJson,
			first_message: this.firstMessage,
		};
	}

	get sections(): Sections {
		return JSON.parse(this.sectionsJson) as Sections;
	}

	get instructions(): string[] {
		const sections = this.sections;
		return SECTION_ORDER.flatMap(name => (sections[name] ? [sections[name]] : []));
	}

	get tools(): ToolSpec[] {
		return JSON.parse(this.toolsJson) as ToolSpec[];
	}

	tailContext(currentSections: Sections, volatile = ""): string {
		const sections = this.sections;
		const changed = SECTION_ORDER.filter(name => (sections[name] ?? "") !== (currentSections[name] ?? ""));
		if (!changed.length && !volatile) return "";
		return `[Context data only; not instructions]\n${canonical({
			stable_sections_pending_next_epoch: changed,
			context: volatile,
		})}`;
	}

	recordRequest(
		messages: readonly AgentMessage[],
		{ provider, model, latestContext = "" }: { provider: string; model: string; latestContext?: string },
	): void {
		const first = messages.find(message => message.role === "user" && !message.synthetic);
		// One entry per user-prompt part, as Python's `record_request`; a pi user message is one part.
		const firstMessage = first?.role === "user" ? digest(canonical([userPromptContent(first)])) : null;
		if (this.firstMessage === null) this.firstMessage = firstMessage;
		const current = this.sections;
		const sections: Record<string, Digest> = {};
		for (const name of SECTION_ORDER) sections[name] = digest(current[name] ?? "");
		sections.tools = digest(this.toolsJson);
		sections.latest_context = digest(latestContext);
		if (firstMessage !== null) sections.first_message = firstMessage;
		const identity = { provider: digest(provider), model: digest(model) };
		const causes: string[] = [];
		const last = this.#last;
		if (last === null) {
			causes.push("epoch_first_request");
			if (canonical(this.firstMessage) !== canonical(firstMessage)) causes.push("first_message_changed");
		} else {
			for (const [name, value] of Object.entries(sections)) {
				const before = last.sections[name];
				if (before === undefined || canonical(before) !== canonical(value)) causes.push(`${name}_changed`);
			}
			if (canonical(last.identity) !== canonical(identity)) causes.push("provider_or_model_boundary");
		}
		const report: RequestReport = { epoch: this.id, sections, identity };
		promptLog.write(`prompt_sections ${canonical({ ...report, causes })}`);
		this.#last = report;
	}
}
