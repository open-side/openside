#!/usr/bin/env python3
"""Render docs/specs/openapi.json as the Markdown API reference (docs/reference/api.md).

Prints to stdout so `tools/gen-docs.sh` can write or diff the result; the output depends only on
the OpenAPI document. Python 3 standard library only.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "docs/specs/openapi.json"
METHODS = ("get", "post", "put", "patch", "delete", "options", "head", "trace")
HANDLED_EXTENSIONS = {"x-required-capability"}


def cell(text: object) -> str:
    return " ".join(str(text).replace("|", "\\|").split())


def code(text: object) -> str:
    return f"`{text}`"


def type_of(schema: object) -> str:
    """A short, human-readable type for a (possibly nested) JSON Schema."""
    if not isinstance(schema, dict):
        return "any"
    if "$ref" in schema:
        name = schema["$ref"].rsplit("/", 1)[-1]
        return f"[{name}](#{name.lower()})"
    if "const" in schema:
        return code(json.dumps(schema["const"]))
    for key in ("oneOf", "anyOf"):
        if key in schema:
            return " \\| ".join(type_of(variant) for variant in schema[key])
    if "enum" in schema:
        return " \\| ".join(code(json.dumps(value)) for value in schema["enum"])
    kind = schema.get("type")
    kinds = kind if isinstance(kind, list) else [kind] if kind else []
    shown = []
    for item in kinds:
        if item == "array":
            shown.append(f"{type_of(schema.get('items', {}))}[]")
        elif item == "object" and not schema.get("properties") and isinstance(schema.get("additionalProperties"), dict):
            shown.append(f"map of {type_of(schema['additionalProperties'])}")
        else:
            label = item
            if schema.get("format"):
                label += f" ({schema['format']})"
            shown.append(label)
    return " \\| ".join(shown) or "any"


def fields(schema: dict) -> list[str]:
    required = set(schema.get("required", []))
    rows = ["| Field | Type | Required | Description |", "|---|---|---|---|"]
    for name, prop in sorted(schema.get("properties", {}).items()):
        description = prop.get("description", "") if isinstance(prop, dict) else ""
        default = f" Default: {code(json.dumps(prop['default']))}." if isinstance(prop, dict) and "default" in prop else ""
        rows.append(
            f"| {code(name)} | {type_of(prop)} | {'yes' if name in required else 'no'} | {cell(description + default)} |"
        )
    return rows


def render_schema(name: str, schema: dict) -> list[str]:
    lines = [f"### {name}", ""]
    if schema.get("description"):
        lines += [cell(schema["description"]), ""]
    for key in ("oneOf", "anyOf"):
        if key in schema:
            variants = schema[key]
            if schema.get("properties"):
                # Fields shared by every variant (e.g. an event envelope) come first.
                lines += ["Common fields:", ""] + fields(schema) + [""]
                lines += ["Variants:", ""]
            if all("const" in variant for variant in variants):
                lines += ["| Value | Description |", "|---|---|"]
                for variant in variants:
                    lines.append(f"| {code(json.dumps(variant['const']))} | {cell(variant.get('description', ''))} |")
            else:
                for variant in variants:
                    lines += render_variant(variant)
            lines.append("")
            return lines
    if schema.get("properties"):
        lines += fields(schema)
        lines.append("")
        if schema.get("additionalProperties") is False:
            lines += ["Unknown fields are rejected.", ""]
    else:
        lines += [f"Type: {type_of(schema)}", ""]
    return lines


def render_variant(variant: dict) -> list[str]:
    if "$ref" in variant or "properties" not in variant:
        return [f"- {type_of(variant)} {cell(variant.get('description', ''))}".rstrip()]
    tag = next(
        (f"{name}={prop['const']}" for name, prop in sorted(variant["properties"].items()) if "const" in prop),
        variant.get("title", "variant"),
    )
    required = set(variant.get("required", []))
    parts = []
    for name, prop in sorted(variant["properties"].items()):
        if "const" in prop:
            continue
        parts.append(f"{code(name)}: {type_of(prop)}{'' if name in required else ' (optional)'}")
    detail = f" Fields: {', '.join(parts)}." if parts else ""
    return [f"- {code(tag)}: {cell(variant.get('description', ''))}{detail}".replace(": .", ".")]


def media(content: dict) -> list[str]:
    out = []
    for media_type, body in sorted(content.items()):
        text = f"{code(media_type)} {type_of(body.get('schema', {}))}"
        if "x-sse-event-schema" in body:
            text += f"; each event is {type_of(body['x-sse-event-schema'])}"
        out.append(text)
    return out


def render_operation(method: str, path: str, operation: dict) -> list[str]:
    lines = [f"### {method.upper()} {path}", ""]
    scope = operation.get("x-required-capability")
    if scope:
        lines += [f"Required capability: {code(scope)}", ""]
    if operation.get("summary"):
        lines += [cell(operation["summary"]), ""]
    if operation.get("description"):
        lines += [cell(operation["description"]), ""]
    for key, value in sorted(operation.items()):
        if key.startswith("x-") and key not in HANDLED_EXTENSIONS:
            lines += [f"{code(key)}: {code(json.dumps(value, sort_keys=True))}", ""]
    params = operation.get("parameters", [])
    if params:
        lines += ["| Parameter | In | Type | Required | Description |", "|---|---|---|---|---|"]
        for param in params:
            lines.append(
                f"| {code(param['name'])} | {param['in']} | {type_of(param.get('schema', {}))} | "
                f"{'yes' if param.get('required') else 'no'} | {cell(param.get('description', ''))} |"
            )
        lines.append("")
    body = operation.get("requestBody")
    if body:
        lines += [f"Request body{' (required)' if body.get('required') else ''}: " + "; ".join(media(body.get("content", {}))), ""]
    responses = operation.get("responses", {})
    if responses:
        lines += ["| Status | Description | Body |", "|---|---|---|"]
        for status, response in sorted(responses.items()):
            lines.append(f"| {status} | {cell(response.get('description', ''))} | {'; '.join(media(response.get('content', {})))} |")
        lines.append("")
    return lines


def render(document: dict) -> str:
    info = document.get("info", {})
    out = [
        "# API reference",
        "",
        f"Generated from `docs/specs/openapi.json` by `tools/gen-api-docs.py`; do not edit by hand "
        f"(`tools/gen-docs.sh` regenerates it). {info.get('title', '')} {info.get('version', '')}".rstrip() + ".",
        "",
        "## Authentication",
        "",
    ]
    for name, scheme in sorted(document.get("components", {}).get("securitySchemes", {}).items()):
        out += [f"- {code(name)} ({scheme.get('type')}, {scheme.get('scheme', '')}): {cell(scheme.get('description', ''))}"]
    out.append("")

    groups: dict[str, list[tuple[str, str, dict]]] = {}
    for path, item in sorted(document.get("paths", {}).items()):
        parts = path.strip("/").split("/")
        group = parts[1] if len(parts) > 1 else parts[0]
        for method in METHODS:
            if method in item:
                groups.setdefault(group, []).append((method, path, item[method]))
    out += ["## Endpoints", ""]
    out += ["| Group | Operations |", "|---|---|"]
    for group, operations in sorted(groups.items()):
        out.append(f"| {group} | {len(operations)} |")
    out.append("")
    for group, operations in sorted(groups.items()):
        out += [f"## {group}", ""]
        for method, path, operation in operations:
            out += render_operation(method, path, operation)

    out += ["## Schemas", ""]
    for name, schema in sorted(document.get("components", {}).get("schemas", {}).items()):
        out += render_schema(name, schema)
    return "\n".join(out).rstrip() + "\n"


def main() -> int:
    try:
        document = json.loads(SOURCE.read_text())
    except (OSError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2
    sys.stdout.write(render(document))
    return 0


if __name__ == "__main__":
    sys.exit(main())
