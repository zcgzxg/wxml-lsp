import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import {
  baseCompilerOptions,
  defaultReadPolicy,
  EnvironmentError,
  assertEnvironment,
  VirtualProject,
} from "../src/compiler.ts";
import { BACKEND_DIR } from "../src/env.ts";
import { checkProbeHealth, scanPragmas } from "../src/probes.ts";
import { semanticDiagnostics } from "../src/queries.ts";

function project(file: string, text: string): VirtualProject {
  return new VirtualProject({
    rootDir: BACKEND_DIR,
    compilerOptions: baseCompilerOptions(),
    files: [{ path: file, text }],
    readPolicy: defaultReadPolicy(BACKEND_DIR),
  });
}

const probeInsertion = (file: string, start: number, length: number) =>
  ({ insertionId: "probe", file, startUtf16: start, lengthUtf16: length, replaceLengthUtf16: 0, viewRevision: 1, backendFileVersion: 1, kind: "probe" as const });

test("pragma scanning handles plain files, BOM and hashbang", () => {
  const plain = scanPragmas("// @ts-check\nconst a = 1;\n", true);
  assert.equal(plain.check, true);
  assert.equal(plain.injectionOffset, 0);
  assert.equal(plain.neutralizeOffset, "// @ts-check\n".length);
  assert.equal(plain.dropBom, false);

  const bom = scanPragmas("\uFEFFconst a = 1;\n", true);
  assert.equal(bom.bomLength, 1);
  assert.equal(bom.dropBom, true, "the BOM is dropped so a pragma is recognised");
  assert.equal(bom.injectionOffset, 1);

  const shebang = scanPragmas("#!/usr/bin/env node\nconst a = 1;\n", true);
  assert.equal(shebang.injectionOffset, 20, "the pragma goes after the hashbang line");
  assert.equal(shebang.dropBom, false);

  const both = scanPragmas("\uFEFF#!/usr/bin/env node\nconst a = 1;\n", true);
  assert.equal(both.dropBom, true);
  assert.equal(both.injectionOffset, 21);
});

test("the last pragma in the leading block wins (TypeScript semantics)", () => {
  const checkThenNoCheck = scanPragmas("// @ts-check\n// @ts-nocheck\nconst a = 1;\n", false);
  assert.equal(checkThenNoCheck.check, false);
  assert.equal(checkThenNoCheck.noCheck, true);
  assert.equal(checkThenNoCheck.neutralizeOffset, "// @ts-check\n// @ts-nocheck\n".length);

  const noCheckThenCheck = scanPragmas("// @ts-nocheck\n// @ts-check\nconst a = 1;\n", false);
  assert.equal(noCheckThenCheck.check, true);
  assert.equal(noCheckThenCheck.noCheck, false);
});

test("`@ts-nocheck` outside the leading comments is ignored", () => {
  assert.equal(scanPragmas("// ordinary comment\nPage({ data: { x: 1 } });\n", false).noCheck, false);
  assert.equal(scanPragmas("const a = 1;\n// @ts-nocheck\n", false).noCheck, false);
});

test("probe health rejects a probe root that resolves to any", () => {
  const file = path.join(BACKEND_DIR, "samples", "page-basic", "health.ts");
  const text = `Page({ data: { n: 1 }, __wxml_check_0() { const __wxml_root = null as any; return [__wxml_root.n]; } });\n`;
  const instance = project(file, text);
  const rootOffset = text.indexOf("null as any") + "null as any".length;
  const health = checkProbeHealth({
    project: instance,
    scriptPath: file,
    helperOffsets: [{ name: "__wxml_check_0", offset: text.indexOf("__wxml_check_0") }],
    rootOffset,
    insertions: [probeInsertion(file, text.indexOf("__wxml_check_0"), 10)],
    mounts: [],
  });
  assert.ok(health.problems.some((problem) => problem.includes("probe root")), health.problems.join("; "));
});

test("probe health rejects diagnostics that intersect an insertion", () => {
  const file = path.join(BACKEND_DIR, "samples", "page-basic", "health2.ts");
  const text = `Page({ data: { n: 1 }, __wxml_check_0() { return [this.data.missing]; } });\n`;
  const instance = project(file, text);
  const start = text.indexOf("__wxml_check_0");
  const health = checkProbeHealth({
    project: instance,
    scriptPath: file,
    helperOffsets: [],
    insertions: [probeInsertion(file, start, text.length - start - 4)],
    mounts: [],
  });
  assert.ok(
    health.problems.some((problem) => problem.includes("intersects insertion")),
    health.problems.join("; "),
  );
});

test("probe health uses the exact offsets it is given, not a name search", () => {
  const file = path.join(BACKEND_DIR, "samples", "page-basic", "health3.ts");
  // A user symbol with the same name appears *before* the probe.
  const text = `const __wxml_root = 1;\nPage({ data: { n: 1 }, __wxml_check_0() { const __wxml_root = this.data; return [__wxml_root.n]; } });\n`;
  const instance = project(file, text);
  const probeOffset = text.lastIndexOf("__wxml_root");
  const userOffset = text.indexOf("__wxml_root");
  assert.notEqual(probeOffset, userOffset);

  const exact = checkProbeHealth({
    project: instance,
    scriptPath: file,
    helperOffsets: [{ name: "__wxml_root", offset: probeOffset }],
    insertions: [],
    mounts: [],
  });
  assert.deepEqual(exact.problems, [], "the probe's own binding resolves");

  const wrong = checkProbeHealth({
    project: instance,
    scriptPath: file,
    helperOffsets: [{ name: "__wxml_root", offset: userOffset }],
    insertions: [],
    mounts: [],
  });
  assert.deepEqual(wrong.problems, [], "the user symbol also resolves; offsets are trusted as given");

  const keyword = checkProbeHealth({
    project: instance,
    scriptPath: file,
    helperOffsets: [{ name: "__wxml_root", offset: text.indexOf("const") }],
    insertions: [],
    mounts: [],
  });
  assert.ok(keyword.problems.some((problem) => problem.includes("has no symbol")), keyword.problems.join("; "));
});

test("a duplicate helper is reported through the insertion check", () => {
  const file = path.join(BACKEND_DIR, "samples", "page-basic", "health4.ts");
  // Same block: the probe redeclares a helper that is already in scope.
  const text = `Page({\n  data: { n: 1 },\n  __wxml_check_0() {\n    const __wxml_root = 1;\n    const __wxml_root = this.data;\n    return [__wxml_root.n];\n  },\n});\n`;
  const instance = project(file, text);
  const health = checkProbeHealth({
    project: instance,
    scriptPath: file,
    helperOffsets: [],
    insertions: [probeInsertion(file, text.indexOf("  __wxml_check_0"), text.length - text.indexOf("  __wxml_check_0") - 4)],
    mounts: [],
  });
  assert.ok(
    health.problems.some((problem) => problem.includes("intersects insertion")),
    `expected a duplicate-identifier artefact, got: ${health.problems.join("; ")}`,
  );
});

test("probe health reports a module classification flip", () => {
  const file = path.join(BACKEND_DIR, "samples", "page-basic", "health5.ts");
  const text = `Page({ data: { n: 1 } });\n`;
  const instance = project(file, text);
  const health = checkProbeHealth({
    project: instance,
    scriptPath: file,
    helperOffsets: [],
    insertions: [],
    mounts: [],
    moduleBefore: "script",
    moduleAfter: "module",
  });
  assert.ok(health.problems.some((problem) => problem.includes("module classification flipped")));
});

test("environment check fails fast when lib or typings are missing", () => {
  assert.throws(
    () => assertEnvironment("/nonexistent-backend", baseCompilerOptions()),
    (error: unknown) => error instanceof EnvironmentError,
  );
});

test("read policy keeps project files out of disk reads", () => {
  const file = path.join(BACKEND_DIR, "samples", "cross-file-overlay", "index.ts");
  const text = "import type { User } from './models';\nPage({ data: { user: { name: 'Ada' } as User } });\n";
  const instance = project(file, text);
  assert.equal(instance.has(path.join(BACKEND_DIR, "samples", "cross-file-overlay", "models.ts")), false);
  const codes = semanticDiagnostics(instance, file).map((diagnostic) => diagnostic.code);
  assert.ok(codes.includes(2307), `expected unresolved import, got ${codes.join(",")}`);
  assert.ok(
    instance.diskReadable().every((entry) => entry.includes("typescript") || entry.includes("miniprogram-api-typings")),
  );
});
