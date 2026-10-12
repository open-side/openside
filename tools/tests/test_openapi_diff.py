"""Tests for tools/openapi-diff.py (the file name has a hyphen, so it is loaded by path)."""

import contextlib
import copy
import importlib.util
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "openapi-diff.py"
spec = importlib.util.spec_from_file_location("openapi_diff", SCRIPT)
od = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = od
spec.loader.exec_module(od)

ROOT = Path(__file__).resolve().parents[2]


def ref(name):
    return {"$ref": f"#/components/schemas/{name}"}


def op(*, scope="read", params=None, body=None, body_required=True, responses=None):
    operation = {
        "x-required-capability": scope,
        "responses": responses
        or {"200": {"description": "ok", "content": {"application/json": {"schema": ref("Thing")}}}},
    }
    if params:
        operation["parameters"] = params
    if body is not None:
        operation["requestBody"] = {
            "required": body_required,
            "content": {"application/json": {"schema": body}},
        }
    return operation


def thing(**extra):
    schema = {
        "type": "object",
        "properties": {"id": {"type": "string"}, "name": {"type": "string"}},
        "required": ["id", "name"],
    }
    schema.update(extra)
    return schema


def document(paths=None, schemas=None):
    return {
        "openapi": "3.1.0",
        "info": {"title": "t", "version": "v1"},
        "paths": paths if paths is not None else {"/v1/things": {"get": op()}},
        "components": {"schemas": schemas if schemas is not None else {"Thing": thing()}},
    }


def breaking(old, new):
    return od.compare(old, new)[0]


class BreakingChanges(unittest.TestCase):
    def assertBreaks(self, old, new, *needles):
        found = breaking(old, new)
        self.assertTrue(found, "expected a breaking change")
        text = "\n".join(found)
        for needle in needles:
            self.assertIn(needle, text)

    def test_removed_path_and_method(self):
        old = document({"/v1/things": {"get": op(), "post": op(scope="steer")}, "/v1/other": {"get": op()}})
        gone_path = copy.deepcopy(old)
        del gone_path["paths"]["/v1/other"]
        self.assertBreaks(old, gone_path, "/v1/other", "path was removed")
        gone_method = copy.deepcopy(old)
        del gone_method["paths"]["/v1/things"]["post"]
        self.assertBreaks(old, gone_method, "POST /v1/things", "method was removed")

    def test_removed_response_field(self):
        old = document()
        new = document(schemas={"Thing": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}})
        self.assertBreaks(old, new, "response field `name` was removed")

    def test_response_field_no_longer_always_present(self):
        old = document()
        new = document(schemas={"Thing": thing(required=["id"])})
        self.assertBreaks(old, new, "`name` is no longer always present")

    def test_nested_response_field_removal_is_found_through_refs_and_arrays(self):
        schemas = {
            "Thing": {"type": "object", "properties": {"items": {"type": "array", "items": ref("Item")}}, "required": ["items"]},
            "Item": {"type": "object", "properties": {"a": {"type": "string"}, "b": {"type": "string"}}, "required": ["a"]},
        }
        new = copy.deepcopy(schemas)
        del new["Item"]["properties"]["b"]
        self.assertBreaks(document(schemas=schemas), document(schemas=new), "items[]", "`b` was removed")

    def test_new_required_request_field(self):
        body = {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}
        old = document({"/v1/things": {"post": op(scope="steer", body=ref("Make"))}}, {"Thing": thing(), "Make": body})
        grown = copy.deepcopy(body)
        grown["properties"]["b"] = {"type": "string"}
        grown["required"] = ["a", "b"]
        new = document({"/v1/things": {"post": op(scope="steer", body=ref("Make"))}}, {"Thing": thing(), "Make": grown})
        self.assertBreaks(old, new, "new required request field `b`")

    def test_new_required_parameter_and_body(self):
        old = document({"/v1/things": {"get": op()}})
        with_param = copy.deepcopy(old)
        with_param["paths"]["/v1/things"]["get"]["parameters"] = [
            {"name": "q", "in": "query", "required": True, "schema": {"type": "string"}}
        ]
        self.assertBreaks(old, with_param, "new required query parameter `q`")
        with_body = copy.deepcopy(old)
        with_body["paths"]["/v1/things"]["get"]["requestBody"] = {
            "required": True,
            "content": {"application/json": {"schema": {"type": "object"}}},
        }
        self.assertBreaks(old, with_body, "a request body is now required")

    def test_optional_parameter_becoming_required_or_removed(self):
        param = {"name": "q", "in": "query", "required": False, "schema": {"type": "string"}}
        old = document({"/v1/things": {"get": op(params=[param])}})
        required = copy.deepcopy(old)
        required["paths"]["/v1/things"]["get"]["parameters"][0]["required"] = True
        self.assertBreaks(old, required, "became required")
        removed = copy.deepcopy(old)
        del removed["paths"]["/v1/things"]["get"]["parameters"]
        self.assertBreaks(old, removed, "`q` was removed")

    def test_narrowed_request_enum(self):
        mode = {"type": "string", "enum": ["a", "b", "c"]}
        old = document({"/v1/things": {"post": op(scope="steer", body={"type": "object", "properties": {"mode": mode}})}})
        new = copy.deepcopy(old)
        new["paths"]["/v1/things"]["post"]["requestBody"]["content"]["application/json"]["schema"]["properties"]["mode"][
            "enum"
        ] = ["a", "b"]
        self.assertBreaks(old, new, "enum value(s) no longer accepted: 'c'")

    def test_request_enum_written_as_one_of_consts_loses_a_member(self):
        enum = {"oneOf": [{"const": "a", "type": "string"}, {"const": "b", "type": "string"}]}
        old = document(
            {"/v1/things": {"post": op(scope="steer", body=ref("Mode"))}},
            {"Thing": thing(), "Mode": enum},
        )
        new = copy.deepcopy(old)
        new["components"]["schemas"]["Mode"] = {"oneOf": [{"const": "a", "type": "string"}]}
        self.assertBreaks(old, new, "union member `b`")

    def test_response_enum_value_removed(self):
        old = document(schemas={"Thing": {"type": "object", "properties": {"s": {"type": "string", "enum": ["x", "y"]}}}})
        new = document(schemas={"Thing": {"type": "object", "properties": {"s": {"type": "string", "enum": ["x"]}}}})
        self.assertBreaks(old, new, "enum value(s) no longer produced: 'y'")

    def test_changed_types(self):
        def doc(request_type, response_type):
            return document(
                {
                    "/v1/things": {
                        "post": op(
                            scope="steer",
                            body={"type": "object", "properties": {"n": {"type": request_type}}},
                            responses={
                                "200": {
                                    "description": "ok",
                                    "content": {
                                        "application/json": {
                                            "schema": {"type": "object", "properties": {"n": {"type": response_type}}}
                                        }
                                    },
                                }
                            },
                        )
                    }
                }
            )

        self.assertBreaks(doc("string", "string"), doc("integer", "string"), "type changed")
        self.assertBreaks(doc("string", "string"), doc("string", "integer"), "type changed")
        # integer -> number: requests accept more, but responses may now carry fractions.
        self.assertEqual(breaking(doc("integer", "integer"), doc("number", "integer")), [])
        self.assertBreaks(doc("integer", "integer"), doc("integer", "number"), "type changed")

    def test_nullability_changes(self):
        old = document(schemas={"Thing": {"type": "object", "properties": {"s": {"type": "string"}}}})
        nullable = document(schemas={"Thing": {"type": "object", "properties": {"s": {"type": ["string", "null"]}}}})
        self.assertBreaks(old, nullable, "type changed")
        any_of = document(
            schemas={"Thing": {"type": "object", "properties": {"s": {"anyOf": [{"type": "string"}, {"type": "null"}]}}}}
        )
        self.assertBreaks(old, any_of, "may now be null")
        # A request that used to accept null must keep doing so.
        accepting = document(
            {"/v1/things": {"post": op(scope="steer", body={"type": "object", "properties": {"s": {"type": ["string", "null"]}}})}}
        )
        refusing = copy.deepcopy(accepting)
        refusing["paths"]["/v1/things"]["post"]["requestBody"]["content"]["application/json"]["schema"]["properties"][
            "s"
        ]["type"] = "string"
        self.assertBreaks(accepting, refusing, "type changed")

    def test_success_status_changed(self):
        old = document({"/v1/things": {"post": op(scope="steer", responses={"201": {"description": "created"}})}})
        new = document({"/v1/things": {"post": op(scope="steer", responses={"200": {"description": "ok"}})}})
        self.assertBreaks(old, new, "success status 201 is no longer returned")

    def test_response_union_loses_a_member(self):
        union = {
            "oneOf": [
                {"type": "object", "properties": {"kind": {"const": "a"}}},
                {"type": "object", "properties": {"kind": {"const": "b"}}},
            ]
        }
        old = document(schemas={"Thing": union})
        new = copy.deepcopy(old)
        new["components"]["schemas"]["Thing"]["oneOf"].pop()
        self.assertBreaks(old, new, "union member `kind=b`")

    def test_const_changed(self):
        old = document(schemas={"Thing": {"type": "object", "properties": {"kind": {"const": "a"}}}})
        new = document(schemas={"Thing": {"type": "object", "properties": {"kind": {"const": "z"}}}})
        self.assertBreaks(old, new, "const changed")

    def test_removed_request_field_is_breaking_only_when_unknown_fields_are_rejected(self):
        def doc(closed):
            body = {"type": "object", "properties": {"a": {"type": "string"}}}
            if closed:
                body["additionalProperties"] = False
            return document({"/v1/things": {"post": op(scope="steer", body=body)}})

        old = copy.deepcopy(doc(False))
        old["paths"]["/v1/things"]["post"]["requestBody"]["content"]["application/json"]["schema"]["properties"]["b"] = {
            "type": "string"
        }
        self.assertEqual(breaking(old, doc(False)), [])
        closed_old = copy.deepcopy(old)
        self.assertBreaks(closed_old, doc(True), "request field `b` was removed")

    def test_capability_scope_tightening(self):
        old = document({"/v1/things": {"get": op(scope="read")}})
        stricter = document({"/v1/things": {"get": op(scope="admin")}})
        self.assertBreaks(old, stricter, "now needs capability `admin`")
        # steer already grants browse, and admin grants everything: no token is newly refused.
        self.assertEqual(
            breaking(document({"/v1/things": {"get": op(scope="steer")}}), document({"/v1/things": {"get": op(scope="browse")}})),
            [],
        )
        self.assertBreaks(
            document({"/v1/things": {"get": op(scope="browse")}}),
            document({"/v1/things": {"get": op(scope="steer")}}),
            "now needs capability `steer`",
        )

    def test_recursive_schemas_terminate(self):
        node = {"type": "object", "properties": {"child": ref("Node"), "v": {"type": "string"}}, "required": ["v"]}
        old = document(schemas={"Thing": ref("Node"), "Node": node})
        new = copy.deepcopy(old)
        del new["components"]["schemas"]["Node"]["properties"]["v"]
        self.assertBreaks(old, new, "`v` was removed")
        self.assertEqual(breaking(old, copy.deepcopy(old)), [])


class ReviewedGaps(unittest.TestCase):
    def assertBreaks(self, old, new, *needles):
        text = "\n".join(breaking(old, new))
        self.assertTrue(text, "expected a breaking change")
        for needle in needles:
            self.assertIn(needle, text)

    def sse_document(self, event):
        item = op(
            responses={
                "200": {
                    "description": "ok",
                    "content": {
                        "text/event-stream": {"schema": {"type": "string"}, "x-sse-event-schema": ref("Event")}
                    },
                }
            }
        )
        return document({"/v1/events": {"get": item}}, {"Thing": thing(), "Event": event})

    def test_sse_event_schema_regressions_are_detected(self):
        old_event = {
            "type": "object",
            "properties": {"seq": {"type": "integer"}, "kind": {"type": "string"}},
            "required": ["seq", "kind"],
        }
        new_event = copy.deepcopy(old_event)
        del new_event["properties"]["seq"]
        self.assertBreaks(self.sse_document(old_event), self.sse_document(new_event), "event", "`seq` was removed")
        grown = copy.deepcopy(old_event)
        grown["properties"]["extra"] = {"type": "string"}
        self.assertEqual(breaking(self.sse_document(old_event), self.sse_document(grown)), [])
        plain = self.sse_document(old_event)
        del plain["paths"]["/v1/events"]["get"]["responses"]["200"]["content"]["text/event-stream"]["x-sse-event-schema"]
        self.assertBreaks(self.sse_document(old_event), plain, "event payload schema is no longer described")

    def test_unreferenced_component_schemas_are_compared(self):
        old = document(schemas={"Thing": thing(), "Orphan": {"type": "object", "properties": {"a": {"type": "string"}}}})
        removed_field = copy.deepcopy(old)
        del removed_field["components"]["schemas"]["Orphan"]["properties"]["a"]
        self.assertBreaks(old, removed_field, "schema `Orphan`", "`a` was removed")
        removed = copy.deepcopy(old)
        del removed["components"]["schemas"]["Orphan"]
        self.assertBreaks(old, removed, "schema `Orphan` was removed")

    def test_union_with_shared_properties_is_checked_beyond_the_variants(self):
        def doc(properties, required):
            return document(
                schemas={
                    "Thing": {
                        "type": "object",
                        "properties": properties,
                        "required": required,
                        "oneOf": [{"properties": {"kind": {"const": "a"}}}],
                    }
                }
            )

        old = doc({"seq": {"type": "integer"}, "ts": {"type": "string"}}, ["seq", "ts"])
        self.assertBreaks(old, doc({"ts": {"type": "string"}}, ["ts"]), "`seq` was removed")
        self.assertBreaks(old, doc({"seq": {"type": "integer"}, "ts": {"type": "string"}}, ["seq"]), "`ts` is no longer always present")

    def test_response_may_not_widen_from_a_single_shape_to_a_union(self):
        old = document(schemas={"Thing": {"type": "object", "properties": {"v": {"type": "string"}}}})
        new = document(
            schemas={"Thing": {"type": "object", "properties": {"v": {"oneOf": [{"type": "string"}, {"type": "integer"}]}}}}
        )
        self.assertBreaks(old, new, "response may now also be")
        # Narrowing a union to one of its old members is fine.
        self.assertEqual(breaking(new, old), [])

    def test_request_wildcard_may_not_become_typed(self):
        def doc(value):
            return document(
                {"/v1/things": {"post": op(scope="steer", body={"type": "object", "properties": {"value": value}})}}
            )

        self.assertBreaks(doc({"description": "any"}), doc({"type": "boolean"}), "used to be unconstrained")
        self.assertEqual(breaking(doc({"type": "boolean"}), doc({"description": "any"})), [])

    def test_capability_extensions(self):
        def doc(**extra):
            item = op(scope="browse")
            item.update(extra)
            return document({"/v1/things": {"post": item}})

        also = doc(**{"x-capability-also-granted-by": ["steer", "vault_use"]})
        self.assertBreaks(also, doc(**{"x-capability-also-granted-by": ["steer"]}), "vault_use")
        self.assertEqual(breaking(doc(**{"x-capability-also-granted-by": ["steer"]}), also), [])
        kinds = doc(**{"x-capability-by-ticket-kind": {"confirm": "approve", "takeover": "human_step"}})
        self.assertBreaks(
            kinds,
            doc(**{"x-capability-by-ticket-kind": {"confirm": "admin", "takeover": "human_step"}}),
            "`confirm` tickets now need capability `admin`",
        )
        self.assertBreaks(kinds, doc(**{"x-capability-by-ticket-kind": {"confirm": "approve"}}), "`takeover` is no longer accepted")

    def test_all_of_keeps_top_level_properties_and_required(self):
        def doc(own):
            return document(
                schemas={
                    "Thing": {
                        "allOf": [{"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}],
                        **own,
                    }
                }
            )

        old = doc({"properties": {"b": {"type": "string"}}, "required": ["b"]})
        self.assertBreaks(old, doc({"properties": {}}), "`b` was removed")
        self.assertBreaks(old, doc({"properties": {"b": {"type": "string"}}}), "`b` is no longer always present")
        self.assertEqual(breaking(old, copy.deepcopy(old)), [])

    def test_openapi_30_nullable(self):
        old = document(schemas={"Thing": {"type": "object", "properties": {"s": {"type": "string"}}}})
        new = document(schemas={"Thing": {"type": "object", "properties": {"s": {"type": "string", "nullable": True}}}})
        self.assertBreaks(old, new, "may now be null")

    def test_path_item_parameters_count(self):
        param = {"name": "id", "in": "path", "required": True, "schema": {"type": "string"}}
        old = document({"/v1/things/{id}": {"parameters": [param], "get": op()}})
        new = document({"/v1/things/{id}": {"get": op()}})
        self.assertBreaks(old, new, "path parameter `id` was removed")
        retyped = copy.deepcopy(old)
        retyped["paths"]["/v1/things/{id}"]["parameters"][0]["schema"] = {"type": "integer"}
        self.assertBreaks(old, retyped, "type changed")
        added = copy.deepcopy(old)
        added["paths"]["/v1/things/{id}"]["parameters"].append({"name": "q", "in": "query", "required": True, "schema": {"type": "string"}})
        self.assertBreaks(old, added, "new required query parameter `q`")

    def test_refs_to_shared_request_bodies_and_responses_are_followed(self):
        def doc(body_fields, response_fields):
            result = document(
                {
                    "/v1/things": {
                        "post": {
                            "x-required-capability": "steer",
                            "requestBody": {"$ref": "#/components/requestBodies/Make"},
                            "responses": {"200": {"$ref": "#/components/responses/Made"}},
                        }
                    }
                },
                {
                    "Make": {"type": "object", "properties": body_fields, "required": list(body_fields)},
                    "Made": {"type": "object", "properties": response_fields, "required": list(response_fields)},
                },
            )
            result["components"]["requestBodies"] = {
                "Make": {"required": True, "content": {"application/json": {"schema": ref("Make")}}}
            }
            result["components"]["responses"] = {
                "Made": {"description": "ok", "content": {"application/json": {"schema": ref("Made")}}}
            }
            return result

        string = {"type": "string"}
        self.assertBreaks(doc({"a": string}, {"x": string}), doc({"a": string, "b": string}, {"x": string}), "new required request field `b`")
        self.assertBreaks(doc({"a": string}, {"x": string, "y": string}), doc({"a": string}, {"x": string}), "response field `y` was removed")

    def test_real_event_stream_regressions_are_detected(self):
        baseline = json.loads((ROOT / "docs/specs/openapi.baseline.json").read_text())
        mutated = copy.deepcopy(baseline)
        event = mutated["components"]["schemas"]["Event"]
        event["properties"].pop("seq")
        found = breaking(baseline, mutated)
        self.assertTrue(any("seq" in line for line in found), found)
        mutated = copy.deepcopy(baseline)
        kinds = mutated["components"]["schemas"]["EventKind"]
        kinds["oneOf"].pop()
        self.assertTrue(breaking(baseline, mutated))


class AdditiveChanges(unittest.TestCase):
    def assertAdditive(self, old, new):
        self.assertEqual(breaking(old, new), [])

    def test_new_paths_methods_and_parameters(self):
        old = document()
        new = document({"/v1/things": {"get": op(params=[{"name": "q", "in": "query", "schema": {"type": "string"}}]), "post": op(scope="steer")}, "/v1/new": {"get": op()}})
        self.assertAdditive(old, new)
        _, additions = od.compare(old, new)
        self.assertTrue(any("/v1/new" in line for line in additions))
        self.assertTrue(any("POST /v1/things" in line for line in additions))

    def test_new_response_field_and_optional_request_field(self):
        grown_thing = thing()
        grown_thing["properties"]["extra"] = {"type": "string"}
        body = {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}
        grown_body = copy.deepcopy(body)
        grown_body["properties"]["opt"] = {"type": "string"}
        old = document({"/v1/things": {"post": op(scope="steer", body=body)}})
        new = document({"/v1/things": {"post": op(scope="steer", body=grown_body)}}, {"Thing": grown_thing})
        self.assertAdditive(old, new)

    def test_enum_and_union_growth(self):
        old = document(
            {"/v1/things": {"post": op(scope="steer", body={"type": "string", "enum": ["a"]})}},
            {"Thing": {"type": "object", "properties": {"s": {"type": "string", "enum": ["x"]}}}},
        )
        new = copy.deepcopy(old)
        new["paths"]["/v1/things"]["post"]["requestBody"]["content"]["application/json"]["schema"]["enum"] = ["a", "b"]
        new["components"]["schemas"]["Thing"]["properties"]["s"]["enum"] = ["x", "y"]
        self.assertAdditive(old, new)
        union_old = document(schemas={"Thing": {"oneOf": [{"type": "object", "properties": {"kind": {"const": "a"}}}]}})
        union_new = copy.deepcopy(union_old)
        union_new["components"]["schemas"]["Thing"]["oneOf"].append({"type": "object", "properties": {"kind": {"const": "b"}}})
        self.assertAdditive(union_old, union_new)

    def test_loosened_scope_widened_request_and_optional_response_to_required(self):
        old = document({"/v1/things": {"post": op(scope="admin", body={"type": "object", "properties": {"n": {"type": "integer"}}})}})
        new = document({"/v1/things": {"post": op(scope="read", body={"type": "object", "properties": {"n": {"type": ["integer", "string"]}}})}})
        self.assertAdditive(old, new)
        optional = document(schemas={"Thing": thing(required=["id"])})
        always = document(schemas={"Thing": thing(required=["id", "name"])})
        self.assertAdditive(optional, always)

    def test_new_error_responses_and_descriptions_are_ignored(self):
        old = document()
        new = copy.deepcopy(old)
        new["paths"]["/v1/things"]["get"]["responses"]["404"] = {"description": "missing"}
        new["paths"]["/v1/things"]["get"]["summary"] = "reworded"
        self.assertAdditive(old, new)


class CommittedDocuments(unittest.TestCase):
    def load(self, name):
        return json.loads((ROOT / "docs/specs" / name).read_text())

    def test_current_openapi_does_not_break_the_baseline(self):
        findings = breaking(self.load("openapi.baseline.json"), self.load("openapi.json"))
        self.assertEqual(findings, [], "\n".join(findings))

    def test_real_document_regressions_are_detected(self):
        baseline = self.load("openapi.baseline.json")

        def mutate(change):
            document = copy.deepcopy(baseline)
            change(document)
            return breaking(baseline, document)

        schemas = lambda document: document["components"]["schemas"]
        self.assertTrue(mutate(lambda d: d["paths"].pop("/v1/sessions")))
        self.assertTrue(mutate(lambda d: d["paths"]["/v1/sessions"].pop("post")))
        self.assertTrue(mutate(lambda d: schemas(d)["Session"]["properties"].pop("title")))
        self.assertTrue(mutate(lambda d: schemas(d)["CreateSessionRequest"].__setitem__("required", ["model"])))
        self.assertTrue(mutate(lambda d: schemas(d)["Session"]["properties"]["id"].clear()))
        self.assertTrue(mutate(lambda d: schemas(d)["PolicyMode"].__setitem__("oneOf", schemas(d)["PolicyMode"]["oneOf"][:-1])))


class CommandLine(unittest.TestCase):
    def run_main(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = od.main(list(argv))
        return code, out.getvalue(), err.getvalue()

    def files(self, old, new):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        base, cur = Path(tmp.name) / "base.json", Path(tmp.name) / "cur.json"
        base.write_text(json.dumps(old))
        cur.write_text(json.dumps(new))
        return base, cur

    def test_exit_codes_and_output(self):
        old = document({"/v1/things": {"get": op()}, "/v1/gone": {"get": op()}})
        new = document({"/v1/things": {"get": op()}, "/v1/added": {"get": op()}})
        base, cur = self.files(old, new)
        code, out, err = self.run_main("--baseline", str(base), "--current", str(cur))
        self.assertEqual(code, 1)
        self.assertIn("BREAKING /v1/gone: path was removed", out)
        self.assertIn("1 breaking change", err)
        good_base, good_cur = self.files(old, copy.deepcopy(old))
        code, out, _ = self.run_main("--baseline", str(good_base), "--current", str(good_cur))
        self.assertEqual(code, 0)
        self.assertIn("OK:", out)
        code, _, err = self.run_main("--baseline", str(good_base), "--current", str(good_base.parent / "missing.json"))
        self.assertEqual(code, 2)
        self.assertIn("ERROR", err)

    def test_update_refuses_breaking_changes_unless_allowed(self):
        old = document({"/v1/things": {"get": op()}, "/v1/gone": {"get": op()}})
        new = document({"/v1/things": {"get": op()}})
        base, cur = self.files(old, new)
        code, _, err = self.run_main("--baseline", str(base), "--current", str(cur), "--update")
        self.assertEqual(code, 1)
        self.assertIn("--allow-breaking", err)
        self.assertEqual(json.loads(base.read_text()), old)
        code, out, _ = self.run_main("--baseline", str(base), "--current", str(cur), "--update", "--allow-breaking")
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(base.read_text()), new)
        added = document({"/v1/things": {"get": op()}, "/v1/more": {"get": op()}})
        cur.write_text(json.dumps(added))
        code, _, _ = self.run_main("--baseline", str(base), "--current", str(cur), "--update")
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(base.read_text()), added)


if __name__ == "__main__":
    unittest.main()
