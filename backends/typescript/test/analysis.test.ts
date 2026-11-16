import assert from "node:assert/strict";
import { test } from "node:test";
import { analyze, finishedStates } from "../src/analysis.ts";
import { BACKEND_DIR, SAMPLES_DIR } from "../src/env.ts";
import { RequestError, validateRequest } from "../src/protocol.ts";
import { inspectRequest, type RunConfig } from "../src/run.ts";
import { scenarioById } from "../src/scenarios.ts";
import type { AnalyzeRequest } from "../src/protocol.ts";

const config: RunConfig = { backendDir: BACKEND_DIR, samplesDir: SAMPLES_DIR };
const analysisConfig = { backendDir: BACKEND_DIR };

function pageBasicRequest(): AnalyzeRequest {
  return structuredClone(inspectRequest(scenarioById("page-basic"), "in-place-shadow", config));
}

function fragment(overrides: Partial<AnalyzeRequest["fragments"][number]> = {}) {
  const exprText = overrides.exprText ?? "title";
  return {
    fragmentId: "f0",
    exprText,
    wxmlRange: { start: 0, end: Buffer.byteLength(exprText, "utf8") },
    fragmentRevision: 1,
    root: "page" as const,
    ...overrides,
  };
}

test("analyze returns hover, diagnostic and definition for a page", () => {
  const request = pageBasicRequest();
  const result = analyze(request, analysisConfig);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.response.protocolVersion, 2);
  assert.equal(result.response.requestId, request.requestId);

  const title = result.response.fragments.find((entry) => entry.fragmentId === "title");
  assert.equal(title?.hover?.typeOnly, "string");
  assert.equal(title?.hover?.documentation, "页面标题");

  const typo = result.response.fragments.find((entry) => entry.fragmentId === "user.nmae");
  assert.deepEqual(typo?.diagnostics[0]?.range, { start: 134, end: 138 });
  assert.ok(!("spanUtf16" in (typo?.diagnostics[0] as object)));

  const name = result.response.fragments.find((entry) => entry.fragmentId === "user.name");
  assert.equal(name?.definition?.kind, "script");
  assert.deepEqual(name?.definition?.range, { start: 136, end: 140 });
});

test("analyze uses the unsaved file text from the request (no disk read)", () => {
  const request = pageBasicRequest();
  const script = request.files.find((file) => file.fileId === request.scriptFileId);
  assert.ok(script);
  script.text = script.text.replace("/** 用户显示名称 */\n      name: 'Ada',", "/** 未保存的名称 */\n      name: 42,");
  const result = analyze(request, analysisConfig);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const name = result.response.fragments.find((entry) => entry.fragmentId === "user.name");
  assert.equal(name?.hover?.typeOnly, "number");
  assert.equal(name?.hover?.documentation, "未保存的名称");
});

test("analyze never reads project files or tsconfig from the project root", () => {
  const request = pageBasicRequest();
  // The project root does not exist on disk; only the in-memory files matter.
  request.project.projectRoot = "/nonexistent/project-root";
  const result = analyze(request, analysisConfig);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.response.fragments.find((entry) => entry.fragmentId === "title")?.hover?.typeOnly, "string");
});

test("target selects the reported token", () => {
  const request = pageBasicRequest();
  const fragment = request.fragments.find((entry) => entry.fragmentId === "user.nmae");
  assert.ok(fragment);
  fragment.target = "nmae";
  const result = analyze(request, analysisConfig);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const typo = result.response.fragments.find((entry) => entry.fragmentId === "user.nmae");
  assert.deepEqual(typo?.diagnostics[0]?.range, { start: 134, end: 138 });
});

test("target must be a whole identifier in the expression", () => {
  const request = pageBasicRequest();
  request.fragments[0]!.target = "nmae";
  assert.throws(
    () => analyze(request, analysisConfig),
    (error: unknown) => error instanceof RequestError && error.code === "bad-fragment",
  );
});

test("locals mount the expression without a root prefix", () => {
  const request = pageBasicRequest();
  request.fragments = [fragment({ exprText: "item.name", target: "item", locals: ["item"] })];
  const result = analyze(request, analysisConfig);
  assert.equal(result.ok, true);
  if (!result.ok) return;

  assert.equal(result.mounts.length, 1);
  assert.equal(result.mounts[0]?.text, "item.name");
  assert.ok(!result.mounts[0]?.text.startsWith("__wxml_root."));
  assert.equal(result.mounts[0]?.segments.length, 1, "local fragments have no synthetic prefix");
  const hover = result.response.fragments[0]?.hover;
  assert.equal(hover?.quality, "any", "locals are bound as any until P6 provides types");
  assert.equal(result.response.fragments[0]?.diagnostics.length, 0);
  assert.deepEqual(result.probeProblems, [], "locals must not break the probe offsets");
  assert.equal(result.states.hover.status, "partial");
  assert.equal(result.states.diagnostics.status, "partial");
  assert.equal(result.states.definition.status, "partial");
  assert.match(result.states.hover.reason ?? "", /could not be resolved and were not checked/);
});

test("component locals keep overlay definitions mapped to the original script", () => {
  const request = structuredClone(inspectRequest(scenarioById("component-properties"), "in-place-shadow", config));
  request.fragments = [
    ...request.fragments,
    fragment({ fragmentId: "f-local", exprText: "item.name", target: "name", locals: ["item"] }),
  ];
  const result = analyze(request, analysisConfig);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.deepEqual(result.probeProblems, []);
  assert.equal(result.states.hover.status, "partial");
  const title = result.response.fragments.find((entry) => entry.fragmentId === "title");
  assert.equal(title?.definition?.kind, "generated");
  assert.equal(title?.definition?.fileId, "index.ts");
  assert.deepEqual(title?.definition?.range, { start: 77, end: 82 });
});

test("off + locals keeps diagnostics Disabled and the rest Partial", () => {
  const request = structuredClone(inspectRequest(scenarioById("js-source-check-off"), "in-place-shadow", config));
  request.fragments = [fragment({ exprText: "item.name", target: "name", locals: ["item"] })];
  const result = analyze(request, analysisConfig);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.states.diagnostics.status, "disabled");
  assert.equal(result.states.hover.status, "partial");
  assert.equal(result.states.definition.status, "partial");
  assert.equal(result.diagnostics.length, 0);
});

test("the default target is the rightmost identifier", () => {
  const request = pageBasicRequest();
  request.fragments = [
    fragment({ fragmentId: "f0", exprText: "user.name" }),
    fragment({ fragmentId: "f1", exprText: "users[0].name" }),
    fragment({ fragmentId: "f2", exprText: "user.nmae" }),
  ];
  const result = analyze(request, analysisConfig);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const target = (id: string) => result.response.mounts.find((mount) => mount.fragmentId === id)?.target;
  assert.equal(target("f0"), "name");
  assert.equal(target("f1"), "name");
  assert.equal(target("f2"), "nmae");
  assert.equal(result.response.fragments.find((entry) => entry.fragmentId === "f0")?.hover?.typeOnly, "string");
});

test("insertions carry viewRevision and backendFileVersion", () => {
  const request = pageBasicRequest();
  request.analysisStamp.viewRevision = 7;
  request.files.find((file) => file.fileId === request.scriptFileId)!.sourceRevision = 3;
  const result = analyze(request, analysisConfig);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.ok(result.response.insertions.length > 0);
  for (const insertion of result.response.insertions) {
    assert.equal(insertion.viewRevision, 7);
    assert.ok(insertion.backendFileVersion >= 1);
    assert.equal(insertion.backendFileVersion, 3, "script file sourceRevision is used");
  }
});

test("a zero-fragment view still reports its real insertions and module kind", () => {
  const request = pageBasicRequest();
  request.fragments = [];
  const result = analyze(request, analysisConfig);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.ok(result.response.insertions.length > 0, "the probe insertion is still reported");
  assert.equal(result.response.moduleKind.after, "script");
});

test("an unmapped in-mount diagnostic downgrades diagnostics, not hover", () => {
  const states = finishedStates({
    build: { buildStatus: "supported", jsCheckApplied: "inherit", reason: undefined } as never,
    policy: "inherit",
    probeProblems: [],
    introduced: [],
    noCheck: false,
    droppedDefinitions: 0,
    unmappedDiagnostics: 2,
    missing: 0,
    unexpected: 0,
  });
  assert.equal(states.diagnostics.status, "partial");
  assert.match(states.diagnostics.reason ?? "", /cannot be attributed to WXML source/);
  assert.equal(states.hover.status, "supported");
  assert.equal(states.definition.status, "supported");
});

test("a dropped definition downgrades only the definition capability", () => {
  const states = finishedStates({
    build: { buildStatus: "supported", jsCheckApplied: "inherit", reason: undefined } as never,
    policy: "inherit",
    probeProblems: [],
    introduced: [],
    noCheck: false,
    droppedDefinitions: 1,
    unmappedDiagnostics: 0,
    missing: 0,
    unexpected: 0,
  });
  assert.equal(states.definition.status, "partial");
  assert.equal(states.hover.status, "supported");
  assert.equal(states.diagnostics.status, "supported");
});

test("one expression may appear under two fragment ids", () => {
  const request = pageBasicRequest();
  request.fragments = [
    fragment({ fragmentId: "f0", exprText: "title" }),
    fragment({ fragmentId: "f1", exprText: "title" }),
  ];
  const result = analyze(request, analysisConfig);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.mounts.length, 2);
  assert.deepEqual(result.mounts.map((mount) => mount.mountId).sort(), ["f0#0", "f1#0"]);
  assert.deepEqual(
    result.response.fragments.map((entry) => entry.fragmentId).sort(),
    ["f0", "f1"],
  );
});

test("duplicate fragment ids are a structured error", () => {
  const request = pageBasicRequest();
  request.fragments = [fragment({ fragmentId: "f0" }), fragment({ fragmentId: "f0" })];
  assert.throws(
    () => analyze(request, analysisConfig),
    (error: unknown) => error instanceof RequestError && error.code === "bad-fragment",
  );
});

test("a missing snapshot file is Unavailable, never read from disk", () => {
  const request = pageBasicRequest();
  request.files = request.files.filter((file) => file.fileId !== request.scriptFileId);
  const result = analyze(request, analysisConfig);
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.match(result.reason, /not part of files\[\]/);
  const states = result.response.states;
  assert.equal(states.hover.status, "unavailable");
  assert.equal(states.diagnostics.status, "unavailable");
  assert.equal(result.response.fragments.length, 0);
});

test("a malformed request is rejected before analysis", () => {
  const request = pageBasicRequest();
  request.protocolVersion = 3;
  assert.throws(
    () => validateRequest(request),
    (error: unknown) => error instanceof RequestError && error.code === "bad-protocol",
  );
});
