// Writes docs/specs/ui-api-map.md from the typed registry; `uiApiMap.test.ts` fails when it is stale.
import { readFileSync, writeFileSync } from "node:fs";
import { renderUiApiMap, type OpenApiDocument } from "../src/api/uiFeatures.ts";

const root = new URL("../../", import.meta.url);
const openapi = JSON.parse(readFileSync(new URL("docs/specs/openapi.json", root), "utf8")) as OpenApiDocument;
writeFileSync(new URL("docs/specs/ui-api-map.md", root), renderUiApiMap(openapi));
