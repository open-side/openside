"""Tests for tools/gen-api-docs.py (the file name has a hyphen, so it is loaded by path)."""

import importlib.util
import json
import sys
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "gen-api-docs.py"
spec = importlib.util.spec_from_file_location("gen_api_docs", SCRIPT)
gen = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = gen
spec.loader.exec_module(gen)

ROOT = Path(__file__).resolve().parents[2]

DOCUMENT = {
    "openapi": "3.1.0",
    "info": {"title": "demo API", "version": "v9"},
    "paths": {
        "/v1/widgets/{id}": {
            "get": {
                "x-required-capability": "read",
                "parameters": [
                    {"name": "id", "in": "path", "required": True, "schema": {"type": "string"}},
                    {"name": "q", "in": "query", "description": "x", "schema": {"type": ["string", "null"]}},
                ],
                "responses": {
                    "200": {
                        "description": "Success",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Widget"}}},
                    }
                },
            }
        }
    },
    "components": {
        "schemas": {
            "Widget": {
                "description": "A | piped\ndescription.",
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "tags": {"type": "array", "items": {"type": "string"}},
                    "kind": {"$ref": "#/components/schemas/Kind"},
                },
                "required": ["id"],
            },
            "Kind": {"oneOf": [{"const": "a", "type": "string", "description": "first"}]},
        },
        "securitySchemes": {"tok": {"type": "http", "scheme": "bearer", "description": "A token."}},
    },
}


class Rendering(unittest.TestCase):
    def test_operation_schema_and_cross_references(self):
        text = gen.render(DOCUMENT)
        self.assertIn("### GET /v1/widgets/{id}", text)
        self.assertIn("Required capability: `read`", text)
        self.assertIn("| `id` | path | string | yes |  |", text)
        self.assertIn("| `q` | query | string \\| null | no | x |", text)
        self.assertIn("`application/json` [Widget](#widget)", text)
        self.assertIn("| `id` | string | yes |  |", text)
        self.assertIn("| `tags` | string[] | no |  |", text)
        self.assertIn("| `kind` | [Kind](#kind) | no |  |", text)
        self.assertIn("| `\"a\"` | first |", text)
        self.assertIn("A \\| piped description.", text)

    def test_union_schema_keeps_its_shared_fields(self):
        document = json.loads(json.dumps(DOCUMENT))
        document["components"]["schemas"]["Envelope"] = {
            "type": "object",
            "properties": {"seq": {"type": "integer", "description": "Order."}},
            "required": ["seq"],
            "oneOf": [{"type": "object", "properties": {"kind": {"const": "a"}}, "required": ["kind"]}],
        }
        text = gen.render(document)
        section = text[text.index("### Envelope") :]
        self.assertIn("Common fields:", section)
        self.assertIn("| `seq` | integer | yes | Order. |", section)
        self.assertIn("- `kind=a`", section)

    def test_output_changes_when_the_schema_changes(self):
        changed = json.loads(json.dumps(DOCUMENT))
        changed["components"]["schemas"]["Widget"]["properties"]["extra"] = {"type": "integer"}
        self.assertNotEqual(gen.render(DOCUMENT), gen.render(changed))


class CommittedReference(unittest.TestCase):
    def test_committed_api_reference_matches_the_openapi_snapshot(self):
        document = json.loads((ROOT / "docs/specs/openapi.json").read_text())
        committed = (ROOT / "docs/reference/api.md").read_text()
        self.assertEqual(
            committed,
            gen.render(document),
            "docs/reference/api.md is stale: run tools/gen-docs.sh",
        )

    def test_every_route_is_documented(self):
        document = json.loads((ROOT / "docs/specs/openapi.json").read_text())
        committed = (ROOT / "docs/reference/api.md").read_text()
        for path, item in document["paths"].items():
            for method in item:
                self.assertIn(f"### {method.upper()} {path}\n", committed)


if __name__ == "__main__":
    unittest.main()
