#!/usr/bin/env node
/**
 * Refresh the committed evidence data files.
 *
 *   npm run evidence
 *
 * Runs the full matrix and the performance probe, sanitizes absolute paths, and
 * writes only inside `<repo>/docs/evidence/data/type-bridge-c1/`.
 */

import fs from "node:fs";
import path from "node:path";
import { BACKEND_DIR, environmentFacts, SAMPLES_DIR } from "../src/env.ts";
import { assertEvidenceDir, sanitizeEvidence, EVIDENCE_RELATIVE_DIR } from "../src/evidence.ts";
import { measurePerformance } from "../src/perf.ts";
import { runScenario, type RunConfig } from "../src/run.ts";
import { SCENARIOS, scenarioById, wideScenario } from "../src/scenarios.ts";
import type { LayoutId, ScenarioRun } from "../src/types.ts";

const LAYOUTS: LayoutId[] = ["in-place-shadow", "symbol-bridge"];
const config: RunConfig = { backendDir: BACKEND_DIR, samplesDir: SAMPLES_DIR };
const repoRoot = path.resolve(BACKEND_DIR, "../..");
const outputDir = path.resolve(repoRoot, EVIDENCE_RELATIVE_DIR);

const runs: ScenarioRun[] = [];
for (const scenario of SCENARIOS) {
  for (const layout of LAYOUTS) {
    runs.push(runScenario(scenario, layout, config));
  }
}

const timings = [];
for (const layout of LAYOUTS) {
  timings.push({
    sample: "page-basic",
    layout,
    ...measurePerformance(scenarioById("page-basic"), layout, config, { hoverIterations: 20, diagnosticIterations: 5 }),
  });
}
const wide = wideScenario(40);
for (const layout of LAYOUTS) {
  timings.push({
    sample: wide.spec.id,
    layout,
    ...measurePerformance(
      wide.spec,
      layout,
      config,
      { hoverIterations: 5, diagnosticIterations: 5 },
      { scriptText: wide.scriptText },
    ),
  });
}

assertEvidenceDir(outputDir, repoRoot);
fs.mkdirSync(outputDir, { recursive: true });
const write = (name: string, value: unknown): void => {
  const sanitized = sanitizeEvidence(value, { backendDir: BACKEND_DIR, repoRoot });
  fs.writeFileSync(path.join(outputDir, name), `${JSON.stringify(sanitized, null, 2)}\n`);
};
write("matrix.json", { environment: environmentFacts(), runs });
write("perf.json", { environment: environmentFacts(), timings });
console.log(`wrote ${runs.length} runs and ${timings.length} timings to ${path.relative(repoRoot, outputDir)}`);
