import assert from "node:assert/strict";
import { test } from "node:test";
import { BACKEND_DIR, SAMPLES_DIR } from "../src/env.ts";
import { measurePerformance } from "../src/perf.ts";
import type { RunConfig } from "../src/run.ts";
import { scenarioById, wideScenario } from "../src/scenarios.ts";
import type { LayoutId } from "../src/types.ts";

const config: RunConfig = { backendDir: BACKEND_DIR, samplesDir: SAMPLES_DIR };
const LAYOUTS: LayoutId[] = ["in-place-shadow", "symbol-bridge"];

test("rough timings for cold start, hover and diagnostics", () => {
  for (const layout of LAYOUTS) {
    const timing = measurePerformance(scenarioById("page-basic"), layout, config, {
      hoverIterations: 20,
      diagnosticIterations: 5,
    });
    console.log(`[perf] page-basic / ${layout}`, timing);
    assert.ok(timing.coldStartMs < 8000, `cold start ${timing.coldStartMs} ms`);
    assert.ok(timing.hoverP50Ms < 500, `hover p50 ${timing.hoverP50Ms} ms`);
    assert.ok(timing.diagnosticsMs < 8000, `diagnostics ${timing.diagnosticsMs} ms`);
  }

  const { spec, scriptText } = wideScenario(40);
  for (const layout of LAYOUTS) {
    const timing = measurePerformance(
      spec,
      layout,
      config,
      { hoverIterations: 5, diagnosticIterations: 5 },
      { scriptText },
    );
    console.log(`[perf] wide-40 / ${layout}`, timing);
    assert.equal(timing.samples, 40 * 5);
    assert.ok(timing.hoverP50Ms < 500, `wide hover p50 ${timing.hoverP50Ms} ms`);
  }
});
