/** Matrix formatting: Markdown tables (pipes escaped) plus the JSON form. */

import type { CapabilityState, ScenarioRun } from "./types.ts";

function esc(value: string): string {
  return value.replace(/\|/g, "\\|").replace(/\n/g, " ");
}

function capability(state: CapabilityState): string {
  return state.reason ? `${state.status}: ${esc(state.reason)}` : state.status;
}

export function toMarkdown(runs: ScenarioRun[]): string {
  const header = [
    "| scenario | candidate | hover | diagnostics | definition | module | perturbs user types | script diagnostics introduced | suppressed | notes |",
    "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |",
  ];
  const rows = runs.map((run) => {
    const introduced = run.userScriptDiagnosticsIntroduced.map((entry) => `${entry.code}`).join(",") || "—";
    const notes = run.notes?.map(esc).join("; ") ?? "—";
    return `| ${run.scenario} | ${run.layout} | ${capability(run.states.hover)} | ${capability(run.states.diagnostics)} | ${capability(run.states.definition)} | ${run.moduleBefore}→${run.moduleAfter} | ${run.perturbsUserTypes ? "yes" : "no"} | ${introduced} | ${run.suppressedDiagnostics} | ${notes} |`;
  });
  return [...header, ...rows].join("\n");
}

export function hoverTable(runs: ScenarioRun[]): string {
  const header = ["| scenario | candidate | query | type | quality | documentation | doc source |", "| --- | --- | --- | --- | --- | --- | --- |"];
  const rows: string[] = [];
  for (const run of runs) {
    for (const hover of run.hovers) {
      const observation = hover.observation;
      rows.push(
        `| ${run.scenario} | ${run.layout} | ${esc(hover.query)} | ${esc(observation?.typeOnly ?? "—")} | ${observation?.quality ?? "—"} | ${esc(observation?.documentation || "—")} | ${observation?.documentationSource ?? "—"} |`,
      );
    }
  }
  return [...header, ...rows].join("\n");
}

export function diagnosticsTable(runs: ScenarioRun[]): string {
  const header = [
    "| scenario | candidate | expected | observed (WXML bytes) | trimmed prefix | messages |",
    "| --- | --- | --- | --- | --- | --- |",
  ];
  const rows = runs.map((run) => {
    const expected = run.expectedDiagnostics.map((diagnostic) => esc(diagnostic.expr)).join(", ") || "—";
    const observed =
      run.diagnostics
        .map((diagnostic) =>
          diagnostic.range ? `${diagnostic.range.start}..${diagnostic.range.end} (${diagnostic.code})` : `${diagnostic.code}`,
        )
        .join(", ") || "—";
    const trimmed = run.diagnostics.some((diagnostic) => diagnostic.trimmedSyntheticPrefix) ? "yes" : "no";
    const messages = run.diagnostics.map((diagnostic) => esc(diagnostic.message)).join(" / ") || "—";
    return `| ${run.scenario} | ${run.layout} | ${expected} | ${observed} | ${trimmed} | ${messages} |`;
  });
  return [...header, ...rows].join("\n");
}

export function definitionsTable(runs: ScenarioRun[]): string {
  const header = ["| scenario | candidate | query | kind | script range |", "| --- | --- | --- | --- | --- |"];
  const rows: string[] = [];
  for (const run of runs) {
    for (const definition of run.definitions) {
      const observation = definition.observation;
      const range = observation?.range ? `${observation?.range.start}..${observation?.range.end}` : "—";
      rows.push(`| ${run.scenario} | ${run.layout} | ${esc(definition.query)} | ${observation?.kind ?? "none"} | ${range} |`);
    }
  }
  return [...header, ...rows].join("\n");
}
