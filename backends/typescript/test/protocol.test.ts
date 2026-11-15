import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import { BACKEND_DIR, SAMPLES_DIR } from "../src/env.ts";
import {
  DEFAULT_LIMITS,
  effectiveLimits,
  optionsDigest,
  RequestError,
  SERVER_LIMITS,
  toCompilerOptions,
  validateRequest,
} from "../src/protocol.ts";
import { fileIdOf, inspectLayout, inspectRequest, runRequest, runScenario, type RunConfig } from "../src/run.ts";
import { scenarioById } from "../src/scenarios.ts";

const config: RunConfig = { backendDir: BACKEND_DIR, samplesDir: SAMPLES_DIR };

test("the request carries files, fragments, options, policy and owner", () => {
  const spec = scenarioById("alias-project");
  const request = inspectRequest(spec, "in-place-shadow", config);
  assert.equal(request.protocolVersion, 1);
  assert.equal(request.view.owner.kind, "page");
  assert.equal(request.view.owner.fileId, "index.ts");
  assert.ok(request.view.viewId.length > 0);
  assert.equal(request.policy.jsCheck, "inherit");
  assert.equal(request.policy.propertiesOverlay, "declaration");
  assert.equal(request.project.typings.version, "5.3.1");
  assert.ok(request.project.projectRoot.endsWith("alias-project"));

  // Effective options come from the project layer (tsconfig extends + paths).
  assert.equal(request.project.compilerOptions.strict, true);
  assert.equal(request.project.compilerOptions.target, "ES2022");
  assert.ok(request.project.compilerOptions.baseUrl);
  assert.deepEqual(request.project.compilerOptions.paths, { "@models/*": ["models/*"] });

  // Files and fragments are inputs, not outputs.
  const fileIds = request.files.map((file) => file.fileId).sort();
  assert.deepEqual(fileIds, ["index.ts", "models/user.ts"]);
  assert.ok(request.files.every((file) => file.text.length > 0 && file.sourceRevision >= 1));
  assert.equal(request.fragments.length, 1);
  const fragment = request.fragments[0]!;
  assert.equal(fragment.fragmentId, "user.name");
  assert.equal(fragment.exprText, "user.name");
  assert.equal(fragment.root, "page");
  assert.ok(fragment.fragmentRevision >= 1);
  assert.ok(fragment.wxmlRange.end > fragment.wxmlRange.start);
  assert.equal(request.limits?.maxFiles, DEFAULT_LIMITS.maxFiles);
});

test("the option digest is independent of the checkout path", () => {
  const base = {
    strict: true,
    allowJs: true,
    checkJs: false,
    target: "ES2022" as const,
    module: "ESNext" as const,
    moduleResolution: "bundler" as const,
    types: ["miniprogram-api-typings"],
  };
  assert.equal(
    optionsDigest({ ...base, baseUrl: "/work/a/miniapp" }, "/work/a/miniapp"),
    optionsDigest({ ...base, baseUrl: "/tmp/copy/miniapp" }, "/tmp/copy/miniapp"),
  );
});

test("input limits are enforced before any language-service work", () => {
  const alias = inspectRequest(scenarioById("alias-project"), "in-place-shadow", config);
  const page = inspectRequest(scenarioById("page-basic"), "in-place-shadow", config);

  assert.throws(
    () => validateRequest({ ...alias, limits: { ...DEFAULT_LIMITS, maxFiles: 0 } }),
    (error: unknown) => error instanceof RequestError && error.code === "bad-limit",
  );
  assert.throws(
    () => validateRequest({ ...alias, limits: { ...DEFAULT_LIMITS, maxFiles: 1 } }),
    (error: unknown) => error instanceof RequestError && error.code === "too-many-files",
  );
  assert.throws(
    () => validateRequest({ ...alias, limits: { ...DEFAULT_LIMITS, maxTotalBytes: 10 } }),
    (error: unknown) => error instanceof RequestError && error.code === "too-many-bytes",
  );
  assert.throws(
    () => validateRequest({ ...page, limits: { ...DEFAULT_LIMITS, maxFragments: 1 } }),
    (error: unknown) => error instanceof RequestError && error.code === "too-many-fragments",
  );
  const fileBytes = page.files.reduce((sum, file) => sum + Buffer.byteLength(file.text, "utf8"), 0);
  assert.ok(page.fragments.some((fragment) => fragment.exprText.length > 0));
  assert.throws(
    () => validateRequest({ ...page, limits: { ...DEFAULT_LIMITS, maxTotalBytes: fileBytes } }),
    (error: unknown) => error instanceof RequestError && error.code === "too-many-bytes",
    "fragments[].exprText bytes count toward maxTotalBytes",
  );
  assert.doesNotThrow(() => validateRequest(alias));
});

test("unknown compiler options are rejected instead of silently dropped", () => {
  const alias = inspectRequest(scenarioById("alias-project"), "in-place-shadow", config);
  const withOption = (options: Record<string, unknown>) => ({
    ...alias,
    project: { ...alias.project, compilerOptions: { ...alias.project.compilerOptions, ...options } },
  });
  assert.throws(
    () => validateRequest(withOption({ allowImportingTsExtensions: true }) as typeof alias),
    (error: unknown) => error instanceof RequestError && error.code === "unsupported-option",
  );
  assert.doesNotThrow(() =>
    validateRequest(
      withOption({
        strictNullChecks: false,
        noUncheckedIndexedAccess: true,
        strictBuiltinIteratorReturn: true,
      }) as typeof alias,
    ),
  );
  const converted = toCompilerOptions({
    ...alias.project.compilerOptions,
    strictBuiltinIteratorReturn: true,
  });
  assert.equal(converted.strictBuiltinIteratorReturn, true, "new strict-family switches must pass through");
});

test("the response reports registration, applied policy and request digest", () => {
  const run = runScenario(scenarioById("multi-registration"), "in-place-shadow", config);
  assert.deepEqual(run.registration, { kind: "page", count: 2, topLevel: true });
  assert.equal(run.jsCheckApplied, "inherit");
  assert.match(run.request.compilerOptionsDigest, /^fnv1a:/);
  assert.equal(run.request.fileCount, 1);
  assert.equal(run.request.fragmentCount, 1);

  const jsOff = runScenario(scenarioById("js-source-check-off"), "in-place-shadow", config);
  assert.equal(jsOff.request.jsCheckPolicy, "off");
  assert.equal(jsOff.jsCheckApplied, "off");
});

test("explicit off neutralises a source `@ts-check` in the derived view", () => {
  const build = inspectLayout(scenarioById("js-source-check-off"), "in-place-shadow", config);
  const neutralise = build.insertions.find((insertion) => insertion.insertionId === "nocheck");
  assert.ok(neutralise, "a neutralising pragma must be inserted");
  assert.ok(build.notes.some((note) => note.includes("neutralised")));
  const text = build.files[0]!.text;
  assert.match(text, /@ts-check[\s\S]*@ts-nocheck/, "the neutralising pragma comes after the source pragma");
  assert.equal(build.jsCheckApplied, "off");
});

test("limits merge with defaults and respect the server ceiling", () => {
  assert.deepEqual(effectiveLimits(undefined), {
    maxFiles: DEFAULT_LIMITS.maxFiles,
    maxTotalBytes: DEFAULT_LIMITS.maxTotalBytes,
    maxFragments: DEFAULT_LIMITS.maxFragments,
  });
  assert.deepEqual(effectiveLimits({}), effectiveLimits(undefined), "a partial limits object must not disable checks");
  const capped = effectiveLimits({ maxFiles: 10_000, maxTotalBytes: 2 * SERVER_LIMITS.maxTotalBytes });
  assert.equal(capped.maxFiles, SERVER_LIMITS.maxFiles, "the caller cannot widen a limit");
  assert.equal(capped.maxTotalBytes, SERVER_LIMITS.maxTotalBytes);

  const request = inspectRequest(scenarioById("alias-project"), "in-place-shadow", config);
  assert.doesNotThrow(() => validateRequest({ ...request, limits: {} }));
  assert.throws(
    () => validateRequest({ ...request, limits: { maxFiles: 1.5 } as typeof request.limits }),
    (error: unknown) => error instanceof RequestError && error.code === "bad-limit",
  );
});

test("enum values and the protocol version are validated", () => {
  const request = inspectRequest(scenarioById("alias-project"), "in-place-shadow", config);
  const withOptions = (options: Record<string, unknown>) => ({
    ...request,
    project: { ...request.project, compilerOptions: { ...request.project.compilerOptions, ...options } },
  });
  for (const options of [{ target: "ES2021" }, { module: "Bogus" }, { moduleResolution: "classic" }, { jsx: "vue" }]) {
    assert.throws(
      () => validateRequest(withOptions(options) as typeof request),
      (error: unknown) => error instanceof RequestError && error.code === "bad-option",
      JSON.stringify(options),
    );
  }
  assert.doesNotThrow(() => validateRequest(withOptions({ target: "ES2020", jsx: "react-jsx" }) as typeof request));
  assert.throws(
    () => validateRequest({ ...request, protocolVersion: 2 }),
    (error: unknown) => error instanceof RequestError && error.code === "bad-protocol",
  );
});

test("the request drives mounted fragments and policy", () => {
  const spec = scenarioById("page-basic");
  const request = inspectRequest(spec, "in-place-shadow", config);
  assert.equal(request.fragments.length, 3);

  const subset = { ...request, fragments: request.fragments.slice(0, 1) };
  const subsetRun = runRequest(subset, spec, "in-place-shadow", config);
  assert.equal(subsetRun.mounts.length, 1, "only the request fragments are mounted");
  assert.equal(subsetRun.hovers.length, 1);
  assert.equal(subsetRun.diagnostics.length, 0);
  assert.deepEqual(subsetRun.mounts[0]?.target, "title");

  const empty = { ...request, fragments: [] };
  const emptyRun = runRequest(empty, spec, "in-place-shadow", config);
  assert.equal(emptyRun.mounts.length, 0);
  assert.equal(emptyRun.hovers.length, 0);
  assert.equal(emptyRun.request.fragmentCount, 0);

  const policySpec = scenarioById("js-source-check-inherit");
  const inherit = inspectRequest(policySpec, "in-place-shadow", config);
  const off = { ...inherit, policy: { ...inherit.policy, jsCheck: "off" as const } };
  const offRun = runRequest(off, policySpec, "in-place-shadow", config);
  assert.equal(offRun.jsCheckApplied, "off");
  assert.equal(offRun.states.diagnostics.status, "disabled");
  assert.equal(offRun.diagnostics.length, 0, "policy from the request must reach the layout");
});

test("the C1 response is assembled from the run", () => {
  const spec = scenarioById("page-basic");
  const request = inspectRequest(spec, "in-place-shadow", config);
  const run = runScenario(spec, "in-place-shadow", config);
  assert.equal(run.response.protocolVersion, 1);
  assert.equal(run.response.requestId, request.requestId);
  assert.deepEqual(run.response.states, run.states);
  assert.equal(run.response.moduleKind.before, "script");
  assert.equal(run.response.moduleKind.after, "script");
  assert.equal(run.response.insertions.length, run.insertions.length);
  assert.equal(run.response.mounts.length, run.mounts.length);
  assert.equal(run.response.mounts[0]?.targetLengthUtf16, run.mounts[0]?.targetLengthUtf16);

  const diagnostics = run.response.fragments.find((fragment) => fragment.fragmentId === "user.nmae");
  assert.ok(diagnostics);
  assert.deepEqual(diagnostics.diagnostics[0]?.range, { start: 134, end: 138 });
  assert.ok(!("spanUtf16" in (diagnostics.diagnostics[0] as object)), "internal spans must not be published");

  const component = runScenario(scenarioById("component-properties"), "in-place-shadow", config);
  const definition = component.response.fragments.find((fragment) => fragment.fragmentId === "title")?.definition;
  assert.equal(definition?.kind, "generated");
  assert.equal(definition?.fileId, "index.ts");
  assert.deepEqual(definition?.range, { start: 77, end: 82 });
  assert.ok(!("startUtf16" in (definition as object)), "definition coordinates are bytes only");
});

test("response file identities use the request's fileId space", () => {
  const spec = scenarioById("page-basic");
  const request = inspectRequest(spec, "in-place-shadow", config);
  const run = runScenario(spec, "in-place-shadow", config);
  assert.equal(request.scriptFileId, "index.ts");
  assert.ok(run.response.insertions.length > 0);
  assert.ok(
    run.response.insertions.every((insertion) => insertion.file === request.scriptFileId),
    "insertions must use the script fileId",
  );
  assert.ok(
    run.response.mounts.every((mount) => mount.file === request.scriptFileId),
    "mounts must use the script fileId",
  );
  assert.equal(fileIdOf(request, path.join(SAMPLES_DIR, spec.dir, "index.ts")), "index.ts");
});

test("BOM + pragma + probe insertions share one fileId", () => {
  const spec = scenarioById("js-bom-check-off");
  const request = inspectRequest(spec, "in-place-shadow", config);
  const run = runScenario(spec, "in-place-shadow", config);
  assert.deepEqual(
    run.response.insertions.map((insertion) => insertion.insertionId).sort(),
    ["bom", "nocheck", "probe"],
  );
  assert.ok(run.response.insertions.every((insertion) => insertion.file === "index.js"));
  assert.equal(request.scriptFileId, "index.js");
});

test("alias-project keeps project-relative ids and the model definition fileId", () => {
  const spec = scenarioById("alias-project");
  const request = inspectRequest(spec, "in-place-shadow", config);
  const run = runScenario(spec, "in-place-shadow", config);
  assert.ok(run.response.insertions.every((insertion) => insertion.file === "index.ts"));
  assert.ok(run.response.mounts.every((mount) => mount.file === "index.ts"));
  const definition = run.response.fragments.find((fragment) => fragment.fragmentId === "user.name")?.definition;
  assert.equal(definition?.kind, "external");
  assert.equal(definition?.fileId, "models/user.ts");
  assert.ok(!run.response.insertions.some((insertion) => insertion.file.includes("samples/")));
  assert.equal(request.scriptFileId, "index.ts");
});

test("candidate B generated files use a synthetic fileId and definitions point at source", () => {
  const spec = scenarioById("component-properties");
  const request = inspectRequest(spec, "symbol-bridge", config);
  const run = runScenario(spec, "symbol-bridge", config);
  const requestFileIds = new Set(request.files.map((file) => file.fileId));
  assert.ok(run.response.mounts.length > 0);
  assert.ok(
    run.response.mounts.every((mount) => mount.file === "__wxml_view_0.ts" && !requestFileIds.has(mount.file)),
    "derived check file is a documented synthetic id",
  );
  const definition = run.response.fragments.find((fragment) => fragment.fragmentId === "title")?.definition;
  assert.equal(definition?.kind, "generated");
  assert.equal(definition?.fileId, request.scriptFileId, "generated members publish the original script id");
  assert.deepEqual(definition?.range, { start: 77, end: 82 });
});

test("lib and new strict-family switches pass through the request", () => {
  const options = {
    strict: true,
    allowJs: false,
    checkJs: false,
    target: "ES2022" as const,
    module: "ESNext" as const,
    moduleResolution: "bundler" as const,
    lib: ["ES2022", "DOM"],
    strictBuiltinIteratorReturn: true,
  };
  const converted = toCompilerOptions(options);
  assert.deepEqual(converted.lib, ["ES2022", "DOM"]);
  assert.equal(converted.strictBuiltinIteratorReturn, true);
});

test("fragment ids from the request are echoed, not replaced by expressions", () => {
  const spec = scenarioById("page-basic");
  const request = inspectRequest(spec, "in-place-shadow", config);
  const renamed = {
    ...request,
    fragments: request.fragments.map((fragment, index) => ({ ...fragment, fragmentId: `f${index}` })),
  };
  const run = runRequest(renamed, spec, "in-place-shadow", config);
  assert.deepEqual(
    run.response.fragments.map((fragment) => fragment.fragmentId),
    ["f0", "f1", "f2"],
    "response fragment ids come from the request",
  );
  assert.deepEqual(
    run.response.mounts.map((mount) => mount.mountId).sort(),
    ["f0#0", "f1#0", "f2#0"],
  );
  assert.ok(run.response.mounts.every((mount) => renamed.fragments.some((fragment) => fragment.fragmentId === mount.fragmentId)));
});

test("the same expression may be mounted under two fragment ids", () => {
  const spec = scenarioById("page-basic");
  const request = inspectRequest(spec, "in-place-shadow", config);
  const title = request.fragments.find((fragment) => fragment.exprText === "title");
  assert.ok(title);
  const duplicated = {
    ...request,
    fragments: [
      { ...title, fragmentId: "f0" },
      { ...title, fragmentId: "f1", wxmlRange: { start: title.wxmlRange.start, end: title.wxmlRange.end } },
      ...request.fragments.filter((fragment) => fragment.exprText !== "title"),
    ],
  };
  assert.doesNotThrow(() => runRequest(duplicated, spec, "in-place-shadow", config));
  const run = runRequest(duplicated, spec, "in-place-shadow", config);
  const titleMounts = run.response.mounts.filter((mount) => mount.fragmentId === "f0" || mount.fragmentId === "f1");
  assert.equal(titleMounts.length, 2, "each request fragment gets its own mount");
  assert.deepEqual(titleMounts.map((mount) => mount.mountId).sort(), ["f0#0", "f1#0"]);
  assert.deepEqual(
    run.response.fragments.filter((fragment) => fragment.fragmentId === "f0" || fragment.fragmentId === "f1").map((fragment) => fragment.fragmentId),
    ["f0", "f1"],
  );
});

test("duplicate fragment ids are a structured request error, not a crash", () => {
  const spec = scenarioById("page-basic");
  const request = inspectRequest(spec, "in-place-shadow", config);
  const title = request.fragments[0]!;
  const duplicated = { ...request, fragments: [title, { ...title }] };
  assert.doesNotThrow(() => runRequest(duplicated, spec, "in-place-shadow", config));
  const run = runRequest(duplicated, spec, "in-place-shadow", config);
  assert.equal(run.states.hover.status, "unavailable");
  assert.match(run.states.hover.reason ?? "", /bad-fragment/);
  assert.equal(run.response.fragments.length, 0);
});

test("C1.1 fragments accept target/locals and reject malformed ones", () => {
  const request = inspectRequest(scenarioById("page-basic"), "in-place-shadow", config);
  const base = request.fragments[0]!;

  assert.doesNotThrow(() => validateRequest({ ...request, fragments: [{ ...base, target: "title" }] }));
  assert.doesNotThrow(() =>
    validateRequest({
      ...request,
      fragments: [
        {
          ...base,
          exprText: "item.name",
          target: "item",
          locals: ["item"],
          wxmlRange: { start: 0, end: Buffer.byteLength("item.name", "utf8") },
        },
      ],
    }),
  );

  const invalid: Array<Record<string, unknown>> = [
    { target: "nmae" },
    { target: "" },
    { locals: "item" },
    { locals: ["item", "item"] },
    { locals: [""] },
    { locals: [1] },
    { locals: ["__wxml_root"] },
    { locals: ["not an identifier"] },
  ];
  for (const overrides of invalid) {
    assert.throws(
      () => validateRequest({ ...request, fragments: [{ ...base, ...overrides }] as typeof request.fragments }),
      (error: unknown) => error instanceof RequestError && error.code === "bad-fragment",
      JSON.stringify(overrides),
    );
  }
});

test("legacy fragments without target/locals still validate", () => {
  const request = inspectRequest(scenarioById("page-basic"), "in-place-shadow", config);
  const legacy = request.fragments.map(({ target, locals, ...rest }) => rest);
  assert.doesNotThrow(() => validateRequest({ ...request, fragments: legacy as typeof request.fragments }));
});

test("files[], view and analysisStamp are validated before analysis", () => {
  const bad: Array<[string, (value: any) => void]> = [
    ["absolute path", (value) => { value.files[0].path = "/etc/passwd"; value.files[0].fileId = "/etc/passwd"; }],
    ["parent path", (value) => { value.files[0].path = "../evil.ts"; value.files[0].fileId = "../evil.ts"; }],
    ["nul path", (value) => { value.files[0].path = "a\u0000b.ts"; value.files[0].fileId = "a\u0000b.ts"; }],
    ["duplicate fileId", (value) => { value.files[1] = { ...value.files[0] }; }],
    ["kind mismatch", (value) => { value.files[0].kind = "js"; }],
    ["fileId != path", (value) => { value.files[0].fileId = "other.ts"; }],
    ["sourceRevision", (value) => { value.files[0].sourceRevision = 0; }],
    ["bad owner kind", (value) => { value.view.owner.kind = "widget"; }],
    ["empty requestId", (value) => { value.requestId = ""; }],
    ["empty viewId", (value) => { value.view.viewId = ""; }],
    ["owner fileId mismatch", (value) => { value.view.owner.fileId = "other.ts"; }],
    ["owner fileId missing", (value) => { delete value.view.owner.fileId; }],
    ["compilerOptions array", (value) => { value.project.compilerOptions = []; }],
    ["compilerOptions paths string", (value) => { value.project.compilerOptions.paths = "x"; }],
    ["compilerOptions paths entry", (value) => { value.project.compilerOptions.paths = { "@/*": [1] }; }],
    ["compilerOptions lib string", (value) => { value.project.compilerOptions.lib = "ES2022"; }],
    ["compilerOptions types item", (value) => { value.project.compilerOptions.types = [1]; }],
    ["compilerOptions typeRoots item", (value) => { value.project.compilerOptions.typeRoots = [null]; }],
    ["compilerOptions baseUrl", (value) => { value.project.compilerOptions.baseUrl = 1; }],
    ["compilerOptions rootDir", (value) => { value.project.compilerOptions.rootDir = false; }],
    ["compilerOptions boolean", (value) => { value.project.compilerOptions.strict = "yes"; }],
    ["fractional stamp", (value) => { value.analysisStamp.snapshotId = 1.5; }],
    ["typings version", (value) => { value.project.typings.version = "5.2.0"; }],
    ["overlay off", (value) => { value.policy.propertiesOverlay = "off"; }],
  ];
  for (const [label, mutate] of bad) {
    const value = cloneRequest();
    mutate(value);
    assert.throws(
      () => validateRequest(value),
      (error: unknown) => error instanceof RequestError && ["bad-params", "bad-option", "unsupported-option"].includes(error.code),
      label,
    );
  }
});

test("propertiesOverlay off is rejected explicitly, not ignored", () => {
  const request = cloneRequest();
  (request.policy as unknown as { propertiesOverlay: string }).propertiesOverlay = "off";
  assert.throws(
    () => validateRequest(request),
    (error: unknown) => error instanceof RequestError && error.code === "unsupported-option",
  );
});

test("invalid binding names are rejected in locals", () => {
  const request = cloneRequest();
  for (const name of ["class", "return", "eval", "arguments", "undefined", "NaN", "Infinity", "__WxmlProps_x", "__wxml_root"]) {
    assert.throws(
      () => validateRequest({ ...request, fragments: [{ ...request.fragments[0]!, locals: [name] }] }),
      (error: unknown) => error instanceof RequestError && error.code === "bad-fragment",
      name,
    );
  }
  for (const name of ["item", "$row", "_i"]) {
    assert.doesNotThrow(() =>
      validateRequest({
        ...request,
        fragments: [
          {
            ...request.fragments[0]!,
            exprText: name,
            target: name,
            locals: [name],
            wxmlRange: { start: 0, end: Buffer.byteLength(name, "utf8") },
          },
        ],
      }),
    );
  }
});

function cloneRequest() {
  return structuredClone(inspectRequest(scenarioById("page-basic"), "in-place-shadow", config));
}
