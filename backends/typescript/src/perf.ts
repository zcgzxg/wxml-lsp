/**
 * Rough performance measurement.
 *
 * The measurement goes through the same request path as a real run
 * (`inspectRequest` + `inspectLayout`), so the numbers include request building,
 * option conversion and program construction. They are a sanity check, not a
 * benchmark: the samples are tiny and the host is in-memory.
 */

import path from "node:path";
import { defaultReadPolicy, VirtualProject } from "./compiler.ts";
import { hoverAt, semanticDiagnostics } from "./queries.ts";
import { toCompilerOptions } from "./protocol.ts";
import { inspectLayout, inspectRequest, type RunConfig } from "./run.ts";
import type { LayoutId, RunOverrides, ScenarioSpec, TimingObservation } from "./types.ts";

export interface PerfOptions {
  hoverIterations: number;
  diagnosticIterations: number;
}

export function measurePerformance(
  spec: ScenarioSpec,
  layoutId: LayoutId,
  config: RunConfig,
  options: PerfOptions = { hoverIterations: 30, diagnosticIterations: 5 },
  overrides: RunOverrides = {},
): TimingObservation {
  const started = performance.now();
  const request = inspectRequest(spec, layoutId, config, overrides);
  const build = inspectLayout(spec, layoutId, config, overrides);
  const project = new VirtualProject({
    rootDir: config.backendDir,
    compilerOptions: toCompilerOptions(request.project.compilerOptions),
    files: [
      ...request.files.map((file) => ({
        path: path.join(path.dirname(request.project.projectRoot), spec.dir, file.path),
        text: file.text,
      })),
      ...build.files,
    ],
    readPolicy: defaultReadPolicy(config.backendDir),
  });
  const mounts = build.fragments.flatMap((fragment) => fragment.mounts);
  const firstMount = mounts[0];
  if (firstMount) hoverAt(project, firstMount.file, firstMount.targetStartUtf16);
  const coldStartMs = performance.now() - started;

  const hoverTimes: number[] = [];
  for (let iteration = 0; iteration < options.hoverIterations; iteration += 1) {
    for (const mount of mounts) {
      const hoverStart = performance.now();
      hoverAt(project, mount.file, mount.targetStartUtf16);
      hoverTimes.push(performance.now() - hoverStart);
    }
  }

  const diagnosticTimes: number[] = [];
  if (firstMount) {
    for (let iteration = 0; iteration < options.diagnosticIterations; iteration += 1) {
      const diagnosticStart = performance.now();
      semanticDiagnostics(project, firstMount.file);
      diagnosticTimes.push(performance.now() - diagnosticStart);
    }
  }

  return {
    coldStartMs: round(coldStartMs),
    hoverP50Ms: round(percentile(hoverTimes, 0.5)),
    hoverP95Ms: round(percentile(hoverTimes, 0.95)),
    diagnosticsMs: round(median(diagnosticTimes)),
    samples: hoverTimes.length,
  };
}

function round(value: number): number {
  return Math.round(value * 100) / 100;
}

function median(values: number[]): number {
  return percentile(values, 0.5);
}

function percentile(values: number[], fraction: number): number {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((left, right) => left - right);
  const index = Math.min(sorted.length - 1, Math.max(0, Math.ceil(fraction * sorted.length) - 1));
  return sorted[index]!;
}
