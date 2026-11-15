/**
 * Evidence harness around the production analyzer.
 *
 * `analyze()` in `analysis.ts` is the real backend entry point. This module maps
 * scenario expectations onto it so the P4 evidence matrix keeps working, and it
 * exposes request/layout inspection helpers for tests.
 */

import fs from "node:fs";
import path from "node:path";
import ts from "typescript";
import { identifierOffset } from "./expression.ts";
import {
  analyze,
  applyMismatch,
  buildView,
  byteRange,
  classifyDefinitionTarget,
  fileIdOf,
  locateExpression,
  originalToShadowOffset,
  shadowToOriginalOffset,
  toAnalyzeResponse,
  type AnalysisConfig,
} from "./analysis.ts";
import {
  DEFAULT_LIMITS,
  fileKindOf,
  PROTOCOL_VERSION,
  RequestError,
  type AnalyzeRequest,
  type AnalyzeResponse,
  type FileKind,
  type JsxName,
  type ModuleDetectionName,
  type ModuleKindName,
  type ModuleResolutionName,
  type RequestCompilerOptions,
  type RequestFile,
  type RequestFragment,
  type TargetName,
} from "./protocol.ts";
import type {
  CapabilityStates,
  LayoutBuild,
  LayoutId,
  RunOverrides,
  ScenarioRun,
  ScenarioSpec,
} from "./types.ts";

export interface RunConfig {
  /** Project root used as the compiler's current directory (holds node_modules). */
  backendDir: string;
  /** Directory that holds the sample projects. */
  samplesDir: string;
}

export { classifyDefinitionTarget, fileIdOf, originalToShadowOffset, shadowToOriginalOffset, toAnalyzeResponse };
export type { AnalyzeRequest, AnalyzeResponse };

/* ------------------------------------------------------------------ request */

function requestOptions(spec: ScenarioSpec, dir: string): { options: RequestCompilerOptions; files?: string[] } {
  const defaults: RequestCompilerOptions = {
    strict: true,
    allowJs: spec.allowJs,
    checkJs: spec.checkJs,
    target: "ES2022",
    module: "ESNext",
    moduleResolution: "bundler",
    types: ["miniprogram-api-typings"],
  };
  if (!spec.tsconfig) {
    return { options: { ...defaults, ...spec.compilerOptions } };
  }
  const configPath = path.join(dir, spec.tsconfig);
  const read = ts.readConfigFile(configPath, ts.sys.readFile);
  if (read.error) {
    throw new Error(`cannot read ${spec.tsconfig}: ${ts.flattenDiagnosticMessageText(read.error.messageText, " | ")}`);
  }
  const parsed = ts.parseJsonConfigFileContent(read.config, ts.sys, path.dirname(configPath));
  if (parsed.errors.length > 0) {
    throw new Error(
      `invalid ${spec.tsconfig}: ${parsed.errors
        .map((error) => ts.flattenDiagnosticMessageText(error.messageText, " | "))
        .join("; ")}`,
    );
  }
  return { options: { ...defaults, ...fromTsOptions(parsed.options), ...spec.compilerOptions }, files: parsed.fileNames };
}

const TARGET_NAMES: Array<[ts.ScriptTarget, TargetName]> = [
  [ts.ScriptTarget.ES2020, "ES2020"],
  [ts.ScriptTarget.ES2022, "ES2022"],
  [ts.ScriptTarget.ESNext, "ESNext"],
];

const MODULE_NAMES: Array<[ts.ModuleKind, ModuleKindName]> = [
  [ts.ModuleKind.ESNext, "ESNext"],
  [ts.ModuleKind.CommonJS, "CommonJS"],
  [ts.ModuleKind.Preserve, "Preserve"],
];

const RESOLUTION_NAMES: Array<[ts.ModuleResolutionKind, ModuleResolutionName]> = [
  [ts.ModuleResolutionKind.Bundler, "bundler"],
  [ts.ModuleResolutionKind.Node16, "node16"],
  [ts.ModuleResolutionKind.NodeNext, "nodenext"],
  [ts.ModuleResolutionKind.Node10, "node10"],
];

const JSX_NAMES: Array<[ts.JsxEmit, JsxName]> = [
  [ts.JsxEmit.Preserve, "preserve"],
  [ts.JsxEmit.React, "react"],
  [ts.JsxEmit.ReactJSX, "react-jsx"],
  [ts.JsxEmit.ReactJSXDev, "react-jsxdev"],
  [ts.JsxEmit.ReactNative, "react-native"],
];

const MODULE_DETECTION_NAMES: Array<[ts.ModuleDetectionKind, ModuleDetectionName]> = [
  [ts.ModuleDetectionKind.Auto, "auto"],
  [ts.ModuleDetectionKind.Legacy, "legacy"],
  [ts.ModuleDetectionKind.Force, "force"],
];

/** Copy every effective option the contract models; never drop a switch. */
function fromTsOptions(options: ts.CompilerOptions): RequestCompilerOptions {
  const pick = <T, U>(table: Array<[T, U]>, value: T | undefined, fallback: U): U =>
    table.find(([key]) => key === value)?.[1] ?? fallback;
  return {
    ...(options.strict !== undefined ? { strict: options.strict } : {}),
    ...(options.strictNullChecks !== undefined ? { strictNullChecks: options.strictNullChecks } : {}),
    ...(options.strictFunctionTypes !== undefined ? { strictFunctionTypes: options.strictFunctionTypes } : {}),
    ...(options.strictBindCallApply !== undefined ? { strictBindCallApply: options.strictBindCallApply } : {}),
    ...(options.strictPropertyInitialization !== undefined
      ? { strictPropertyInitialization: options.strictPropertyInitialization }
      : {}),
    ...(options.noImplicitAny !== undefined ? { noImplicitAny: options.noImplicitAny } : {}),
    ...(options.noImplicitThis !== undefined ? { noImplicitThis: options.noImplicitThis } : {}),
    ...(options.noUncheckedIndexedAccess !== undefined
      ? { noUncheckedIndexedAccess: options.noUncheckedIndexedAccess }
      : {}),
    ...(options.exactOptionalPropertyTypes !== undefined
      ? { exactOptionalPropertyTypes: options.exactOptionalPropertyTypes }
      : {}),
    ...(options.strictBuiltinIteratorReturn !== undefined
      ? { strictBuiltinIteratorReturn: options.strictBuiltinIteratorReturn }
      : {}),
    ...(options.useUnknownInCatchVariables !== undefined
      ? { useUnknownInCatchVariables: options.useUnknownInCatchVariables }
      : {}),
    ...(options.allowJs !== undefined ? { allowJs: options.allowJs } : {}),
    ...(options.checkJs !== undefined ? { checkJs: options.checkJs } : {}),
    ...(options.esModuleInterop !== undefined ? { esModuleInterop: options.esModuleInterop } : {}),
    ...(options.allowSyntheticDefaultImports !== undefined
      ? { allowSyntheticDefaultImports: options.allowSyntheticDefaultImports }
      : {}),
    ...(options.resolveJsonModule !== undefined ? { resolveJsonModule: options.resolveJsonModule } : {}),
    ...(options.isolatedModules !== undefined ? { isolatedModules: options.isolatedModules } : {}),
    ...(options.verbatimModuleSyntax !== undefined ? { verbatimModuleSyntax: options.verbatimModuleSyntax } : {}),
    ...(options.forceConsistentCasingInFileNames !== undefined
      ? { forceConsistentCasingInFileNames: options.forceConsistentCasingInFileNames }
      : {}),
    ...(options.skipLibCheck !== undefined ? { skipLibCheck: options.skipLibCheck } : {}),
    target: pick(TARGET_NAMES, options.target, "ES2022"),
    module: pick(MODULE_NAMES, options.module, "ESNext"),
    moduleResolution: pick(RESOLUTION_NAMES, options.moduleResolution, "bundler"),
    ...(options.jsx !== undefined ? { jsx: pick(JSX_NAMES, options.jsx, "preserve") } : {}),
    ...(options.moduleDetection !== undefined
      ? { moduleDetection: pick(MODULE_DETECTION_NAMES, options.moduleDetection, "auto") }
      : {}),
    ...(options.lib ? { lib: options.lib } : {}),
    ...(options.types ? { types: options.types } : {}),
    ...(options.typeRoots ? { typeRoots: options.typeRoots } : {}),
    ...(options.rootDir ? { rootDir: options.rootDir } : {}),
    ...(options.baseUrl ? { baseUrl: options.baseUrl } : {}),
    ...(options.paths ? { paths: options.paths } : {}),
  };
}

function requestFragments(spec: ScenarioSpec, repeat: number): RequestFragment[] {
  // `queries[].target` is the request target; `expectedDiagnostics[].target` only
  // locates a diagnostic in WXML (it may be an index expression like `users[0]`)
  // and is never sent as a C1.1 target.
  const targets = new Map<string, string | undefined>();
  for (const query of spec.queries) targets.set(query.expr, query.target);
  for (const diagnostic of spec.expectedDiagnostics) {
    if (targets.has(diagnostic.expr)) continue;
    // `expectedDiagnostics[].target` locates a diagnostic in WXML; it is only a
    // legal request target when it is a whole identifier in the expression
    // (`users[0]` is not). Otherwise the C1.1 rightmost default applies.
    const target = identifierOffset(diagnostic.expr, diagnostic.target) >= 0 ? diagnostic.target : undefined;
    targets.set(diagnostic.expr, target);
  }
  const fragments: RequestFragment[] = [];
  for (const [expr, target] of targets) {
    const location = locateExpression(spec.wxml, expr, spec.id);
    const range = byteRange(spec.wxml, location.start, expr.length);
    if (!range) throw new Error(`scenario ${spec.id}: fragment ${expr} is not on a character boundary`);
    for (let copy = 0; copy < Math.max(1, repeat); copy += 1) {
      fragments.push({
        fragmentId: repeat > 1 ? `${expr}#${copy}` : expr,
        exprText: expr,
        ...(target === undefined ? {} : { target }),
        wxmlRange: range,
        fragmentRevision: 1,
        root: spec.registration,
      });
    }
  }
  return fragments;
}

function buildRequest(
  spec: ScenarioSpec,
  layoutId: LayoutId,
  config: RunConfig,
  overrides: RunOverrides,
): AnalyzeRequest {
  const dir = path.join(config.samplesDir, spec.dir);
  const scriptPath = path.join(dir, spec.script);
  const scriptText = overrides.scriptText ?? readSample(config, spec, spec.script);
  const resolved = requestOptions(spec, dir);

  const files = new Map<string, string>();
  const fileList = resolved.files ?? (spec.extraFiles ?? []).map((extra) => path.join(dir, extra));
  for (const filePath of [...fileList, scriptPath]) {
    const relative = path.relative(dir, filePath);
    const text =
      overrides.files?.[relative] ?? (filePath === scriptPath ? scriptText : readSample(config, spec, relative));
    files.set(filePath, text);
  }
  files.set(scriptPath, scriptText);
  for (const [relative, text] of Object.entries(overrides.files ?? {})) {
    files.set(path.join(dir, relative), text);
  }
  const requestFiles: RequestFile[] = [...files].map(([filePath, text]) => ({
    fileId: path.relative(dir, filePath),
    path: path.relative(dir, filePath),
    kind: fileKindOf(filePath) as FileKind,
    text,
    sourceRevision: 1,
  }));

  return {
    protocolVersion: PROTOCOL_VERSION,
    requestId: `${spec.id}:${layoutId}:1`,
    analysisStamp: { snapshotId: 1, projectRevision: 1, viewRevision: 1, backendEpoch: 1 },
    view: {
      viewId: `${spec.id}:${layoutId}`,
      owner: { kind: spec.registration, fileId: path.relative(dir, scriptPath) },
    },
    project: {
      projectRoot: dir,
      compilerOptions: resolved.options,
      typings: { package: "miniprogram-api-typings", version: "5.3.1" },
    },
    scriptFileId: path.relative(dir, scriptPath),
    files: requestFiles,
    fragments: requestFragments(spec, Math.max(1, overrides.mountRepeat ?? 1)),
    policy: { jsCheck: spec.jsCheck, propertiesOverlay: "declaration" },
    limits: DEFAULT_LIMITS,
  };
}

/** Exposed for tests that assert insertion/mount/segment structure. */
export function inspectLayout(
  spec: ScenarioSpec,
  layoutId: LayoutId,
  config: RunConfig,
  overrides: RunOverrides = {},
): LayoutBuild {
  const request = buildRequest(spec, layoutId, config, overrides);
  const view = buildView(request, analysisConfig(config, layoutId));
  if (!view.ok) throw new Error(view.reason);
  return view.build;
}

export function inspectRequest(
  spec: ScenarioSpec,
  layoutId: LayoutId,
  config: RunConfig,
  overrides: RunOverrides = {},
): AnalyzeRequest {
  return buildRequest(spec, layoutId, config, overrides);
}

function analysisConfig(config: RunConfig, layoutId: LayoutId): AnalysisConfig {
  return { backendDir: config.backendDir, layoutId };
}

/* ---------------------------------------------------------------------- run */

export function runScenario(
  spec: ScenarioSpec,
  layoutId: LayoutId,
  config: RunConfig,
  overrides: RunOverrides = {},
): ScenarioRun {
  return runRequest(buildRequest(spec, layoutId, config, overrides), spec, layoutId, config);
}

/** Execute an explicit C1 request (the request drives fragments and policy). */
export function runRequest(
  request: AnalyzeRequest,
  spec: ScenarioSpec,
  layoutId: LayoutId,
  config: RunConfig,
): ScenarioRun {
  let result: ReturnType<typeof analyze>;
  try {
    result = analyze(request, analysisConfig(config, layoutId));
  } catch (error) {
    const reason =
      error instanceof RequestError
        ? `request rejected (${error.code}): ${error.message}`
        : `analysis failed: ${error instanceof Error ? error.message : String(error)}`;
    const registration = {
      kind: request.view?.owner?.kind === "component" ? ("component" as const) : ("page" as const),
      count: 0,
      topLevel: null,
    };
    return baseRun(spec, layoutId, reason, registration, request);
  }

  if (!result.ok) {
    return baseRun(spec, layoutId, result.reason, result.registration, request);
  }

  const queryExpressions = new Set([
    ...spec.queries.map((query) => query.expr),
    ...spec.expectedDiagnostics.map((diagnostic) => diagnostic.expr),
  ]);
  const hovers = result.hovers.filter((hover) => queryExpressions.has(hover.query));
  const definitions = result.definitions.filter((definition) => queryExpressions.has(definition.query));

  const expected = spec.expectedDiagnostics;
  const expectedRanges = expected.map((diagnostic) => expectedRange(spec, diagnostic));
  const matched = new Set<number>();
  const unexpected = [];
  for (const observation of result.diagnostics) {
    const index = expectedRanges.findIndex(
      (range) => range !== undefined && observation.range?.start === range.start && observation.range?.end === range.end,
    );
    if (index >= 0) matched.add(index);
    else unexpected.push(observation);
  }
  const missing = expected.filter((_, index) => !matched.has(index));
  const states = applyMismatch(result.states, result.build.buildStatus, missing.length, unexpected.length);

  const run: ScenarioRun = {
    scenario: spec.id,
    layout: layoutId,
    states,
    jsCheckApplied: result.jsCheckApplied,
    moduleBefore: result.moduleBefore,
    moduleAfter: result.moduleAfter,
    perturbsUserTypes: result.perturbsUserTypes,
    userScriptDiagnosticsIntroduced: result.userScriptDiagnosticsIntroduced,
    suppressedDiagnostics: result.suppressedDiagnostics,
    suppressedDetails: result.suppressedDetails,
    probeProblems: result.probeProblems,
    registration: result.registration,
    request: summarizeRequest(request),
    insertions: result.insertions,
    mounts: result.mounts,
    hovers,
    diagnostics: result.diagnostics,
    expectedDiagnostics: expected,
    diagnosticsExact: missing.length === 0 && unexpected.length === 0,
    unexpectedDiagnostics: unexpected,
    missingDiagnostics: missing,
    definitions,
    rootDeclarationSpans: result.rootDeclarationSpans,
    response: toAnalyzeResponse({
      request,
      jsCheckApplied: result.jsCheckApplied,
      registration: result.registration,
      states,
      insertions: result.insertions,
      mounts: result.mounts,
      hovers,
      diagnostics: result.diagnostics,
      definitions,
      moduleBefore: result.moduleBefore,
      moduleAfter: result.moduleAfter,
      suppressedDiagnostics: result.suppressedDiagnostics,
      suppressedDetails: result.suppressedDetails,
    }),
  };
  if (result.notes.length > 0) run.notes = result.notes;
  return run;
}

function summarizeRequest(request: AnalyzeRequest): ScenarioRun["request"] {
  return {
    viewId: request.view.viewId,
    ownerKind: request.view.owner.kind,
    jsCheckPolicy: request.policy.jsCheck,
    fileCount: request.files.length,
    totalBytes: request.files.reduce((sum, file) => sum + Buffer.byteLength(file.text, "utf8"), 0),
    fragmentCount: request.fragments.length,
    compilerOptionsDigest: digestOptions(request.project.compilerOptions, request.project.projectRoot),
  };
}

function digestOptions(options: RequestCompilerOptions, projectRoot: string): string {
  const normalized: Record<string, unknown> = { ...options };
  if (projectRoot && options.baseUrl) {
    normalized.baseUrl = path.relative(projectRoot, options.baseUrl).split(path.sep).join("/") || ".";
  }
  const shape = JSON.stringify(normalized, Object.keys(normalized).sort());
  let hash = 2166136261;
  for (let index = 0; index < shape.length; index += 1) {
    hash ^= shape.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return `fnv1a:${(hash >>> 0).toString(16)}`;
}

function expectedRange(spec: ScenarioSpec, diagnostic: { expr: string; target: string }): { start: number; end: number } | undefined {
  const location = locateExpression(spec.wxml, diagnostic.expr, spec.id);
  return byteRange(spec.wxml, location.start + diagnostic.expr.indexOf(diagnostic.target), diagnostic.target.length);
}

function readSample(config: RunConfig, spec: ScenarioSpec, relative: string): string {
  const filePath = path.join(config.samplesDir, spec.dir, relative);
  try {
    return fs.readFileSync(filePath, "utf8");
  } catch {
    throw new Error(`sample file is missing: ${spec.dir}/${relative}`);
  }
}

/** Run result for a request/environment failure: nothing is guessed. */
function baseRun(
  spec: ScenarioSpec,
  layoutId: LayoutId,
  reason: string,
  registration: ScenarioRun["registration"],
  request: AnalyzeRequest,
): ScenarioRun {
  const states: CapabilityStates = {
    hover: { status: "unavailable", reason },
    diagnostics: { status: "unavailable", reason },
    definition: { status: "unavailable", reason },
  };
  return {
    scenario: spec.id,
    layout: layoutId,
    states,
    jsCheckApplied: "off",
    moduleBefore: "unknown",
    moduleAfter: "unknown",
    perturbsUserTypes: false,
    userScriptDiagnosticsIntroduced: [],
    suppressedDiagnostics: 0,
    suppressedDetails: [],
    probeProblems: [],
    registration,
    request: summarizeRequest(request),
    insertions: [],
    mounts: [],
    hovers: [],
    diagnostics: [],
    expectedDiagnostics: spec.expectedDiagnostics,
    diagnosticsExact: false,
    unexpectedDiagnostics: [],
    missingDiagnostics: spec.expectedDiagnostics,
    definitions: [],
    rootDeclarationSpans: [],
    response: {
      protocolVersion: PROTOCOL_VERSION,
      requestId: request.requestId,
      jsCheckApplied: "off",
      registration,
      states,
      insertions: [],
      mounts: [],
      fragments: [],
      moduleKind: { before: "unknown", after: "unknown" },
      suppressed: { count: 0, details: [] },
    },
  };
}
