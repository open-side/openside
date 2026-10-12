/**
 * Turns the daemon's configuration JSON Schema (`SettingsDocument.schema`) into a flat list of
 * editable keys, so the settings screen follows the daemon's schema instead of a copy of it.
 */

export type FieldKind = "boolean" | "integer" | "number" | "string" | "enum" | "list" | "json";

export interface Field {
  /** Dotted key, e.g. `tickets.confirm_ttl_secs`. */
  path: string;
  section: string;
  description: string;
  kind: FieldKind;
  options: string[];
  nullable: boolean;
  minimum?: number;
}

interface Schema {
  $ref?: string;
  $defs?: Record<string, Schema>;
  type?: string | string[];
  properties?: Record<string, Schema>;
  items?: Schema;
  oneOf?: Schema[];
  anyOf?: Schema[];
  enum?: unknown[];
  const?: unknown;
  description?: string;
  minimum?: number;
}

export type Draft = string | boolean;

export function schemaFields(root: unknown, hiddenSections: string[] = []): Field[] {
  const schema = root as Schema;
  const defs = schema.$defs ?? {};
  const resolve = (node: Schema): Schema => {
    let current = node;
    for (let depth = 0; current.$ref && depth < 32; depth++) {
      current = defs[current.$ref.replace(/^#\/\$defs\//, "")] ?? {};
    }
    return current;
  };
  const out: Field[] = [];

  const walk = (node: Schema, path: string, inherited: string, nullable: boolean) => {
    const resolved = resolve(node);
    const description = node.description ?? resolved.description ?? inherited;
    const base = { path, section: path.split(".")[0] ?? path, description, options: [] as string[] };
    if (resolved.anyOf) {
      const real = resolved.anyOf.filter((variant) => resolve(variant).type !== "null");
      if (real.length === 1 && real[0] && real.length < resolved.anyOf.length) {
        walk({ ...real[0], description }, path, description, true);
      } else {
        out.push({ ...base, kind: "json", nullable });
      }
      return;
    }
    const types = Array.isArray(resolved.type) ? resolved.type : resolved.type ? [resolved.type] : [];
    const isNullable = nullable || types.includes("null");
    const type = types.find((t) => t !== "null");
    const consts = resolved.oneOf?.map((variant) => resolve(variant).const);
    if (consts?.length && consts.every((value) => typeof value === "string")) {
      out.push({ ...base, kind: "enum", options: consts as string[], nullable: isNullable });
    } else if (resolved.enum?.every((value) => typeof value === "string")) {
      out.push({ ...base, kind: "enum", options: resolved.enum as string[], nullable: isNullable });
    } else if (type === "object" && resolved.properties) {
      for (const [key, child] of Object.entries(resolved.properties)) {
        const childPath = path ? `${path}.${key}` : key;
        if (!path && hiddenSections.includes(key)) continue;
        walk(child, childPath, "", false);
      }
    } else if (type === "boolean" || type === "integer" || type === "number" || type === "string") {
      out.push({ ...base, kind: type, nullable: isNullable, minimum: resolved.minimum });
    } else if (type === "array" && resolved.items && resolve(resolved.items).type === "string") {
      out.push({ ...base, kind: "list", nullable: isNullable });
    } else {
      out.push({ ...base, kind: "json", nullable: isNullable });
    }
  };

  walk(schema, "", "", false);
  return out;
}

/** Same rule as the daemon (`settings_api::remote_editable`): a trailing `.` is a prefix. */
export function remoteEditable(path: string, patterns: string[]): boolean {
  return patterns.some((pattern) => (pattern.endsWith(".") ? path.startsWith(pattern) : path === pattern));
}

export function valueAt(document: unknown, path: string): unknown {
  let current: unknown = document;
  for (const key of path.split(".")) {
    if (current === null || typeof current !== "object") return undefined;
    current = (current as Record<string, unknown>)[key];
  }
  return current;
}

function setAt(document: Record<string, unknown>, path: string, value: unknown) {
  const keys = path.split(".");
  let current = document;
  keys.slice(0, -1).forEach((key) => {
    const next = current[key];
    if (next === null || typeof next !== "object") current[key] = {};
    current = current[key] as Record<string, unknown>;
  });
  current[keys[keys.length - 1] ?? path] = value;
}

/** The form's text (or checkbox) representation of a stored value. */
export function toDraft(field: Field, value: unknown): Draft {
  if (field.kind === "boolean") return value === true;
  if (value === null || value === undefined) return "";
  if (field.kind === "list") return (value as string[]).join("\n");
  if (field.kind === "json") return JSON.stringify(value, null, 2);
  return String(value);
}

export class DraftError extends Error {}

function fromDraft(field: Field, draft: Draft): unknown {
  if (field.kind === "boolean") return draft === true;
  const text = String(draft);
  if (text.trim() === "" && field.kind !== "list" && field.kind !== "string") {
    if (field.nullable) return null;
    throw new DraftError(`${field.path} needs a value.`);
  }
  switch (field.kind) {
    case "integer":
    case "number": {
      const number = Number(text);
      if (!Number.isFinite(number) || (field.kind === "integer" && !Number.isInteger(number))) {
        throw new DraftError(`${field.path} must be ${field.kind === "integer" ? "a whole number" : "a number"}.`);
      }
      if (field.minimum !== undefined && number < field.minimum) {
        throw new DraftError(`${field.path} must be at least ${field.minimum}.`);
      }
      return number;
    }
    case "string":
      return text === "" && field.nullable ? null : text;
    case "list":
      return text
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean);
    case "json":
      try {
        return JSON.parse(text) as unknown;
      } catch {
        throw new DraftError(`${field.path} is not valid JSON.`);
      }
    default:
      return text;
  }
}

/** A full replacement document: the stored config with only the edited keys changed. */
export function applyDrafts(config: unknown, fields: Field[], drafts: Record<string, Draft>): unknown {
  const document = JSON.parse(JSON.stringify(config ?? {})) as Record<string, unknown>;
  for (const field of fields) {
    const draft = drafts[field.path];
    if (draft !== undefined) setAt(document, field.path, fromDraft(field, draft));
  }
  return document;
}
