// A-9: citation heuristic, visited sources, ultrabrowse planning and citations.
// The ultrabrowse cases drive the research hooks the way the runtime calls them per
// request; streaming and event emission are covered by the runtime tests.

import { describe, expect, it } from "bun:test";
import { rm } from "node:fs/promises";
import type { AgentMessage } from "@oh-my-pi/pi-agent-core";
import type { AssistantMessage, ToolResultMessage, UserMessage } from "@oh-my-pi/pi-ai";
import * as citations from "../src/features/citations";
import * as research from "../src/features/research";

const A = "http://127.0.0.1:41001/rivers";
const B = "http://127.0.0.1:41001/lakes";

const GOOD = `## Findings
- The Nile is about 6,650 km long [Rivers](${A}).
The Amazon carries the largest discharge of any river. [source](${A})
Lake Baikal is the deepest lake on Earth [Lakes](${B}).
Want more detail?`;

const UNCITED = `The Nile is about 6,650 km long [Rivers](${A}).
Lake Baikal holds about a fifth of the world's fresh surface water.
Lake Superior is the largest lake by area [blog](http://127.0.0.1:41001/never-opened).`;

const PLAN = "Plan:\n1. Open the rivers page\n2. Open the lakes page\n";

function user(text: string, synthetic = false): UserMessage {
	return { role: "user", content: text, timestamp: 0, ...(synthetic ? { synthetic } : {}) };
}

function assistant(text: string | null, urls: string[] = []): AssistantMessage {
	return {
		role: "assistant",
		content: [
			...(text ? [{ type: "text" as const, text }] : []),
			...urls.map((url, index) => ({
				type: "toolCall" as const,
				id: `open-${index}`,
				name: "browser_open_tab",
				arguments: { url },
			})),
		],
		api: "mock",
		provider: "test",
		model: "test",
		usage: {
			input: 0,
			output: 0,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 0,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason: urls.length ? "toolUse" : "stop",
		timestamp: 0,
	};
}

function returned(tool: string, details: unknown): ToolResultMessage {
	return { role: "toolResult", toolCallId: tool, toolName: tool, content: [], details, isError: false, timestamp: 0 };
}

function requestText(messages: readonly AgentMessage[]): string {
	return messages
		.flatMap(message => {
			if (message.role !== "user") return [];
			if (typeof message.content === "string") return [message.content];
			return message.content.flatMap(part => (part.type === "text" ? [part.text] : []));
		})
		.join("\n");
}

describe("citations", () => {
	it("test_every_claim_cited_from_visited_pages_scores_zero", () => {
		const report = citations.check(GOOD, [A, `${B}/`]);
		expect(report.claims).toHaveLength(3);
		expect(report.ratio).toBe(0);
		expect(citations.annotate(GOOD, report)).toBe(GOOD);
	});

	it("test_uncited_and_unvisited_links_count_and_are_annotated", () => {
		const report = citations.check(UNCITED, [A, B]);
		expect(report.claims.map(claim => claim.cited)).toEqual([true, false, false]);
		expect(report.ratio).toBeCloseTo(2 / 3);
		const annotated = citations.annotate(UNCITED, report);
		const lines = annotated.split("\n");
		expect(lines[0]).toBe(UNCITED.split("\n")[0] as string);
		expect(lines[1]?.endsWith(`water. ${citations.MARKER}`)).toBe(true);
		expect(lines[2]?.endsWith(`never-opened). ${citations.MARKER}`)).toBe(true);
		// Annotating twice changes nothing: marked sentences are recognized.
		const again = citations.check(annotated, [A, B]);
		expect(citations.annotate(annotated, again)).toBe(annotated);
	});

	it("test_questions_short_fragments_headings_and_code_are_not_claims", () => {
		const text =
			"# Rivers of the world\nShort note.\nIs the Nile longest?\n```\nprint('x is a long code line here')\n```";
		expect(citations.check(text, []).claims).toEqual([]);
		expect(citations.check(text, []).ratio).toBe(0);
	});

	it("test_citation_only_line_attaches_to_the_previous_sentence", () => {
		const text = `The Amazon carries the largest discharge of any river.\n[source](${A})`;
		const report = citations.check(text, [A]);
		expect(report.claims).toHaveLength(1);
		expect(report.claims[0]?.cited).toBe(true);
	});

	it("test_link_after_the_period_belongs_to_its_own_sentence", () => {
		const text = `Fact one has four words. [one](${A}) Fact two has four words [two](${B}).`;
		const report = citations.check(text, [A, B]);
		expect(report.claims.map(claim => claim.cited)).toEqual([true, true]);
		expect(report.claims[0]?.text.endsWith(`[one](${A})`)).toBe(true);
	});

	it("test_abbreviations_and_initials_do_not_split_a_cited_sentence", () => {
		const text = `Rivers in the U.S. carry silt, e.g. the Mississippi delta grows each year, per J. R. Smith [Rivers](${A}).`;
		const report = citations.check(text, [A]);
		expect(report.claims).toHaveLength(1);
		expect(report.claims[0]?.cited).toBe(true);
		// A number before the period still ends the sentence.
		const two = citations.check(`The river is 6,650 km long in 5. Its delta is wide [r](${A}).`, [A]);
		expect(two.claims).toHaveLength(2);
	});

	it("test_urls_with_parentheses_match_visited_pages", () => {
		const page = "https://en.wikipedia.org/wiki/Nile_(river)";
		const report = citations.check(`The Nile flows north to the sea [Nile](${page}).`, [page]);
		expect(report.claims[0]?.cited).toBe(true);
		expect(report.claims[0]?.urls).toEqual([page]);
	});

	it("test_only_titled_http_links_cite_a_claim", () => {
		const claim = "The Nile flows north to the sea";
		const cited = (suffix: string) => citations.check(`${claim} ${suffix}.`, [A]).claims[0]?.cited;
		expect(cited(`[Rivers of Africa](${A})`)).toBe(true);
		expect(cited(A)).toBe(false);
		expect(cited(`[](${A})`)).toBe(false);
		expect(cited(`[ ](${A})`)).toBe(false);
		expect(cited(`[${A}](${A})`)).toBe(false);
		expect(cited(`[Rivers](javascript:alert(1))`)).toBe(false);
		expect(cited(`[Rivers](ftp://127.0.0.1:41001/rivers)`)).toBe(false);
		// A URL wrapped in punctuation is still not a title.
		expect(cited(`[${A}.](${A})`)).toBe(false);
		expect(cited(`[<${A}>](${A})`)).toBe(false);
		expect(cited(`[ — ](${A})`)).toBe(false);
		// Real titles: unicode, nested brackets, and text that merely mentions a URL.
		expect(cited(`[나일강 — 위키백과](${A})`)).toBe(true);
		expect(cited(`[Nile [river] facts](${A})`)).toBe(true);
		expect(cited(`[Rivers at ${A}](${A})`)).toBe(true);
	});

	it("test_paren_url_title_is_not_a_title", () => {
		const page = "https://en.wikipedia.org/wiki/Nile_(river)";
		expect(citations.check(`The Nile flows north to the sea [${page}](${page}).`, [page]).claims[0]?.cited).toBe(false);
	});

	it("test_reference_style_links_are_not_citations", () => {
		const text = `The Nile flows north to the sea [Rivers][r].\n\n[r]: ${A}`;
		const report = citations.check(text, [A]);
		expect(report.claims).toHaveLength(1);
		expect(report.claims[0]?.cited).toBe(false);
	});

	it("test_link_with_nested_brackets_after_the_period_stays_with_its_sentence", () => {
		const text = `Fact one has four words. [One [a]](${A}) Fact two has four words [two](${A}).`;
		const report = citations.check(text, [A]);
		expect(report.claims.map(claim => claim.cited)).toEqual([true, true]);
		expect(report.claims[0]?.text.endsWith(`[One [a]](${A})`)).toBe(true);
	});

	it("test_result_urls_reads_only_structured_url_fields", () => {
		const found = citations.resultUrls([
			{ tab_id: "t", url: `${A}?q=1#x` },
			B,
			{ snapshot: "see http://127.0.0.1:41001/elsewhere", console: [`${B}/x`] },
			"not a url",
		]);
		expect(found).toEqual(new Set([A, B]));
	});

	it("test_visited_sources_trust_only_page_loading_tool_urls", () => {
		const other = "http://127.0.0.1:41001/mentioned";
		const messages = [
			returned("browser_open_tab", { tab_id: "t", url: A }),
			returned("browser_navigate", { status: "navigated", url: B }),
			returned("browser_repl", { return_value: other, console: [other], url: A }),
			returned("browser_snapshot", `[snapshot_id 1]\nlink ${other}`),
			returned("browser_tabs", [{ tab_id: "u", url: other }]),
			returned("bash", { url: other }),
			// A page read with webfetch is a source; a search result list is not.
			returned("webfetch", { url: `${A}/read`, title: "Read", text: `see ${other}` }),
			returned("websearch", { query: "q", results: [{ url: other }] }),
		];
		expect(research.visitedSources(messages)).toEqual(new Set([A, B, `${A}/read`]));
	});

	it("test_web_tools_wait_for_a_plan_like_browser_tools", () => {
		const history: AgentMessage[] = [user("research rivers")];
		expect(research.planGate("websearch", history)).toBe(research.PLAN_REQUIRED);
		expect(research.planGate("webfetch", history)).toBe(research.PLAN_REQUIRED);
		expect(research.planGate("read", history)).toBeUndefined();
		history.push(assistant(PLAN, [A]));
		expect(research.planGate("websearch", history)).toBeUndefined();
		expect(research.planGate("webfetch", history)).toBeUndefined();
	});

	it("test_plan_parsing_takes_the_last_plan_block", () => {
		const text = "Plan:\n1. Open rivers\n2) Open lakes\nThen I browse.\n**Plan:** check peaks";
		expect(research.parsePlan(text)).toEqual(["check peaks"]);
		expect(research.parsePlan("## Plan\n- rivers\n- lakes")).toEqual(["rivers", "lakes"]);
		expect(research.parsePlan("No plan here.")).toEqual([]);
	});

	it("test_only_ultrabrowse_raises_thinking", () => {
		expect(research.modelSettings("ultrabrowse")).toEqual({ thinking: "high" });
		expect(research.modelSettings("browser-agent")).toBeUndefined();
		expect(research.modelSettings(null)).toBeUndefined();
	});

	it("test_ultrabrowse_plans_thinks_and_retries_until_every_claim_is_cited", () => {
		const profile = "ultrabrowse";
		expect(research.modelSettings(profile)).toEqual({ thinking: "high" });
		expect(research.holdsFinalText(profile)).toBe(true);
		const history: AgentMessage[] = [user("research rivers and lakes")];
		// Browsing before planning is refused.
		history.push(assistant(null, [A]));
		const refusal = research.planGate("browser_open_tab", history);
		expect(refusal).toBe(research.PLAN_REQUIRED);
		history.push({
			...returned("browser_open_tab", undefined),
			content: [{ type: "text", text: refusal as string }],
			isError: true,
		});
		expect(research.withPlanReminder(history)).toEqual(history);
		// The plan is stated, so browsing proceeds and the plan rides along every request.
		history.push(assistant(PLAN, [A]));
		expect(research.planGate("browser_open_tab", history)).toBeUndefined();
		history.push(returned("browser_open_tab", { tab_id: "t", url: A }));
		const second = research.withPlanReminder(history);
		expect(second).toHaveLength(history.length + 1);
		expect(requestText(second)).toContain("plan-reminder");
		expect(requestText(second)).toContain("Open the lakes page");
		expect(history).not.toContain(second[second.length - 1] as AgentMessage);
		history.push(assistant(null, [B]));
		history.push(returned("browser_open_tab", { tab_id: "t", url: B }));
		expect(requestText(research.withPlanReminder(history))).toContain("Open the lakes page");
		// The first answer is sent back: two uncited claims.
		const decision = research.citationGuard(history, UNCITED, 0, 3);
		if (decision.action !== "retry") throw new Error(`expected a retry, got ${decision.action}`);
		expect(decision.message).toContain("fresh surface water");
		expect(decision.message).toContain("largest lake by area");
		expect(decision.message).toContain(A);
		expect(decision.message).toContain(B);
		// The retry prompt does not start a new task: the plan still applies.
		history.push(assistant(UNCITED), user(decision.message, true));
		expect(research.planSteps(history)).toEqual(["Open the rivers page", "Open the lakes page"]);
		expect(research.citationGuard(history, GOOD, 1, 3)).toEqual({ action: "allow" });
	});

	it("test_ultrabrowse_annotates_once_citation_retries_are_spent", () => {
		const history: AgentMessage[] = [user("research rivers and lakes")];
		const decisions = Array.from({ length: research.CITATION_RETRIES + 1 }, (_, retry) =>
			research.citationGuard(history, UNCITED, retry, 3),
		);
		expect(decisions.map(decision => decision.action)).toEqual(["retry", "retry", "replace"]);
		const last = decisions[decisions.length - 1];
		if (last?.action !== "replace") throw new Error("expected the answer to be replaced");
		expect(last.text.split(citations.MARKER).length - 1).toBe(3);
		expect(last.text.replaceAll(` ${citations.MARKER}`, "")).toBe(UNCITED);
		// A smaller session retry budget wins over CITATION_RETRIES.
		expect(research.citationGuard(history, UNCITED, 0, 0).action).toBe("replace");
	});

	it("test_other_profiles_neither_gate_browsing_nor_rewrite_answers", () => {
		for (const profile of ["full", "browser-agent", undefined]) {
			expect(research.isResearch(profile)).toBe(false);
			expect(research.holdsFinalText(profile)).toBe(false);
			expect(research.modelSettings(profile)).toBeUndefined();
		}
	});

	it("harness prints the Python report format and exit status", async () => {
		const dir = `${process.env.TMPDIR ?? "/tmp"}/openside-citations-${process.pid}`;
		await Bun.write(`${dir}/answer.md`, UNCITED);
		await Bun.write(`${dir}/visited.json`, JSON.stringify([{ url: A }, { url: B, snapshot: "x" }]));
		const run = (args: string[]) =>
			Bun.spawnSync(
				[process.execPath, "run", `${import.meta.dir}/../src/features/citations.ts`, "--harness", ...args],
				{ stdout: "pipe", stderr: "pipe" },
			);
		const measured = run(["--answer", `${dir}/answer.md`, "--visited-file", `${dir}/visited.json`]);
		expect(measured.exitCode).toBe(0);
		const report = citations.check(UNCITED, [A, B]);
		expect(measured.stdout.toString()).toBe(
			`{"claims": 3, "uncited": 2, "uncited_ratio": 0.6667, "uncited_claims": ${JSON.stringify(
				report.uncited.map(claim => claim.text),
			).replaceAll('","', '", "')}}\n`,
		);
		expect(run(["--answer", `${dir}/answer.md`, "--visited", A, "--max-uncited-ratio", "0.5"]).exitCode).toBe(1);
		expect(run(["--answer", `${dir}/missing.md`]).exitCode).toBe(2);
		const clean = run(["--answer", `${dir}/answer.md`, "--visited", A, "--visited", B, "--max-uncited-ratio", "1"]);
		expect(clean.exitCode).toBe(0);
		await rm(dir, { recursive: true, force: true });
	});
});
