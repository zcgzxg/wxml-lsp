import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { BACKEND_DIR, SAMPLES_DIR } from "../src/env.ts";
import { runScenario, type RunConfig } from "../src/run.ts";
import { scenarioById } from "../src/scenarios.ts";
import type { LayoutId, ScenarioSpec } from "../src/types.ts";

const config: RunConfig = { backendDir: BACKEND_DIR, samplesDir: SAMPLES_DIR };
const LAYOUTS: LayoutId[] = ["in-place-shadow", "symbol-bridge"];
const spec = scenarioById("page-basic");
const scriptPath = path.join(SAMPLES_DIR, spec.dir, spec.script);
const original = fs.readFileSync(scriptPath, "utf8");

function hoverOf(overlay: string, layout: LayoutId, scenario: ScenarioSpec = spec): { type?: string; doc?: string } {
  const result = runScenario(scenario, layout, config, { scriptText: overlay });
  const hover = result.hovers.find((entry) => entry.query === "user.name")?.observation;
  assert.ok(hover, "hover for user.name");
  return { type: hover.typeOnly, doc: hover.documentation };
}

test("unsaved type and JSDoc changes reach the template without touching disk", () => {
  const overlay = original.replace("/** 用户显示名称 */\n      name: 'Ada',", "/** 未保存的名称 */\n      name: 42,");
  assert.notEqual(overlay, original, "overlay must actually change the source");

  for (const layout of LAYOUTS) {
    const hover = hoverOf(overlay, layout);
    assert.equal(hover.type, "number", `${layout} type after overlay`);
    assert.equal(hover.doc, "未保存的名称", `${layout} documentation after overlay`);
  }

  assert.equal(fs.readFileSync(scriptPath, "utf8"), original, "the sample file must stay untouched");
});

test("JSDoc-only edits are visible without a type change", () => {
  const overlay = original.replace("/** 用户显示名称 */", "/** 未保存的名称说明 */");
  assert.notEqual(overlay, original, "overlay must actually change the source");

  for (const layout of LAYOUTS) {
    const hover = hoverOf(overlay, layout);
    assert.equal(hover.type, "string", `${layout} type stays string`);
    assert.equal(hover.doc, "未保存的名称说明", `${layout} documentation after doc-only overlay`);
  }
});

test("closing the overlay falls back to the original content", () => {
  const overlayed = original.replace("/** 用户显示名称 */\n      name: 'Ada',", "/** 未保存的名称 */\n      name: 42,");
  for (const layout of LAYOUTS) {
    assert.equal(hoverOf(overlayed, layout).type, "number");
    assert.equal(hoverOf(original, layout).type, "string", `${layout} falls back to the disk text`);
  }
});

test("unsaved changes in an imported model file propagate (AC-004)", () => {
  const scenario = scenarioById("cross-file-overlay");
  const modelsPath = path.join(SAMPLES_DIR, scenario.dir, "models.ts");
  const models = fs.readFileSync(modelsPath, "utf8");
  const overlay = models.replace("/** 用户显示名称 */\n  name: string;", "/** 未保存的名称 */\n  name: number;");
  assert.notEqual(overlay, models);

  for (const layout of LAYOUTS) {
    const before = runScenario(scenario, layout, config);
    assert.equal(before.hovers[0]?.observation?.typeOnly, "string", `${layout} baseline type`);
    assert.equal(before.hovers[0]?.observation?.documentation, "用户显示名称");

    const after = runScenario(scenario, layout, config, { files: { "models.ts": overlay } });
    assert.equal(after.hovers[0]?.observation?.typeOnly, "number", `${layout} overlay type`);
    assert.equal(after.hovers[0]?.observation?.documentation, "未保存的名称", `${layout} overlay documentation`);
    assert.equal(after.states.hover.status, "supported");
  }

  assert.equal(fs.readFileSync(modelsPath, "utf8"), models, "the imported model file must stay untouched");
});

test("a model file outside the snapshot is never read from disk", () => {
  const scenario = scenarioById("cross-file-overlay");
  // Without the `models.ts` snapshot entry the import must stay unresolved; a
  // silent disk read would produce the real `string` type again.
  const withoutModels: ScenarioSpec = { ...scenario, extraFiles: [] };
  for (const layout of LAYOUTS) {
    const run = runScenario(withoutModels, layout, config);
    const hover = run.hovers[0]?.observation;
    assert.ok(!hover || hover.typeOnly === "any" || hover.typeOnly === "unknown", `${layout} must not read models.ts from disk`);
  }
});
