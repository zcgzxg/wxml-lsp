import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import ts from "typescript";
import { baseCompilerOptions, defaultReadPolicy, VirtualProject } from "../src/compiler.ts";
import { BACKEND_DIR, SAMPLES_DIR } from "../src/env.ts";
import { classifyDefinitionTarget, inspectLayout, runScenario, type RunConfig } from "../src/run.ts";
import { definitionsAt } from "../src/queries.ts";
import { scenarioById } from "../src/scenarios.ts";

const config: RunConfig = { backendDir: BACKEND_DIR, samplesDir: SAMPLES_DIR };

test("a definition target in a .d.ts file is an external snapshot file", () => {
  const dir = path.join(SAMPLES_DIR, "page-basic");
  const script = path.join(dir, "dts-page.ts");
  const types = path.join(dir, "dts-models.d.ts");
  const scriptText = `import type { User } from './dts-models';\nconst user = { name: 'Ada' } as User;\nuser.name;\n`;
  const project = new VirtualProject({
    rootDir: BACKEND_DIR,
    compilerOptions: baseCompilerOptions(),
    files: [
      { path: script, text: scriptText },
      { path: types, text: "export interface User {\n  /** 用户显示名称 */\n  name: string;\n}\n" },
    ],
    readPolicy: defaultReadPolicy(BACKEND_DIR),
  });
  const offset = scriptText.indexOf("user.name") + "user.".length;
  const definitions = definitionsAt(project, script, offset);
  assert.equal(definitions.length, 1);
  assert.equal(path.basename(definitions[0]!.file), "dts-models.d.ts");
  // The file is inside the snapshot, so the bridge classifies it as `external`
  // and can report its byte range without touching disk.
  assert.ok(project.text(definitions[0]!.file) !== undefined);
});

test("overlay-generated members map back to `properties.<name>`", () => {
  const spec = scenarioById("component-properties");
  for (const layout of ["in-place-shadow", "symbol-bridge"] as const) {
    const build = inspectLayout(spec, layout, config);
    assert.ok(build.generatedDeclarations.length > 0, `${layout} should record generated members`);
    const title = build.generatedDeclarations.find((declaration) => declaration.length === "title".length);
    assert.ok(title?.source, "generated member must carry a source mapping");
    const scriptText = build.files[0]!.text;
    void scriptText;
    const run = runScenario(spec, layout, config);
    const definition = run.definitions.find((entry) => entry.query === "title")?.observation;
    assert.equal(definition?.kind, "generated");
    assert.deepEqual(definition?.range, { start: title!.source!.start, end: title!.source!.end });
  }
});

test("every query definition is classified and maps to original bytes", () => {
  const derivedFile = path.join(SAMPLES_DIR, "component-properties", "__wxml_view_0.ts");
  for (const id of ["page-basic", "component-properties", "cross-file-overlay"]) {
    const spec = scenarioById(id);
    for (const layout of ["in-place-shadow", "symbol-bridge"] as const) {
      const run = runScenario(spec, layout, config);
      for (const entry of run.definitions) {
        const observation = entry.observation;
        if (!observation) continue;
        assert.notEqual(observation.kind, "dropped", `${id}/${layout}: ${entry.query} dropped`);
        assert.ok(observation.range, `${id}/${layout}: ${entry.query} must map to original bytes`);
        // The published target is `range`; a generated member may live in
        // the derived file internally, but the *external* kind must never point
        // at a derived file.
        if (observation.kind === "external") {
          assert.notEqual(observation.file, derivedFile, "external target must not be a derived file");
        }
      }
    }
  }
});

test("a definition inside the derived query file is dropped, never published", () => {
  const spec = scenarioById("component-properties");
  const build = inspectLayout(spec, "symbol-bridge", config);
  const target = {
    file: path.resolve(build.queryFile),
    start: 0,
    length: "0123456789".length,
  };
  // Not inside a generated declaration: a derived path must be dropped.
  const outsideGenerated = classifyDefinitionTarget(
    { ...target, start: build.queryFile.length + 1000 },
    build,
    path.join(SAMPLES_DIR, spec.dir, spec.script),
    "",
  );
  assert.equal(outsideGenerated.kind, "dropped");
  assert.match(outsideGenerated.reason ?? "", /derived file/);
});

test("TypeScript uses the same `.d.ts` target through the language service API", () => {
  // Guard against API drift: `definitionsAt` relies on `DefinitionInfo.fileName`.
  const dir = path.join(SAMPLES_DIR, "page-basic");
  const script = path.join(dir, "dts-guard.ts");
  const project = new VirtualProject({
    rootDir: BACKEND_DIR,
    compilerOptions: baseCompilerOptions(),
    files: [{ path: script, text: "const value: number = 1;\nvalue.toFixed();\n" }],
    readPolicy: defaultReadPolicy(BACKEND_DIR),
  });
  const text = "const value: number = 1;\nvalue.toFixed();\n";
  const definitions = definitionsAt(project, script, text.indexOf("toFixed"));
  assert.ok(
    definitions.some((definition) => definition.file.endsWith(".d.ts") && definition.file.includes("typescript")),
    definitions.map((definition) => definition.file).join(","),
  );
  assert.equal(ts.version.startsWith("6."), true);
});
