#!/usr/bin/env python3
"""Fail when docs/specs/openapi.json breaks clients of docs/specs/openapi.baseline.json.

The baseline is the last released public API v1 snapshot. Additive changes (new paths, methods,
optional request fields, new response fields, new response enum values or union variants) pass.
Breaking changes are reported one per line and make the exit status 1:

* a path or method disappears, or an old 2xx status is no longer returned;
* a response field disappears or stops being guaranteed (required -> optional);
* a request gains a required field, parameter or body;
* an enum or union loses a value that clients may send, or a response enum/union loses one
  clients may receive; a `const` changes;
* a type changes (requests may only widen, responses may only narrow);
* a request property is removed from a schema that rejects unknown properties;
* the capability scope an operation needs becomes stricter than what old tokens held.

`--update` copies the current document over the baseline (refused when it breaks the old one,
unless `--allow-breaking`). Python 3 standard library only.
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CURRENT = ROOT / "docs/specs/openapi.json"
DEFAULT_BASELINE = ROOT / "docs/specs/openapi.baseline.json"
METHODS = ("get", "put", "post", "delete", "options", "head", "patch", "trace")
REQUEST, RESPONSE = "request", "response"
SCOPES = ("read", "steer", "approve", "human_step", "browse", "vault_use", "otp_ingest", "admin")


def scope_grants(held: str, required: str) -> bool:
    """Whether a token holding `held` may call an operation that requires `required`."""
    return held == required or held == "admin" or (held == "steer" and required == "browse")


class Differ:
    def __init__(self, old: dict, new: dict):
        self.old, self.new = old, new
        self.findings: list[str] = []
        self.additions: list[str] = []

    # --- schema helpers -------------------------------------------------------------------

    @staticmethod
    def resolve(doc: dict, schema, seen=()):
        """Follow `$ref` and fold `allOf` into one object schema; siblings of `$ref` win."""
        if not isinstance(schema, dict):
            return {}
        schema = dict(schema)
        ref = schema.pop("$ref", None)
        if ref is not None:
            if not ref.startswith("#/") or ref in seen:
                return schema
            target = doc
            for part in ref[2:].split("/"):
                target = target.get(part.replace("~1", "/").replace("~0", "~"), {}) if isinstance(target, dict) else {}
            merged = Differ.resolve(doc, target, seen + (ref,))
            merged.update(schema)
            schema = merged
        parts = schema.pop("allOf", None)
        if parts:
            folded: dict = {}
            for part in parts:
                part = Differ.resolve(doc, part, seen)
                for key, value in part.items():
                    if key == "properties":
                        folded.setdefault("properties", {}).update(value)
                    elif key == "required":
                        folded["required"] = sorted(set(folded.get("required", [])) | set(value))
                    else:
                        folded.setdefault(key, value)
            for key, value in schema.items():
                if key == "properties":
                    folded.setdefault("properties", {}).update(value)
                elif key == "required":
                    folded["required"] = sorted(set(folded.get("required", [])) | set(value))
                else:
                    folded[key] = value
            schema = folded
        return schema

    @staticmethod
    def types(schema: dict) -> set[str] | None:
        kind = schema.get("type")
        if kind is None:
            return None
        return set(kind) if isinstance(kind, list) else {kind}

    def nullable(self, doc: dict, schema: dict) -> bool:
        types = self.types(schema)
        if (types is not None and "null" in types) or schema.get("nullable") is True:
            return True
        return any(self.nullable(doc, self.resolve(doc, variant)) for variant in self.variants(schema) or [])

    @staticmethod
    def wildcard(schema: dict) -> bool:
        """A schema that accepts any value (no type, enum, const, union, shape keywords)."""
        shape = ("type", "enum", "const", "oneOf", "anyOf", "properties", "items", "pattern", "format")
        return not any(key in schema for key in shape)

    def union(self, old, new, old_variants, new_variants, direction, where, seen) -> list[str]:
        found: list[str] = []
        old_list = old_variants if old_variants is not None else [old]
        new_list = new_variants if new_variants is not None else [new]

        def compatible(old_variant, new_variant) -> bool:
            return not self.schema(old_variant, new_variant, direction, where, seen)

        if direction == RESPONSE and old_variants is None:
            # A field that used to be one shape may not start producing other shapes.
            for index, candidate in enumerate(new_list):
                if not compatible(old, candidate):
                    label = describe(self.resolve(self.new, candidate)) or f"variant {index}"
                    found.append(f"{where}: response may now also be `{label}`, which the old schema did not allow")
        elif direction == RESPONSE and new_variants is None:
            # Producing fewer shapes is fine as long as the one left was already allowed.
            if not any(compatible(variant, new) for variant in old_list):
                found.append(f"{where}: response no longer matches any previously documented shape")
        else:
            for index, old_variant in enumerate(old_list):
                # A union may grow, never lose a member some client relies on.
                if not any(compatible(old_variant, candidate) for candidate in new_list):
                    label = describe(self.resolve(self.old, old_variant)) or f"variant {index}"
                    found.append(f"{where}: union member `{label}` was removed or changed incompatibly")
        return found

    @staticmethod
    def variants(schema: dict) -> list | None:
        for key in ("oneOf", "anyOf"):
            if key in schema:
                return schema[key]
        return None

    # --- schema comparison ----------------------------------------------------------------

    def schema(self, old, new, direction: str, where: str, seen=frozenset()) -> list[str]:
        """Breaking differences between two schemas, as seen by a client of `direction`.

        For a request the client sends values: the new schema must accept everything the old one
        did. For a response the client receives values: the new schema may only produce what the
        old one promised.
        """
        # Recursive schemas always recurse through a `$ref`, so the ref pair is the cycle key.
        if isinstance(old, dict) and isinstance(new, dict) and "$ref" in old and "$ref" in new:
            key = (old["$ref"], new["$ref"], direction)
            if key in seen:
                return []
            seen = seen | {key}
        old = self.resolve(self.old, old)
        new = self.resolve(self.new, new)
        found: list[str] = []

        old_null, new_null = self.nullable(self.old, old), self.nullable(self.new, new)
        if direction == RESPONSE and new_null and not old_null:
            found.append(f"{where}: may now be null")
        if direction == REQUEST and old_null and not new_null:
            found.append(f"{where}: null is no longer accepted")

        old_variants, new_variants = self.variants(old), self.variants(new)
        if old_variants is not None or new_variants is not None:
            found += self.union(old, new, old_variants, new_variants, direction, where, seen)
            # A union may sit next to shared `properties`/`required`; those are checked below.
            if not any(key in old or key in new for key in ("properties", "required", "items", "type")):
                return found

        if direction == REQUEST and self.wildcard(old) and not self.wildcard(new):
            found.append(f"{where}: request value used to be unconstrained and is now restricted")

        old_const, new_const = "const" in old, "const" in new
        if old_const and new_const:
            if old["const"] != new["const"]:
                found.append(f"{where}: const changed from {old['const']!r} to {new['const']!r}")
            return found
        if old_const and direction == RESPONSE:
            found.append(f"{where}: constant {old['const']!r} is no longer guaranteed")
        if new_const and direction == REQUEST:
            found.append(f"{where}: request value is now fixed to {new['const']!r}")

        old_enum, new_enum = old.get("enum"), new.get("enum")
        if old_enum is not None:
            if new_enum is None:
                if direction == RESPONSE:
                    found.append(f"{where}: enum {old_enum} is no longer enforced")
            else:
                if direction == REQUEST and set(map(repr, old_enum)) - set(map(repr, new_enum)):
                    gone = sorted(set(map(repr, old_enum)) - set(map(repr, new_enum)))
                    found.append(f"{where}: enum value(s) no longer accepted: {', '.join(gone)}")
                if direction == RESPONSE and set(map(repr, old_enum)) - set(map(repr, new_enum)):
                    gone = sorted(set(map(repr, old_enum)) - set(map(repr, new_enum)))
                    found.append(f"{where}: enum value(s) no longer produced: {', '.join(gone)}")
        elif new_enum is not None and direction == REQUEST:
            found.append(f"{where}: request value is now restricted to enum {new_enum}")

        old_types, new_types = self.types(old), self.types(new)
        if old_types is not None and new_types is not None and old_types != new_types:
            widened = old_types <= new_types
            narrowed = new_types <= old_types
            # `integer` is a kind of `number`.
            if "number" in new_types and old_types <= (new_types | {"integer"}):
                widened = True
            if "number" in old_types and new_types <= (old_types | {"integer"}):
                narrowed = True
            ok = widened if direction == REQUEST else narrowed
            if not ok:
                found.append(f"{where}: type changed from {sorted(old_types)} to {sorted(new_types)}")
        elif old_types is not None and new_types is None and direction == RESPONSE:
            found.append(f"{where}: type {sorted(old_types)} is no longer declared")

        old_props, new_props = old.get("properties", {}), new.get("properties", {})
        old_required, new_required = set(old.get("required", [])), set(new.get("required", []))
        closed = new.get("additionalProperties") is False
        if direction == REQUEST:
            for name in sorted(new_required - old_required):
                found.append(f"{where}: new required request field `{name}`")
            for name in sorted(set(old_props) - set(new_props)):
                if closed:
                    found.append(f"{where}: request field `{name}` was removed and unknown fields are rejected")
        else:
            for name in sorted(set(old_props) - set(new_props)):
                found.append(f"{where}: response field `{name}` was removed")
            for name in sorted((old_required - new_required) & set(new_props)):
                found.append(f"{where}: response field `{name}` is no longer always present")
        for name in sorted(set(old_props) & set(new_props)):
            found += self.schema(old_props[name], new_props[name], direction, f"{where}.{name}", seen)

        if "items" in old or "items" in new:
            if "items" in old and "items" in new:
                found += self.schema(old["items"], new["items"], direction, f"{where}[]", seen)
            elif "items" in old and direction == RESPONSE:
                found.append(f"{where}: array items are no longer described")
        return found

    # --- operations -----------------------------------------------------------------------

    def parameters(self, old_op: dict, new_op: dict, label: str):
        def index(doc, op):
            result = {}
            for param in op.get("parameters", []):
                param = self.resolve(doc, param)
                result[(param.get("in"), param.get("name"))] = param
            return result

        old_params, new_params = index(self.old, old_op), index(self.new, new_op)
        for key in sorted(set(old_params) - set(new_params), key=str):
            self.findings.append(f"{label}: {key[0]} parameter `{key[1]}` was removed")
        for key in sorted(set(new_params) - set(old_params), key=str):
            if new_params[key].get("required"):
                self.findings.append(f"{label}: new required {key[0]} parameter `{key[1]}`")
            else:
                self.additions.append(f"{label}: new optional {key[0]} parameter `{key[1]}`")
        for key in sorted(set(old_params) & set(new_params), key=str):
            old_p, new_p = old_params[key], new_params[key]
            if new_p.get("required") and not old_p.get("required"):
                self.findings.append(f"{label}: {key[0]} parameter `{key[1]}` became required")
            for message in self.schema(
                old_p.get("schema", {}), new_p.get("schema", {}), REQUEST, f"{label} {key[0]} parameter `{key[1]}`"
            ):
                self.findings.append(message)

    def media(self, old_content: dict, new_content: dict, direction: str, label: str):
        for media_type, old_media in old_content.items():
            if media_type not in new_content:
                self.findings.append(f"{label}: media type `{media_type}` is no longer offered")
                continue
            new_media = new_content[media_type]
            self.findings += self.schema(
                old_media.get("schema", {}), new_media.get("schema", {}), direction, f"{label} [{media_type}]"
            )
            # Server-sent events carry their payload schema in an extension, not in `schema`.
            if "x-sse-event-schema" in old_media:
                if "x-sse-event-schema" not in new_media:
                    self.findings.append(f"{label} [{media_type}]: the event payload schema is no longer described")
                else:
                    self.findings += self.schema(
                        old_media["x-sse-event-schema"],
                        new_media["x-sse-event-schema"],
                        direction,
                        f"{label} [{media_type}] event",
                    )

    @staticmethod
    def allowed(required: str, also: list | None) -> set[str]:
        """Scopes whose tokens may call an operation that needs `required`."""
        return {scope for scope in SCOPES if scope_grants(scope, required)} | set(also or [])

    def capabilities(self, old_op: dict, new_op: dict, label: str):
        old_scope, new_scope = old_op.get("x-required-capability"), new_op.get("x-required-capability")
        if old_scope and new_scope:
            lost = self.allowed(old_scope, old_op.get("x-capability-also-granted-by")) - self.allowed(
                new_scope, new_op.get("x-capability-also-granted-by")
            )
            if lost:
                self.findings.append(
                    f"{label}: now needs capability `{new_scope}`; tokens with {', '.join(sorted(lost))} are refused"
                )
        old_by_kind = old_op.get("x-capability-by-ticket-kind", {})
        new_by_kind = new_op.get("x-capability-by-ticket-kind", {})
        for kind, old_kind_scope in sorted(old_by_kind.items()):
            if kind not in new_by_kind:
                self.findings.append(f"{label}: ticket kind `{kind}` is no longer accepted")
                continue
            lost = self.allowed(old_kind_scope, None) - self.allowed(new_by_kind[kind], None)
            if lost:
                self.findings.append(
                    f"{label}: `{kind}` tickets now need capability `{new_by_kind[kind]}`; "
                    f"tokens with {', '.join(sorted(lost))} are refused"
                )

    def operation(self, old_op: dict, new_op: dict, label: str, old_item: dict, new_item: dict):
        self.capabilities(old_op, new_op, label)
        # Path-item parameters apply to every operation of the path unless the operation overrides.
        old_op = {**old_op, "parameters": old_item.get("parameters", []) + old_op.get("parameters", [])}
        new_op = {**new_op, "parameters": new_item.get("parameters", []) + new_op.get("parameters", [])}
        self.parameters(old_op, new_op, label)

        old_body = self.resolve(self.old, old_op["requestBody"]) if "requestBody" in old_op else None
        new_body = self.resolve(self.new, new_op["requestBody"]) if "requestBody" in new_op else None
        if old_body is None and new_body is not None and new_body.get("required"):
            self.findings.append(f"{label}: a request body is now required")
        elif old_body is not None and new_body is None:
            self.findings.append(f"{label}: the request body was removed")
        elif old_body is not None and new_body is not None:
            if new_body.get("required") and not old_body.get("required"):
                self.findings.append(f"{label}: the request body became required")
            self.media(old_body.get("content", {}), new_body.get("content", {}), REQUEST, f"{label} request")

        old_responses, new_responses = old_op.get("responses", {}), new_op.get("responses", {})
        for status, old_response in old_responses.items():
            if status not in new_responses:
                if status.startswith("2"):
                    self.findings.append(f"{label}: success status {status} is no longer returned")
                continue
            self.media(
                self.resolve(self.old, old_response).get("content", {}),
                self.resolve(self.new, new_responses[status]).get("content", {}),
                RESPONSE,
                f"{label} response {status}",
            )

    @staticmethod
    def reachable(doc: dict) -> set[str]:
        """Names of component schemas that some operation (transitively) refers to."""
        seen: set[str] = set()
        schemas = doc.get("components", {}).get("schemas", {})

        def visit(node):
            if isinstance(node, dict):
                target = node.get("$ref")
                if isinstance(target, str) and target.startswith("#/components/schemas/"):
                    name = target.rsplit("/", 1)[-1]
                    if name not in seen:
                        seen.add(name)
                        visit(schemas.get(name))
                for value in node.values():
                    visit(value)
            elif isinstance(node, list):
                for value in node:
                    visit(value)

        visit(doc.get("paths", {}))
        return seen

    def components(self) -> None:
        """Published schemas no operation refers to are still public (payload types)."""
        old_schemas = self.old.get("components", {}).get("schemas", {})
        new_schemas = self.new.get("components", {}).get("schemas", {})
        for name in sorted(set(old_schemas) - self.reachable(self.old)):
            if name not in new_schemas:
                self.findings.append(f"schema `{name}` was removed")
                continue
            self.findings += self.schema(
                {"$ref": f"#/components/schemas/{name}"},
                {"$ref": f"#/components/schemas/{name}"},
                RESPONSE,
                f"schema `{name}`",
            )

    def run(self) -> None:
        old_paths, new_paths = self.old.get("paths", {}), self.new.get("paths", {})
        for path, old_item in sorted(old_paths.items()):
            new_item = new_paths.get(path)
            if new_item is None:
                self.findings.append(f"{path}: path was removed")
                continue
            for method in METHODS:
                label = f"{method.upper()} {path}"
                if method in old_item and method not in new_item:
                    self.findings.append(f"{label}: method was removed")
                elif method in old_item:
                    self.operation(old_item[method], new_item[method], label, old_item, new_item)
                elif method in new_item:
                    self.additions.append(f"{label}: new operation")
        for path, new_item in sorted(new_paths.items()):
            if path not in old_paths:
                self.additions.append(f"{path}: new path")
        self.components()


def describe(schema: dict) -> str:
    """A short name for a union member: its discriminating const, or its title."""
    for name, prop in schema.get("properties", {}).items():
        if isinstance(prop, dict) and "const" in prop:
            return f"{name}={prop['const']}"
    return str(schema.get("title") or schema.get("const") or "")


def compare(old: dict, new: dict) -> tuple[list[str], list[str]]:
    differ = Differ(old, new)
    differ.run()
    return differ.findings, differ.additions


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--baseline", type=Path, default=DEFAULT_BASELINE)
    parser.add_argument("--current", type=Path, default=DEFAULT_CURRENT)
    parser.add_argument("--update", action="store_true", help="make the current document the new baseline")
    parser.add_argument("--allow-breaking", action="store_true", help="with --update: accept breaking changes")
    parser.add_argument("-v", "--verbose", action="store_true", help="also list additive changes")
    args = parser.parse_args(argv)
    try:
        old = json.loads(args.baseline.read_text())
        new = json.loads(args.current.read_text())
    except (OSError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2
    findings, additions = compare(old, new)
    for line in findings:
        print(f"BREAKING {line}")
    if args.verbose:
        for line in additions:
            print(f"additive {line}")
    if args.update:
        if findings and not args.allow_breaking:
            print("ERROR: refusing to update the baseline with breaking changes (use --allow-breaking)", file=sys.stderr)
            return 1
        shutil.copyfile(args.current, args.baseline)
        print(f"baseline updated: {args.baseline}")
        return 0
    if findings:
        print(
            f"ERROR: {len(findings)} breaking change(s) against {args.baseline.name}; add a new API version "
            "instead, or run with --update --allow-breaking when cutting a breaking release",
            file=sys.stderr,
        )
        return 1
    print(f"OK: {len(additions)} additive change(s), no breaking changes against {args.baseline.name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
