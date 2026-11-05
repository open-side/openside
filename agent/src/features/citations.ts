/**
 * Sentence-level citation coverage of research answers (A-9).
 *
 * Standard library only: `tools/citation-harness.py` runs this file through
 * `bun run agent/src/features/citations.ts --harness`, outside the sidecar. The
 * heuristic is documented in docs/specs/ultrabrowse.md §3:
 *
 * - The answer is split into sentences per line, after dropping headings, fenced code
 *   and list markers. Links right after a sentence's end punctuation belong to that
 *   sentence ("Fact. [src](url) Next fact."), and common abbreviations ("e.g.",
 *   "U.S.") or initials do not end a sentence.
 * - A sentence is a factual claim when, with links and URLs removed, it has at least
 *   MIN_CLAIM_WORDS words and does not end with "?".
 * - A claim is cited when it contains an inline Markdown link `[title](url)` whose text is
 *   a title and whose http(s) target, normalized, equals a page visited in the session.
 *   Link text may hold one level of nested brackets. Text that is blank, or only a URL
 *   once surrounding punctuation is trimmed, is not a title. Bare URLs, reference-style
 *   links (`[title][id]`) and links to pages that were never opened count as uncited.
 * - Visited pages come only from structured `url` fields of browser tool results,
 *   never from URLs that appear inside page text.
 */

import { dumps, floatRepr, lstrip, PY_SPACE, PY_WORD, roundHalfEven, rstrip, splitlines, strip } from "./pytext";

export const MARKER = "[citation needed]";
export const MIN_CLAIM_WORDS = 4;

const URL_CHAR = `[^${PY_SPACE}<>()\\[\\]"'\`]`;
// One level of balanced parentheses keeps URLs like .../wiki/Nile_(river) whole.
const URL_BODY = `(?:${URL_CHAR}|\\(${URL_CHAR}*\\))+`;
const URL_SOURCE = `https?://${URL_BODY}`;
const URL_GLOBAL = new RegExp(URL_SOURCE, "gu");
// Link text with one level of nested brackets, e.g. "[Nile [river] facts](url)".
const LINK_TEXT = "(?:[^\\[\\]]|\\[[^\\[\\]]*\\])*";
const LINK_GLOBAL = new RegExp(`\\[(${LINK_TEXT})\\]\\((${URL_SOURCE})\\)`, "gu");
const EDGE = "[\\p{P}\\p{S}\\p{Z}\\s]*";
// A URL wrapped only in punctuation ("<https://a/b>", "https://a/b.") is not a title.
const URL_TITLE = new RegExp(`^${EDGE}${URL_SOURCE}${EDGE}$`, "u");
const TITLE_CHAR = /[^\p{P}\p{S}\p{Z}\s]/u;
const WORD = new RegExp(`[${PY_WORD}]+`, "gu");
const SENTENCE_END_SOURCE = `(?<=[.!?])((?:[${PY_SPACE}]*\\[${LINK_TEXT}\\]\\(${URL_BODY}\\))*)[${PY_SPACE}]+`;
const LIST_MARKER = new RegExp(`^[${PY_SPACE}]*(?:[-*+]|\\d+[.)])[${PY_SPACE}]+`, "u");
// "etc." and "no." often end sentences, so they still split.
const ABBREVIATIONS = new Set(
	"e.g. i.e. cf. vs. approx. ca. dr. mr. mrs. ms. prof. st. fig. inc. ltd. co. jr. sr. u.s. u.k. u.n. e.u. a.m. p.m.".split(
		" ",
	),
);
const INITIALS = /^(?:[\p{L}\p{Nl}\p{No}]\.){1,2}$/u;
const SPACE_RUN = new RegExp(`[${PY_SPACE}]+`, "u");
const ANY_WORD = new RegExp(`[${PY_WORD}]`, "u");

export interface Claim {
	readonly text: string;
	readonly urls: readonly string[];
	readonly cited: boolean;
	/** Offset just past the sentence in the checked answer, where annotate() marks it. */
	readonly end: number;
}

export interface ReportJson {
	claims: number;
	uncited: number;
	uncited_ratio: number;
	uncited_claims: string[];
}

export class Report {
	constructor(readonly claims: readonly Claim[]) {}

	get uncited(): Claim[] {
		return this.claims.filter(claim => !claim.cited);
	}

	get ratio(): number {
		return this.claims.length ? this.uncited.length / this.claims.length : 0;
	}

	toJSON(): ReportJson {
		const uncited = this.uncited;
		return {
			claims: this.claims.length,
			uncited: uncited.length,
			uncited_ratio: roundHalfEven(this.ratio, 4),
			uncited_claims: uncited.map(claim => claim.text),
		};
	}
}

/** Python `urlsplit` for the parts `normalize` keeps. */
function splitUrl(raw: string): { scheme: string; netloc: string; path: string } {
	// biome-ignore lint/suspicious/noControlCharactersInRegex: Python `urlsplit` strips leading C0 controls and spaces.
	let url = raw.replace(/^[\x00-\x20]+/, "").replace(/[\t\r\n]/g, "");
	let scheme = "";
	let netloc = "";
	const colon = url.indexOf(":");
	if (colon > 0 && /^[A-Za-z][A-Za-z0-9+.-]*$/.test(url.slice(0, colon))) {
		scheme = url.slice(0, colon).toLowerCase();
		url = url.slice(colon + 1);
	}
	if (url.startsWith("//")) {
		const rest = url.slice(2);
		const cut = rest.search(/[/?#]/);
		netloc = cut < 0 ? rest : rest.slice(0, cut);
		url = cut < 0 ? "" : rest.slice(cut);
	}
	const hash = url.indexOf("#");
	if (hash >= 0) url = url.slice(0, hash);
	const query = url.indexOf("?");
	if (query >= 0) url = url.slice(0, query);
	return { scheme, netloc, path: url };
}

// Python 3.13 `urllib.parse.uses_netloc`.
const USES_NETLOC = new Set([
	"",
	"ftp",
	"http",
	"gopher",
	"nntp",
	"telnet",
	"imap",
	"wais",
	"file",
	"mms",
	"https",
	"shttp",
	"snews",
	"prospero",
	"rtsp",
	"rtsps",
	"rtspu",
	"rsync",
	"svn",
	"svn+ssh",
	"sftp",
	"nfs",
	"git",
	"git+ssh",
	"ws",
	"wss",
	"itms-services",
]);

/** Scheme/host case, query, fragment, trailing slash or punctuation do not count. */
export function normalize(url: string): string {
	const parts = splitUrl(rstrip(url, ".,;:!?"));
	const netloc = parts.netloc.toLowerCase();
	let path = rstrip(parts.path, "/");
	// Python `urlunsplit` with empty query and fragment.
	if (netloc) {
		if (path && !path.startsWith("/")) path = `/${path}`;
		path = `//${netloc}${path}`;
	} else if (path.startsWith("//")) {
		path = `//${path}`;
	} else if (parts.scheme && USES_NETLOC.has(parts.scheme) && (!path || path.startsWith("/"))) {
		path = `//${path}`;
	}
	return parts.scheme ? `${parts.scheme}:${path}` : path;
}

/**
 * Normalized page URLs named by browser tool results.
 *
 * Accepts a URL string, a result object (its top-level `url` field) or a list of
 * those. Other strings inside a result (snapshot text, console output, return values)
 * are page content and are deliberately ignored.
 */
export function resultUrls(value: unknown): Set<string> {
	const items: unknown[] = Array.isArray(value) ? value : [value];
	const found = new Set<string>();
	for (const item of items) {
		const url =
			item !== null && typeof item === "object" && !Array.isArray(item) ? (item as Record<string, unknown>).url : item;
		if (typeof url === "string" && (url.startsWith("http://") || url.startsWith("https://"))) {
			found.add(normalize(url));
		}
	}
	return found;
}

function plain(sentence: string): string {
	return sentence.replaceAll(MARKER, " ").replace(LINK_GLOBAL, " ").replace(URL_GLOBAL, " ");
}

/** Whether the '.' at `period` ends an abbreviation or initial, not a sentence. */
function abbreviated(line: string, period: number): boolean {
	if (line[period] !== ".") return false;
	const tokens = line.slice(0, period + 1).split(SPACE_RUN);
	const token = lstrip(tokens[tokens.length - 1] as string, "([\"'").toLowerCase();
	return ABBREVIATIONS.has(token) || INITIALS.test(token);
}

/** [start, end) offsets of each sentence in `answer`. */
export function sentenceSpans(answer: string): Array<[number, number]> {
	const spans: Array<[number, number]> = [];
	const add = (start: number, end: number): void => {
		if (start >= end) return;
		const last = spans[spans.length - 1];
		if (last && !ANY_WORD.test(plain(answer.slice(start, end)))) {
			last[1] = end;
		} else {
			spans.push([start, end]);
		}
	};
	let fenced = false;
	let offset = 0;
	for (const raw of splitlines(answer, true)) {
		const base = offset;
		offset += raw.length;
		const line = rstrip(raw, "\r\n");
		const stripped = strip(line);
		if (stripped.startsWith("```")) {
			fenced = !fenced;
			continue;
		}
		if (fenced || !stripped || stripped.startsWith("#")) continue;
		const marker = LIST_MARKER.exec(line);
		let start = marker ? marker[0].length : line.length - lstrip(line).length;
		const gaps = new RegExp(SENTENCE_END_SOURCE, "dgu");
		gaps.lastIndex = start;
		for (let gap = gaps.exec(line); gap !== null; gap = gaps.exec(line)) {
			// Group 1 always participates (it may be empty), so its indices exist.
			const [groupStart, groupEnd] = (gap.indices as RegExpIndicesArray)[1] as [number, number];
			if (groupStart === groupEnd && abbreviated(line, gap.index - 1)) continue;
			add(base + start, base + groupEnd);
			start = gap.index + gap[0].length;
		}
		add(base + start, base + rstrip(line).length);
	}
	return spans;
}

export function sentences(answer: string): string[] {
	return sentenceSpans(answer).map(([start, end]) => answer.slice(start, end));
}

export function check(answer: string, visited: Iterable<string>): Report {
	const sources = new Set(Array.from(visited, normalize));
	const claims: Claim[] = [];
	for (const [start, end] of sentenceSpans(answer)) {
		const sentence = answer.slice(start, end);
		const text = strip(plain(sentence));
		if ((text.match(WORD)?.length ?? 0) < MIN_CLAIM_WORDS || text.endsWith("?")) continue;
		const urls = sentence.match(URL_GLOBAL) ?? [];
		const cited = Array.from(sentence.matchAll(LINK_GLOBAL)).some(
			([, title = "", url = ""]) => TITLE_CHAR.test(title) && !URL_TITLE.test(title) && sources.has(normalize(url)),
		);
		claims.push({ text: sentence, urls, cited, end });
	}
	return new Report(claims);
}

/** Append MARKER after each uncited claim; the answer is otherwise unchanged. */
export function annotate(answer: string, report: Report): string {
	let out = answer;
	for (const claim of report.uncited.reverse()) {
		if (!claim.text.endsWith(MARKER)) out = `${out.slice(0, claim.end)} ${MARKER}${out.slice(claim.end)}`;
	}
	return out;
}

const HARNESS_USAGE =
	"usage: citations.ts --harness --answer ANSWER [--visited VISITED] [--visited-file VISITED_FILE] [--max-uncited-ratio MAX_UNCITED_RATIO]";

/**
 * CLI behind `tools/citation-harness.py`; same arguments, JSON report and exit status
 * (0 measured and within the threshold, 1 above it, 2 unusable input).
 */
export async function harness(argv: string[]): Promise<number> {
	let answerPath: string | undefined;
	const visitedArgs: string[] = [];
	let visitedFile: string | undefined;
	let maxRatio: number | undefined;
	for (let index = 0; index < argv.length; index += 1) {
		const arg = argv[index] as string;
		const eq = arg.indexOf("=");
		const name = arg.startsWith("--") && eq > 0 ? arg.slice(0, eq) : arg;
		const inline = arg.startsWith("--") && eq > 0 ? arg.slice(eq + 1) : undefined;
		const take = (): string | undefined => inline ?? argv[++index];
		let value: string | undefined;
		switch (name) {
			case "--answer":
				value = take();
				answerPath = value;
				break;
			case "--visited":
				value = take();
				if (value !== undefined) visitedArgs.push(value);
				break;
			case "--visited-file":
				value = take();
				visitedFile = value;
				break;
			case "--max-uncited-ratio": {
				value = take();
				const ratio = value === undefined ? Number.NaN : Number(value);
				if (value === undefined || value.trim() === "" || Number.isNaN(ratio)) {
					process.stderr.write(
						`${HARNESS_USAGE}\ncitations.ts: error: argument --max-uncited-ratio: invalid float value\n`,
					);
					return 2;
				}
				maxRatio = ratio;
				break;
			}
			default:
				process.stderr.write(`${HARNESS_USAGE}\ncitations.ts: error: unrecognized arguments: ${arg}\n`);
				return 2;
		}
		if (value === undefined) {
			process.stderr.write(`${HARNESS_USAGE}\ncitations.ts: error: argument ${name}: expected one argument\n`);
			return 2;
		}
	}
	if (answerPath === undefined) {
		process.stderr.write(`${HARNESS_USAGE}\ncitations.ts: error: the following arguments are required: --answer\n`);
		return 2;
	}
	let answer: string;
	const visited = new Set(visitedArgs);
	try {
		answer = answerPath === "-" ? await Bun.stdin.text() : await Bun.file(answerPath).text();
		if (visitedFile !== undefined) {
			for (const url of resultUrls(JSON.parse(await Bun.file(visitedFile).text()))) visited.add(url);
		}
	} catch (error) {
		process.stderr.write(`cannot read input: ${error instanceof Error ? error.message : String(error)}\n`);
		return 2;
	}
	const report = check(answer, visited);
	const json = report.toJSON();
	// Python `json.dumps` spacing and float formatting, so existing consumers parse it unchanged.
	const ratio = floatRepr(json.uncited_ratio);
	const head = dumps({ claims: json.claims, uncited: json.uncited });
	process.stdout.write(
		`${head.slice(0, -1)}, "uncited_ratio": ${ratio}, "uncited_claims": ${dumps(json.uncited_claims)}}\n`,
	);
	if (maxRatio !== undefined && report.ratio > maxRatio) return 1;
	return 0;
}

if (import.meta.main) {
	const args = process.argv.slice(2);
	if (args[0] !== "--harness") {
		process.stderr.write(`${HARNESS_USAGE}\n`);
		process.exit(2);
	}
	process.exit(await harness(args.slice(1)));
}
