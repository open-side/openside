/**
 * The daemon runs a stdio MCP server as an argv list, never through a shell, so the command line
 * a person types is split here with shell-like quoting only: single quotes keep everything,
 * double quotes and backslashes escape. Nothing is expanded (`$HOME`, `~`, globs stay literal),
 * which is what the daemon will run. Returns null for an unclosed quote or a trailing backslash.
 */
export function splitCommand(line: string): string[] | null {
  const argv: string[] = [];
  let current = "";
  let started = false;
  let quote: "'" | '"' | null = null;
  for (let index = 0; index < line.length; index += 1) {
    const char = line[index]!;
    if (quote === "'") {
      if (char === "'") quote = null;
      else current += char;
      continue;
    }
    if (char === "\\") {
      const next = line[index + 1];
      if (next === undefined) return null;
      // Inside double quotes a backslash only escapes what a shell would let it escape.
      if (quote === '"' && next !== '"' && next !== "\\") current += char;
      current += next;
      started = true;
      index += 1;
      continue;
    }
    if (quote === '"') {
      if (char === '"') quote = null;
      else current += char;
      continue;
    }
    if (char === "'" || char === '"') {
      quote = char;
      started = true;
      continue;
    }
    if (/\s/.test(char)) {
      if (started) argv.push(current);
      current = "";
      started = false;
      continue;
    }
    current += char;
    started = true;
  }
  if (quote) return null;
  if (started) argv.push(current);
  return argv;
}

/** The inverse of `splitCommand`, for showing a saved argv in the edit form. */
export function joinCommand(argv: readonly string[]): string {
  return argv.map((arg) => (arg !== "" && /^[A-Za-z0-9_@%+=:,./-]+$/.test(arg) ? arg : `'${arg.replaceAll("'", `'\\''`)}'`)).join(" ");
}
