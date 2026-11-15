#!/usr/bin/env node
/**
 * `npm run matrix` / `npm run matrix:json` / `npm run perf:json`
 *
 * Runs every scenario with both candidates and prints the comparison matrix or
 * raw JSON. Nothing is written to disk; use `scripts/emit-evidence.ts` to
 * refresh the committed data files.
 */

import { environmentFacts, BACKEND_DIR, SAMPLES_DIR } from "./env.ts";
import { measurePerformance } from "./perf.ts";
import { definitionsTable, diagnosticsTable, hoverTable, toMarkdown } from "./report.ts";
import { runScenario, type RunConfig } from "./run.ts";
import { SCENARIOS, scenarioById, wideScenario } from "./scenarios.ts";
import type { LayoutId, ScenarioRun } from "./types.ts";

const LAYOUTS: LayoutId[] = ["in-place-shadow", "symbol-bridge"];
const config: RunConfig = { backendDir: BACKEND_DIR, samplesDir: SAMPLES_DIR };
const json = process.argv.includes("--json");

if (process.argv.includes("--perf")) {
  const timings = [];
  for (const layout of LAYOUTS) {
    timings.push({
      sample: "page-basic",
      layout,
      ...measurePerformance(scenarioById("page-basic"), layout, config, {
        hoverIterations: 20,
        diagnosticIterations: 5,
      }),
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
  console.log(JSON.stringify({ environment: environmentFacts(), timings }, null, 2));
} else {
  const runs: ScenarioRun[] = [];
  for (const scenario of SCENARIOS) {
    for (const layout of LAYOUTS) {
      runs.push(runScenario(scenario, layout, config));
    }
  }
  if (json) {
    console.log(JSON.stringify({ environment: environmentFacts(), runs }, null, 2));
  } else {
    const env = environmentFacts();
    console.log("# type-bridge matrix\n");
    console.log(
      `environment: node ${env.node}, typescript ${env.typescript}, miniprogram-api-typings ${env.typings}, ${env.platform}, ${env.cpu} x${env.cpuCount}\n`,
    );
    console.log(toMarkdown(runs));
    console.log("\n## hover\n");
    console.log(hoverTable(runs));
    console.log("\n## diagnostics\n");
    console.log(diagnosticsTable(runs));
    console.log("\n## definitions\n");
    console.log(definitionsTable(runs));
  }
}
