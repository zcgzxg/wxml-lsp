import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { BACKEND_DIR, SAMPLES_DIR } from "../src/env.ts";
import { runScenario, type RunConfig } from "../src/run.ts";
import { SCENARIOS } from "../src/scenarios.ts";
import type { CapabilityStates, CapabilityStatus, LayoutId, ScenarioRun } from "../src/types.ts";

const config: RunConfig = { backendDir: BACKEND_DIR, samplesDir: SAMPLES_DIR };
const LAYOUTS: LayoutId[] = ["in-place-shadow", "symbol-bridge"];

type States = Partial<Record<keyof CapabilityStates, CapabilityStatus>>;
const ALL = (status: CapabilityStatus): States => ({ hover: status, diagnostics: status, definition: status });

/** Capability states that the evidence record documents. */
const EXPECTED_STATES: Record<string, Partial<Record<LayoutId, States>>> = {
  "page-closure": { "symbol-bridge": ALL("unsupported") },
  "component-module": { "symbol-bridge": ALL("partial") },
  "component-data-cross-file": { "symbol-bridge": ALL("partial") },
  "properties-spread": { "in-place-shadow": ALL("partial"), "symbol-bridge": ALL("partial") },
  "multi-registration": { "in-place-shadow": ALL("partial"), "symbol-bridge": ALL("partial") },
  "methods-nonliteral": { "in-place-shadow": ALL("partial") },
  "data-shorthand": { "in-place-shadow": ALL("unsupported"), "symbol-bridge": ALL("unsupported") },
  "js-jsdoc-pragma": { "in-place-shadow": { diagnostics: "partial" } },
  "js-source-check-off": {
    "in-place-shadow": { diagnostics: "disabled" },
    "symbol-bridge": { diagnostics: "disabled" },
  },
  "js-bom-check-off": {
    "in-place-shadow": { diagnostics: "disabled" },
    "symbol-bridge": { diagnostics: "disabled" },
  },
  "no-implicit-this-off": {
    "in-place-shadow": ALL("unavailable"),
    "symbol-bridge": ALL("unavailable"),
  },
  "js-jsdoc-off": { "in-place-shadow": { diagnostics: "disabled" }, "symbol-bridge": { diagnostics: "disabled" } },
  "ts-nocheck": { "in-place-shadow": { diagnostics: "disabled" }, "symbol-bridge": { diagnostics: "disabled" } },
  "js-nocheck": { "in-place-shadow": { diagnostics: "disabled" }, "symbol-bridge": { diagnostics: "disabled" } },
  "interface-root": { "in-place-shadow": ALL("unsupported"), "symbol-bridge": ALL("unsupported") },
  "interface-root-import": { "in-place-shadow": ALL("unsupported"), "symbol-bridge": ALL("unsupported") },
  "component-properties": {
    "in-place-shadow": { diagnostics: "supported" },
    "symbol-bridge": { diagnostics: "partial" },
  },
};

/** User-script diagnostics the derived view is known to add. */
const EXPECTED_INTRODUCED: Record<string, Partial<Record<LayoutId, number[]>>> = {
  "js-jsdoc-pragma": { "in-place-shadow": [2322] },
  "component-properties": { "symbol-bridge": [2339] },
  "component-module": { "symbol-bridge": [2339, 2304] },
  "component-data-cross-file": { "symbol-bridge": [2304] },
};

/** Candidate B rewrites the registration and loses contextual `this`. */
const EXPECTED_PERTURBATION: Record<string, LayoutId[]> = {
  "page-basic": ["symbol-bridge"],
  "component-properties": ["symbol-bridge"],
  "component-module": ["symbol-bridge"],
  "component-js": ["symbol-bridge"],
};

/** Candidate B cannot see the overlay type from a module script's anchor. */
const EXPECTED_INEXACT: Record<string, LayoutId[]> = {
  "component-module": ["symbol-bridge"],
};

/** Root cause that must survive the diagnostic-mismatch reason. */
const EXPECTED_REASON: Record<string, Partial<Record<LayoutId, string>>> = {
  "component-module": { "symbol-bridge": "module scope" },
  "component-data-cross-file": { "symbol-bridge": "module scope" },
  "properties-spread": { "in-place-shadow": "spread assignment", "symbol-bridge": "spread assignment" },
  "methods-nonliteral": { "in-place-shadow": "object literal" },
  "multi-registration": { "in-place-shadow": "2 page registrations", "symbol-bridge": "2 page registrations" },
  "data-shorthand": { "in-place-shadow": "property assignment", "symbol-bridge": "property assignment" },
  "no-implicit-this-off": {
    "in-place-shadow": "probe root",
    "symbol-bridge": "probe root",
  },
};

function scenarioText(spec: (typeof SCENARIOS)[number]): string {
  return fs.readFileSync(path.join(SAMPLES_DIR, spec.dir, spec.script), "utf8");
}

function bytesOf(text: string, range: { start: number; end: number }): string {
  return Buffer.from(text, "utf8").subarray(range.start, range.end).toString("utf8");
}

test("every scenario runs both candidates without throwing", () => {
  for (const spec of SCENARIOS) {
    for (const layout of LAYOUTS) {
      assert.doesNotThrow(() => runScenario(spec, layout, config), `${spec.id} / ${layout}`);
    }
  }
});

for (const spec of SCENARIOS) {
  for (const layout of LAYOUTS) {
    test(`${spec.id} / ${layout}`, () => {
      const run: ScenarioRun = runScenario(spec, layout, config);
      const expectedStates: States = {
        hover: "supported",
        diagnostics: "supported",
        definition: "supported",
        ...EXPECTED_STATES[spec.id]?.[layout],
      };

      for (const capability of ["hover", "diagnostics", "definition"] as const) {
        const state = run.states[capability];
        assert.equal(state.status, expectedStates[capability], `${capability} status of ${spec.id}/${layout}`);
        if (state.status !== "supported") {
          assert.ok(state.reason, `${capability} degradation must carry a reason (${spec.id}/${layout})`);
        }
        const rootCause = EXPECTED_REASON[spec.id]?.[layout];
        if (rootCause) {
          assert.match(run.states[capability].reason ?? "", new RegExp(rootCause), `root cause of ${spec.id}/${layout}`);
        }
      }

      if (run.states.hover.status === "unsupported") {
        assert.equal(run.hovers.length, 0, "unsupported hover must not return data");
        assert.equal(run.diagnostics.length, 0, "unsupported layout must not publish diagnostics");
        return;
      }

      for (const query of spec.queries) {
        if (run.states.hover.status !== "supported") continue;
        const hover = run.hovers.find((entry) => entry.query === query.expr);
        assert.ok(hover, `missing hover for ${query.expr}`);
        const observation = hover.observation;
        assert.ok(observation, `no quick info for ${query.expr}`);
        if (query.expectType) assert.equal(observation.typeOnly, query.expectType, `type of ${query.expr}`);
        if (query.expectDoc) assert.equal(observation.documentation, query.expectDoc, `documentation of ${query.expr}`);
        if (query.expectDocSource) {
          assert.equal(observation.documentationSource, query.expectDocSource, `doc source of ${query.expr}`);
        }
      }

      const inexact = (EXPECTED_INEXACT[spec.id] ?? []).includes(layout);
      if (inexact) {
        assert.equal(run.diagnosticsExact, false, "documented degradation: the overlay type is not visible");
        assert.deepEqual(run.diagnostics, []);
        assert.deepEqual(
          run.missingDiagnostics.map((diagnostic) => diagnostic.expr),
          spec.expectedDiagnostics.map((diagnostic) => diagnostic.expr),
        );
      } else {
        assert.equal(
          run.diagnosticsExact,
          true,
          `diagnostics of ${spec.id}/${layout}: missing=${JSON.stringify(run.missingDiagnostics)} ` +
            `unexpected=${JSON.stringify(run.unexpectedDiagnostics.map((diagnostic) => diagnostic.range))}`,
        );
      }

      const scriptText = scenarioText(spec);
      for (const query of spec.queries) {
        if (!query.definition || run.states.definition.status !== "supported") continue;
        const definition = run.definitions.find((entry) => entry.query === query.expr)?.observation;
        assert.ok(definition, `no definition for ${query.expr}`);
        assert.equal(definition.kind, query.expectDefinition, `definition kind of ${query.expr}`);
        assert.ok(definition.range, `definition of ${query.expr} must map to source bytes`);
        const targetText =
          definition.kind === "external"
            ? fs.readFileSync(definition.file!, "utf8")
            : scriptText;
        assert.ok(bytesOf(targetText, definition.range).length > 0, "definition range must point at source text");
      }

      // Non-interference is reported per metric, never as a single flag.
      const perturbation = (EXPECTED_PERTURBATION[spec.id] ?? []).includes(layout);
      assert.equal(run.perturbsUserTypes, perturbation, `user type perturbation of ${spec.id}/${layout}`);

      const introduced = (EXPECTED_INTRODUCED[spec.id]?.[layout] ?? []).slice().sort();
      assert.deepEqual(
        run.userScriptDiagnosticsIntroduced.map((entry) => entry.code).sort(),
        introduced,
        `introduced script diagnostics of ${spec.id}/${layout}`,
      );

      // Request/response envelope is present on every successful run.
      const expressions = new Set([
        ...spec.queries.map((query) => query.expr),
        ...spec.expectedDiagnostics.map((diagnostic) => diagnostic.expr),
      ]);
      assert.equal(run.request.ownerKind, spec.registration);
      assert.ok(run.request.fileCount >= 1);
      assert.equal(run.request.fragmentCount, expressions.size, "request fragments mirror the WXML inputs");
      assert.ok(run.registration.count >= 1);
    });
  }
}

test("candidate B cannot anchor a registration inside a function", () => {
  const spec = SCENARIOS.find((candidate) => candidate.id === "page-closure");
  assert.ok(spec);
  const run = runScenario(spec, "symbol-bridge", config);
  assert.equal(run.states.hover.status, "unsupported");
  assert.match(run.states.hover.reason ?? "", /inside a function/);
});

test("module components expose candidate B's overlay-scope failure", () => {
  const spec = SCENARIOS.find((candidate) => candidate.id === "component-module");
  assert.ok(spec);
  const run = runScenario(spec, "symbol-bridge", config);
  assert.equal(run.states.hover.status, "partial");
  assert.match(run.states.hover.reason ?? "", /module scope/);
  assert.ok(run.hovers.every((hover) => hover.observation === null || hover.observation.quality === "any"));
  assert.deepEqual(run.userScriptDiagnosticsIntroduced.map((entry) => entry.code).sort(), [2304, 2339]);
});

test("interface used as `data` value is reported, never fabricated (same file and import)", () => {
  for (const id of ["interface-root", "interface-root-import"]) {
    const spec = SCENARIOS.find((candidate) => candidate.id === id);
    assert.ok(spec);
    for (const layout of LAYOUTS) {
      const run = runScenario(spec, layout, config);
      assert.equal(run.states.hover.status, "unsupported", `${id}/${layout}`);
      assert.match(run.states.hover.reason ?? "", /type-only/);
      assert.equal(run.diagnostics.length, 0);
    }
  }
});

test("a shorthand `data` member is unsupported, not 'missing'", () => {
  const spec = SCENARIOS.find((candidate) => candidate.id === "data-shorthand");
  assert.ok(spec);
  for (const layout of LAYOUTS) {
    const run = runScenario(spec, layout, config);
    assert.equal(run.states.hover.status, "unsupported");
    assert.match(run.states.hover.reason ?? "", /not a property assignment/);
    assert.ok(!/no `data` member/.test(run.states.hover.reason ?? ""));
  }
});

test("JavaScript hover works without checking, diagnostics do not", () => {
  const spec = SCENARIOS.find((candidate) => candidate.id === "js-jsdoc-off");
  assert.ok(spec);
  for (const layout of LAYOUTS) {
    const run = runScenario(spec, layout, config);
    assert.equal(run.states.hover.status, "supported");
    assert.equal(run.states.diagnostics.status, "disabled");
    assert.match(run.states.diagnostics.reason ?? "", /checkJs|policy/);
    assert.equal(run.diagnostics.length, 0);
    assert.equal(run.hovers.find((entry) => entry.query === "greeting")?.observation?.typeOnly, "string");
  }
});

test("explicit off beats a source `@ts-check` and publishes no diagnostics", () => {
  const off = SCENARIOS.find((candidate) => candidate.id === "js-source-check-off");
  const inherit = SCENARIOS.find((candidate) => candidate.id === "js-source-check-inherit");
  assert.ok(off && inherit);
  for (const layout of LAYOUTS) {
    const disabled = runScenario(off, layout, config);
    assert.equal(disabled.jsCheckApplied, "off");
    assert.equal(disabled.states.diagnostics.status, "disabled");
    assert.equal(disabled.diagnostics.length, 0, "off must not publish template diagnostics");
    assert.equal(disabled.states.hover.status, "supported", "hover still works under off");

    const checked = runScenario(inherit, layout, config);
    assert.equal(checked.jsCheckApplied, "inherit");
    assert.equal(checked.states.diagnostics.status, "supported");
    assert.equal(checked.diagnostics.length, 1);
    assert.equal(checked.diagnostics[0]?.code, 2339);
  }
});

test("component properties union needs the declaration overlay in both candidates", () => {
  const spec = SCENARIOS.find((candidate) => candidate.id === "component-properties");
  assert.ok(spec);
  for (const layout of LAYOUTS) {
    const run = runScenario(spec, layout, config);
    const title = run.hovers.find((entry) => entry.query === "title")?.observation;
    assert.equal(title?.typeOnly, "string | number", `${layout} must report the declared union`);
    assert.equal(title?.documentation, "标题，支持字符串或数字");
    assert.equal(title?.documentationSource, "copied", `${layout} documentation is copied text`);
  }
});

test("BOM + hashbang + source pragma works for off and inherit", () => {
  const off = SCENARIOS.find((candidate) => candidate.id === "js-bom-check-off");
  const inherit = SCENARIOS.find((candidate) => candidate.id === "js-bom-check-inherit");
  assert.ok(off && inherit);
  for (const layout of LAYOUTS) {
    const disabled = runScenario(off, layout, config);
    assert.equal(disabled.jsCheckApplied, "off");
    assert.equal(disabled.states.diagnostics.status, "disabled");
    assert.equal(disabled.diagnostics.length, 0, "off must not publish anything");

    const checked = runScenario(inherit, layout, config);
    assert.equal(checked.jsCheckApplied, "inherit");
    assert.equal(checked.diagnostics.length, 1, "the source @ts-check must stay effective");
    assert.equal(checked.diagnostics[0]?.code, 2339);
    assert.deepEqual(checked.diagnostics[0]?.range, { start: 91, end: 98 });
  }
});

test("multi-registration uses the first registration and does not merge", () => {
  const spec = SCENARIOS.find((candidate) => candidate.id === "multi-registration");
  assert.ok(spec);
  const scriptText = scenarioText(spec);
  assert.match(scriptText, /title: 42/, "the second registration intentionally declares a number");
  for (const layout of LAYOUTS) {
    const run = runScenario(spec, layout, config);
    assert.equal(run.states.hover.status, "partial");
    assert.match(run.states.hover.reason ?? "", /2 page registrations/);
    assert.equal(run.registration.count, 2);
    assert.equal(
      run.hovers.find((entry) => entry.query === "title")?.observation?.typeOnly,
      "string",
      "the first registration wins; the number from the second is not merged",
    );
  }
});

test("documented option switches change the derived check", () => {
  const unchecked = SCENARIOS.find((candidate) => candidate.id === "unchecked-index");
  const nullableOff = SCENARIOS.find((candidate) => candidate.id === "nullable-no-strict-null");
  assert.ok(unchecked && nullableOff);
  for (const layout of LAYOUTS) {
    const indexed = runScenario(unchecked, layout, config);
    assert.equal(indexed.diagnostics.length, 1);
    assert.equal(indexed.diagnostics[0]?.code, 2532, "noUncheckedIndexedAccess must not be dropped");
    assert.equal(indexed.diagnosticsExact, true);

    const nullable = runScenario(nullableOff, layout, config);
    assert.equal(nullable.diagnostics.length, 0, "strictNullChecks:false must not produce 18047");
    assert.equal(nullable.hovers[0]?.observation?.typeOnly, "string");
  }
});

test("noImplicitThis:false degrades the probe instead of claiming Supported", () => {
  const spec = SCENARIOS.find((candidate) => candidate.id === "no-implicit-this-off");
  assert.ok(spec);
  for (const layout of LAYOUTS) {
    const run = runScenario(spec, layout, config);
    assert.equal(run.states.hover.status, "unavailable");
    assert.match(run.states.hover.reason ?? "", /probe root resolves to any/);
    assert.equal(run.diagnostics.length, 0);
  }
});
