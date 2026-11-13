import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import Ajv2020 from "ajv/dist/2020.js";

const load = async (path) => JSON.parse(await readFile(new URL(path, import.meta.url), "utf8"));
const schema = await load("../protocol/schemas/configuration-v1.schema.json");
const base = await load("../protocol/fixtures/configuration.json");
const cases = await load("../protocol/fixtures/configuration-cases.json");
const validate = new Ajv2020({ allErrors: true, strict: true }).compile(schema);

for (const test of cases) {
  const document = structuredClone(base);
  for (const [path, value] of Object.entries(test.set ?? {})) {
    const keys = path.split(".");
    const key = keys.pop();
    const parent = keys.reduce((object, part) => object[part], document);
    parent[key] = value;
  }
  for (const path of test.remove ?? []) {
    const keys = path.split(".");
    const key = keys.pop();
    const parent = keys.reduce((object, part) => object[part], document);
    delete parent[key];
  }
  assert.equal(validate(document), test.valid, test.name + ": " + JSON.stringify(validate.errors));
}
console.log("JSON Schema: " + cases.length + " shared cases passed");
