/**
 * Python `str`/`re`/`json` semantics that the ported feature modules depend on.
 *
 * The prompt, citation and memory code was written against Python string rules
 * (Unicode `\s`, `str.splitlines`, `json.dumps` spacing). Reproducing them keeps
 * sentence splits, digests and prompt bytes identical to the Python sidecar.
 * Standard library only: `citations.ts` runs standalone through `--harness`.
 */

/** Characters of `str.isspace()`, as a regex class body (Python `re` `\s`). */
export const PY_SPACE = "\\t-\\r\\x1c-\\x20\\x85\\xa0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000";
/** Python `re` `\w` for `str` patterns: `str.isalnum()` or underscore. */
export const PY_WORD = "\\p{L}\\p{N}_";

const LEADING_SPACE = new RegExp(`^[${PY_SPACE}]+`, "u");
const TRAILING_SPACE = new RegExp(`[${PY_SPACE}]+$`, "u");
// `str.splitlines()` boundaries; `\r\n` counts as one.
// biome-ignore lint/suspicious/noControlCharactersInRegex: Python treats \x1c-\x1e as line breaks.
const LINE_BREAK = /\r\n|[\n\r\v\f\x1c\x1d\x1e\x85\u2028\u2029]/gu;

/** `str.lstrip(chars)`; whitespace when `chars` is omitted. */
export function lstrip(value: string, chars?: string): string {
	if (chars === undefined) return value.replace(LEADING_SPACE, "");
	let start = 0;
	while (start < value.length && chars.includes(value[start] as string)) start += 1;
	return value.slice(start);
}

/** `str.rstrip(chars)`; whitespace when `chars` is omitted. */
export function rstrip(value: string, chars?: string): string {
	if (chars === undefined) return value.replace(TRAILING_SPACE, "");
	let end = value.length;
	while (end > 0 && chars.includes(value[end - 1] as string)) end -= 1;
	return value.slice(0, end);
}

export function strip(value: string, chars?: string): string {
	return lstrip(rstrip(value, chars), chars);
}

/** `str.splitlines(keepends)`. */
export function splitlines(value: string, keepends = false): string[] {
	const lines: string[] = [];
	let start = 0;
	for (const match of value.matchAll(LINE_BREAK)) {
		const end = match.index + match[0].length;
		lines.push(value.slice(start, keepends ? end : match.index));
		start = end;
	}
	if (start < value.length) lines.push(value.slice(start));
	return lines;
}

/** `len(value)`: Python counts code points, not UTF-16 units. */
export function codePointLength(value: string): number {
	let count = 0;
	for (const _ of value) count += 1;
	return count;
}

/** `value[:limit]` in code points. */
export function headCodePoints(value: string, limit: number): string {
	if (value.length <= limit) return value;
	let count = 0;
	let index = 0;
	for (const char of value) {
		if (count === limit) break;
		count += 1;
		index += char.length;
	}
	return value.slice(0, index);
}

/** `value[-limit:]` in code points. */
export function tailCodePoints(value: string, limit: number): string {
	if (value.length <= limit) return value;
	const total = codePointLength(value);
	if (total <= limit) return value;
	let skip = total - limit;
	let index = 0;
	for (const char of value) {
		if (skip === 0) break;
		skip -= 1;
		index += char.length;
	}
	return value.slice(index);
}

/** `repr(float)`: shortest round-trip digits, Python's exponent thresholds. */
export function floatRepr(value: number): string {
	if (Number.isNaN(value)) return "NaN";
	if (!Number.isFinite(value)) return value > 0 ? "Infinity" : "-Infinity";
	if (value === 0) return Object.is(value, -0) ? "-0.0" : "0.0";
	const [mantissa, exponentText] = value.toExponential().split("e") as [string, string];
	const exponent = Number(exponentText);
	const negative = mantissa.startsWith("-");
	const digits = mantissa.replace("-", "").replace(".", "");
	const sign = negative ? "-" : "";
	if (exponent < -4 || exponent >= 16) {
		const body = digits.length > 1 ? `${digits[0]}.${digits.slice(1)}` : digits;
		const magnitude = String(Math.abs(exponent)).padStart(2, "0");
		return `${sign}${body}e${exponent < 0 ? "-" : "+"}${magnitude}`;
	}
	if (exponent < 0) return `${sign}0.${"0".repeat(-exponent - 1)}${digits}`;
	const whole = digits.slice(0, exponent + 1).padEnd(exponent + 1, "0");
	const fraction = digits.slice(exponent + 1);
	return `${sign}${whole}.${fraction || "0"}`;
}

/** `round(value, places)`: correctly rounded, ties to even. */
export function roundHalfEven(value: number, places: number): number {
	const rounded = Number(value.toFixed(places));
	// `toFixed` breaks exact binary ties upward; Python breaks them to even.
	const exact = value.toFixed(100);
	const point = exact.indexOf(".");
	const rest = exact.slice(point + 1 + places);
	if (rest[0] !== "5" || /[^0]/.test(rest.slice(1))) return rounded;
	const kept = exact.slice(0, point + 1 + places);
	const last = Number(kept[kept.length - 1]);
	if (last % 2 === 0) return Number(kept);
	return rounded;
}

function compareCodePoints(left: string, right: string): number {
	const a = Array.from(left, char => char.codePointAt(0) as number);
	const b = Array.from(right, char => char.codePointAt(0) as number);
	for (let index = 0; index < Math.min(a.length, b.length); index += 1) {
		const diff = (a[index] as number) - (b[index] as number);
		if (diff !== 0) return diff;
	}
	return a.length - b.length;
}

export interface DumpsOptions {
	/** `sort_keys=True`. */
	sortKeys?: boolean;
	/** `separators=(",", ":")` instead of the default `(", ", ": ")`. */
	compact?: boolean;
}

/**
 * `json.dumps(value, ensure_ascii=False, ...)`. JSON numbers without a fraction
 * are written as Python ints; other numbers as Python floats.
 */
export function dumps(value: unknown, options: DumpsOptions = {}): string {
	const item = options.compact ? "," : ", ";
	const key = options.compact ? ":" : ": ";
	const write = (current: unknown): string => {
		if (current === null || current === undefined) return "null";
		if (typeof current === "boolean") return current ? "true" : "false";
		if (typeof current === "number") return Number.isInteger(current) ? String(current) : floatRepr(current);
		if (typeof current === "string") return JSON.stringify(current);
		if (Array.isArray(current)) return `[${current.map(write).join(item)}]`;
		if (typeof current === "object") {
			const keys = Object.keys(current);
			if (options.sortKeys) keys.sort(compareCodePoints);
			const record = current as Record<string, unknown>;
			return `{${keys.map(name => `${JSON.stringify(name)}${key}${write(record[name])}`).join(item)}}`;
		}
		throw new TypeError(`Object of type ${typeof current} is not JSON serializable`);
	};
	return write(value);
}
