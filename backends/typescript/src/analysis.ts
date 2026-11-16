/**
 * Request-driven analysis (P5).
 *
 * `analyze(request)` is the production entry point: it uses only the request
 * (`projectRoot`, `files[]`, `scriptFileId`, `compilerOptions`, `fragments[]`,
 * `policy`, `view`) and never reads project sources or tsconfig from disk. The
 * evidence harness (`run.ts`) wraps this module with scenario expectations.
 */

import path from "node:path";
import ts from "typescript";
import { declarationSpans, findRegistrations, memberMap, scanDeclaredProperties } from "./ast.ts";
import type { DataRootKind } from "./ast.ts";
import {
  assertEnvironment,
  defaultReadPolicy,
  EnvironmentError,
  moduleKind,
  parseSource,
  VirtualProject,
} from "./compiler.ts";
import { classifyDataRoot } from "./dataroot.ts";
import { collectComponentDiagnostics } from "./componentBindings.ts";
import { collectEventResults } from "./events.ts";
import { locateExpression } from "./fragments.ts";
import { buildInPlaceShadow } from "./layouts/inPlaceShadow.ts";
import { buildSymbolBridge } from "./layouts/symbolBridge.ts";
import type { LayoutContext } from "./layouts/context.ts";
import { sanitizeMessage } from "./messages.ts";
import { checkProbeHealth, scanPragmas } from "./probes.ts";
import {
  PROTOCOL_VERSION,
  toCompilerOptions,
  validateRequest,
  type AnalyzeRequest,
  type AnalyzeResponse,
  type RequestCompilerOptions,
} from "./protocol.ts";
import { definitionsAt, hoverAt, semanticDiagnostics } from "./queries.ts";
import { mapSpanToSource } from "./text.ts";
import type {
  CapabilityState,
  CapabilityStates,
  CapabilityStatus,
  DefinitionObservation,
  DiagnosticObservation,
  HoverObservation,
  LayoutBuild,
  LayoutId,
  SourceByteRange,
  TextEdit,
} from "./types.ts";

export interface AnalysisConfig {
  /** Project root used as the compiler's current directory (holds node_modules). */
  backendDir: string;
  /** Candidate layout; production uses the default in-place shadow. */
  layoutId?: LayoutId;
}

export interface AnalysisHoverCandidate {
  /** WXML byte range of the identifier. */
  range: SourceByteRange;
  observation: HoverObservation;
}

export interface AnalysisQuery {
  fragmentId: string;
  /** Expression text (the harness calls it `query`). */
  query: string;
  observation: HoverObservation | null;
  /** Per-identifier hover observations for cursor-accurate LSP hovers. */
  candidates?: AnalysisHoverCandidate[];
}

export interface AnalysisDefinition {
  fragmentId: string;
  query: string;
  observation: DefinitionObservation | null;
}

export interface AnalysisResponseInput {
  request: AnalyzeRequest;
  jsCheckApplied: "inherit" | "pragma" | "off";
  registration: AnalyzeResponse["registration"];
  states: CapabilityStates;
  insertions: AnalyzeResponse["insertions"];
  mounts: AnalyzeResponse["mounts"];
  hovers: AnalysisQuery[];
  diagnostics: DiagnosticObservation[];
  definitions: AnalysisDefinition[];
  moduleBefore: "script" | "module" | "unknown";
  moduleAfter: "script" | "module" | "unknown";
  suppressedDiagnostics: number;
  suppressedDetails: Array<{ code: number; message: string }>;
  /** Custom-component binding results (protocol v2). */
  components?: AnalyzeResponse["components"];
  /** Event handler resolutions (protocol v2). */
  eventResults?: AnalyzeResponse["eventResults"];
}

/** Assemble the C1 response (diagnostics publish WXML bytes only). */
export function toAnalyzeResponse(input: AnalysisResponseInput): AnalyzeResponse {
  const fragmentIds: string[] = [];
  for (const fragment of input.request.fragments) fragmentIds.push(fragment.fragmentId);
  for (const mount of input.mounts) {
    if (!fragmentIds.includes(mount.fragmentId)) fragmentIds.push(mount.fragmentId);
  }

  const fileId = (file: string | undefined): string | undefined =>
    file ? fileIdOf(input.request, file) : undefined;

  const fragments = fragmentIds.map((fragmentId) => {
    const query = input.hovers.find((entry) => entry.fragmentId === fragmentId);
    const hover = query?.observation;
    const candidates = query?.candidates ?? [];
    const definition = input.definitions.find((entry) => entry.fragmentId === fragmentId)?.observation;
    return {
      fragmentId,
      ...(hover
        ? {
            hover: {
              typeText: hover.typeText,
              typeOnly: hover.typeOnly,
              quality: hover.quality,
              documentation: hover.documentation,
              documentationSource: hover.documentationSource,
            },
          }
        : {}),
      ...(candidates.length > 0
        ? {
            hovers: candidates.map((candidate) => ({
              range: candidate.range,
              typeText: candidate.observation.typeText,
              typeOnly: candidate.observation.typeOnly,
              quality: candidate.observation.quality,
              documentation: candidate.observation.documentation,
              documentationSource: candidate.observation.documentationSource,
            })),
          }
        : {}),
      diagnostics: input.diagnostics
        .filter((diagnostic) => diagnostic.fragmentId === fragmentId)
        .map((diagnostic) => ({
          code: diagnostic.code,
          ...(diagnostic.range ? { range: diagnostic.range } : {}),
          message: diagnostic.message,
          attribution: diagnostic.trimmedSyntheticPrefix
            ? ("trimmed-synthetic" as const)
            : ("source" as const),
        })),
      ...(definition
        ? {
            definition: {
              kind: definition.kind,
              ...(definition.kind === "external" || definition.kind === "script" || definition.kind === "generated"
                ? {
                    fileId: fileId(
                      definition.kind === "generated" ? (definition.sourceFile ?? definition.file) : definition.file,
                    ),
                  }
                : {}),
              ...(definition.range ? { range: definition.range } : {}),
              ...(definition.reason ? { reason: definition.reason } : {}),
            },
          }
        : {}),
    };
  });

  return {
    protocolVersion: PROTOCOL_VERSION,
    requestId: input.request.requestId,
    jsCheckApplied: input.jsCheckApplied,
    registration: input.registration,
    states: input.states,
    insertions: input.insertions,
    mounts: input.mounts.map((mount) => ({
      mountId: mount.mountId,
      fragmentId: mount.fragmentId,
      insertionId: mount.insertionId,
      file: mount.file,
      text: mount.text,
      startUtf16: mount.startUtf16,
      targetStartUtf16: mount.targetStartUtf16,
      targetLengthUtf16: mount.targetLengthUtf16,
      target: mount.target,
      segments: mount.segments,
    })),
    fragments,
    moduleKind: { before: input.moduleBefore, after: input.moduleAfter },
    suppressed: { count: input.suppressedDiagnostics, details: input.suppressedDetails },
    ...(input.components && input.components.length > 0 ? { components: input.components } : {}),
    ...(input.eventResults && input.eventResults.length > 0 ? { eventResults: input.eventResults } : {}),
  };
}

/** A file identity in the same space as `files[].fileId` / `scriptFileId`. */
export function fileIdOf(request: AnalyzeRequest, file: string): string {
  const relative = path.relative(path.resolve(request.project.projectRoot), path.resolve(file));
  return relative === "" ? "." : relative.split(path.sep).join("/");
}

export interface PreparedView {
  ok: true;
  request: AnalyzeRequest;
  scriptPath: string;
  scriptText: string;
  files: Map<string, string>;
  compilerOptions: ts.CompilerOptions;
  readPolicy: ReturnType<typeof defaultReadPolicy>;
  moduleBefore: "script" | "module";
  build: LayoutBuild;
  registration: AnalyzeResponse["registration"];
  pragmaNoCheck: boolean;
  baselineFingerprint: Fingerprint;
  baselineDiagnostics: Array<{ code: number; start: number; length: number }>;
  rootDeclarationSpans: Array<{ name: string; kind: "data" | "properties"; start: number; end: number }>;
}

export interface UnavailableView {
  ok: false;
  reason: string;
  registration: AnalyzeResponse["registration"];
}

/** Validate the request, build the derived view, but do not run queries. */
export function buildView(request: AnalyzeRequest, config: AnalysisConfig): PreparedView | UnavailableView {
  // Validate before touching any optional field: a malformed request must not
  // surface as a TypeError (the server maps RequestError to -32602).
  validateRequest(request);
  const registration: AnalyzeResponse["registration"] = {
    kind: request.view.owner.kind,
    count: 0,
    topLevel: null,
  };

  const projectRoot = request.project.projectRoot;
  const files = new Map<string, string>();
  for (const file of request.files) files.set(path.resolve(projectRoot, file.path), file.text);
  const scriptPath = path.resolve(projectRoot, request.scriptFileId);
  const scriptText = files.get(scriptPath);
  if (scriptText === undefined) {
    return {
      ok: false,
      reason: `scriptFileId ${request.scriptFileId} is not part of files[]; no disk fallback`,
      registration,
    };
  }

  const compilerOptions = toCompilerOptions(request.project.compilerOptions);
  const readPolicy = defaultReadPolicy(config.backendDir);
  try {
    assertEnvironment(config.backendDir, compilerOptions);
  } catch (error) {
    const reason = error instanceof EnvironmentError ? error.message : String(error);
    return { ok: false, reason: `type environment is incomplete: ${reason}`, registration };
  }

  const baseline = new VirtualProject({
    rootDir: config.backendDir,
    compilerOptions,
    files: [...files].map(([filePath, text]) => ({ path: filePath, text })),
    readPolicy,
  });
  const baselineSource = baseline.program().getSourceFile(scriptPath);
  if (!baselineSource) {
    return { ok: false, reason: "script is not part of the program", registration };
  }
  const moduleBefore = moduleKind(baselineSource);

  const sourceFile = parseSource(scriptText, scriptPath);
  const scan = findRegistrations(sourceFile);
  const wantedKind = registration.kind;
  let registrations = scan.registrations.filter((candidate) => candidate.kind === wantedKind);
  // A page route may be registered with `Component` instead of `Page` (a
  // supported Mini Program pattern). Prefer `Page` when both exist, but do not
  // reject a page solely because its module uses the component registrar.
  if (registrations.length === 0 && wantedKind === "page") {
    registrations = scan.registrations.filter((candidate) => candidate.kind === "component");
  }
  const found = registrations[0];
  registration.kind = found ? found.kind : wantedKind;
  registration.count = registrations.length;
  registration.topLevel = found ? found.topLevel : null;
  const propertyScan = scanDeclaredProperties(found?.options);
  const dataMember = found ? memberMap(found.options).get("data") : undefined;
  // Distinguish "no data member" (empty root) from "data exists but is not a
  // property assignment" (cannot bind a root at all).
  const dataRootKind: DataRootKind = !found
    ? "absent"
    : !dataMember
      ? "absent"
      : !ts.isPropertyAssignment(dataMember)
        ? "unsupported"
        : classifyDataRoot(
            baseline,
            scriptPath,
            dataMember.initializer.getStart(sourceFile),
            isSyntacticTypeReference(sourceFile, dataMember.initializer.getStart(sourceFile)),
          );

  const isJavaScript = /\.(js|jsx)$/.test(scriptPath);
  const checkJs = request.project.compilerOptions.checkJs ?? false;
  const wantPragma = request.policy.jsCheck === "pragma" && !checkJs && isJavaScript;
  const pragma = scanPragmas(scriptText, wantPragma);

  const context: LayoutContext = {
    rootKind: registration.kind,
    scriptPath,
    scriptText,
    sourceFile,
    scan,
    ...(found ? { registration: found } : {}),
    registrationCount: registrations.length,
    dataRootKind,
    propertyScan,
    pragma,
    wantPragma,
    fragments: request.fragments,
    jsCheckPolicy: request.policy.jsCheck,
    checkJs,
    isJavaScript,
  };

  const build =
    (config.layoutId ?? "in-place-shadow") === "in-place-shadow"
      ? buildInPlaceShadow(context)
      : buildSymbolBridge(context);
  stampInsertions(build, request, scriptPath);

  const baselineFingerprint = thisFingerprint(baseline, scriptPath, scriptText);
  const baselineDiagnostics = semanticDiagnostics(baseline, scriptPath).map((diagnostic) => ({
    code: diagnostic.code,
    start: diagnostic.start,
    length: diagnostic.length,
  }));
  const rootDeclarationSpans = declarationSpans(found?.options).map((span) => {
    const start = byteOffset(scriptText, span.node.getStart(sourceFile));
    const end = byteOffset(scriptText, span.node.getEnd());
    return { name: span.name, kind: span.kind, start, end };
  });

  return {
    ok: true,
    request,
    scriptPath,
    scriptText,
    files,
    compilerOptions,
    readPolicy,
    moduleBefore,
    build,
    registration,
    pragmaNoCheck: pragma.noCheck,
    baselineFingerprint,
    baselineDiagnostics,
    rootDeclarationSpans,
  };
}

export interface AnalysisResult {
  ok: true;
  request: AnalyzeRequest;
  response: AnalyzeResponse;
  build: LayoutBuild;
  jsCheckApplied: "inherit" | "pragma" | "off";
  moduleBefore: "script" | "module" | "unknown";
  moduleAfter: "script" | "module" | "unknown";
  registration: AnalyzeResponse["registration"];
  states: CapabilityStates;
  insertions: AnalyzeResponse["insertions"];
  mounts: AnalyzeResponse["mounts"];
  hovers: AnalysisQuery[];
  diagnostics: DiagnosticObservation[];
  definitions: AnalysisDefinition[];
  /** Event handler resolutions (protocol v2). */
  eventResults: AnalyzeResponse["eventResults"];
  suppressedDiagnostics: number;
  suppressedDetails: Array<{ code: number; message: string }>;
  probeProblems: string[];
  rootDeclarationSpans: Array<{ name: string; kind: "data" | "properties"; start: number; end: number }>;
  perturbsUserTypes: boolean;
  userScriptDiagnosticsIntroduced: Array<{ code: number; message: string; range?: SourceByteRange }>;
  notes: string[];
}

export interface UnavailableAnalysis {
  ok: false;
  reason: string;
  request: AnalyzeRequest;
  registration: AnalyzeResponse["registration"];
  response: AnalyzeResponse;
}

/**
 * Analyze a C1 request.
 *
 * Request-level problems (bad ids, bad options, unsupported protocol) throw
 * `RequestError`; runtime limitations (missing environment, missing snapshot
 * file) produce an `Unavailable` response instead of fabricating results.
 */
export function analyze(request: AnalyzeRequest, config: AnalysisConfig): AnalysisResult | UnavailableAnalysis {
  const view = buildView(request, config);
  if (!view.ok) {
    const states: CapabilityStates = {
      hover: { status: "unavailable", reason: view.reason },
      diagnostics: { status: "unavailable", reason: view.reason },
      definition: { status: "unavailable", reason: view.reason },
    };
    return {
      ok: false,
      reason: view.reason,
      request,
      registration: view.registration,
      response: unavailabilityResponse(request, view.registration, states),
    };
  }

  const {
    scriptPath,
    scriptText,
    files,
    compilerOptions,
    readPolicy,
    moduleBefore,
    build,
    registration,
    baselineFingerprint,
    baselineDiagnostics,
    rootDeclarationSpans,
  } = view;

  // A template without mustaches can still have component bindings (literal or
  // boolean attributes) and event handlers; only a request with none of the
  // three can skip the whole pipeline.
  if (
    build.fragments.length === 0 &&
    (request.components?.length ?? 0) === 0 &&
    (request.events?.length ?? 0) === 0
  ) {
    const states = finishedStates({
      build,
      policy: request.policy.jsCheck,
      probeProblems: [],
      introduced: [],
      noCheck: view.pragmaNoCheck,
      droppedDefinitions: 0,
      unmappedDiagnostics: 0,
      missing: 0,
      unexpected: 0,
    });
    const insertions = evidenceInsertions(build, request);
    const response = toAnalyzeResponse({
      request,
      jsCheckApplied: build.jsCheckApplied,
      registration,
      states,
      insertions,
      mounts: [],
      hovers: [],
      diagnostics: [],
      definitions: [],
      moduleBefore,
      moduleAfter: build.moduleAfter,
      suppressedDiagnostics: 0,
      suppressedDetails: [],
    });
    return {
      ok: true,
      request,
      response,
      build,
      jsCheckApplied: build.jsCheckApplied,
      moduleBefore,
      moduleAfter: build.moduleAfter,
      registration,
      states,
      insertions,
      mounts: [],
      hovers: [],
      diagnostics: [],
      definitions: [],
      eventResults: [],
      suppressedDiagnostics: 0,
      suppressedDetails: [],
      probeProblems: [],
      rootDeclarationSpans,
      perturbsUserTypes: false,
      userScriptDiagnosticsIntroduced: [],
      notes: build.notes,
    };
  }

  const project = new VirtualProject({
    rootDir: config.backendDir,
    compilerOptions,
    files: [...files].map(([filePath, text]) => ({ path: filePath, text })),
    readPolicy,
  });
  for (const file of build.files) project.set(file.path, file.text);

  const diagnosticsOff = build.jsCheckApplied === "off";
  const probeProblems = probeHealth(project, build, moduleBefore, !diagnosticsOff);
  const hovers = collectHovers(project, build);
  const { mapped, suppressed, suppressedDetails, unmapped } = diagnosticsOff
    ? { mapped: [], suppressed: 0, suppressedDetails: [], unmapped: 0 }
    : mapDiagnostics(project, build);
  const definitions = collectDefinitions(project, build, scriptPath, scriptText);
  const introduced = diagnosticsOff
    ? []
    : introducedDiagnostics(project, build, scriptPath, scriptText, baselineDiagnostics);
  const components = diagnosticsOff
    ? { results: [], skipped: 0, skippedDetails: [] }
    : collectComponentDiagnostics({ request, build, project });
  // Event resolution is AST-only: it does not depend on the derived view or on
  // the JavaScript check policy, and it must not disturb fragment results.
  const events = collectEventResults(request, view);
  const shadowFingerprint = thisFingerprintAtOffsets(project, scriptPath, build.edits, baselineFingerprint.offsets);
  const perturbsUserTypes =
    shadowFingerprint !== undefined && !sameFingerprint(baselineFingerprint.types, shadowFingerprint.types);
  let states = finishedStates({
    build,
    policy: request.policy.jsCheck,
    probeProblems,
    introduced,
    noCheck: view.pragmaNoCheck,
    droppedDefinitions: definitions.dropped,
    unmappedDiagnostics: unmapped,
    missing: 0,
    unexpected: 0,
  });
  if (components.skipped > 0) {
    // Component bindings that were not checked are a coverage gap, not a clean
    // check: keep Disabled/Unsupported and make anything else Partial. Up to
    // three `tag.attribute` pairs make the reason actionable.
    const detail =
      components.skippedDetails.length > 0 ? ` (${components.skippedDetails.join(", ")})` : "";
    states = {
      ...states,
      diagnostics: coverPartial(
        states.diagnostics,
        `some component bindings could not be resolved and were not checked${detail}`,
      ),
    };
  }
  if (events.skipped > 0) {
    states = {
      ...states,
      diagnostics: coverPartial(states.diagnostics, eventCoverageReason(events.skippedDetails)),
    };
  }

  const insertions = evidenceInsertions(build, request);
  const mounts = evidenceMounts(build, request);
  const response = toAnalyzeResponse({
    request,
    jsCheckApplied: build.jsCheckApplied,
    registration,
    states,
    insertions,
    mounts,
    hovers,
    diagnostics: mapped,
    definitions: definitions.observations,
    moduleBefore,
    moduleAfter: build.moduleAfter,
    suppressedDiagnostics: suppressed,
    suppressedDetails,
    ...(components.results.length > 0 ? { components: components.results } : {}),
    ...(events.results.length > 0 ? { eventResults: events.results } : {}),
  });

  return {
    ok: true,
    request,
    response,
    build,
    jsCheckApplied: build.jsCheckApplied,
    moduleBefore,
    moduleAfter: build.moduleAfter,
    registration,
    states,
    insertions,
    mounts,
    hovers,
    diagnostics: mapped,
    definitions: definitions.observations,
    eventResults: events.results,
    suppressedDiagnostics: suppressed,
    suppressedDetails,
    probeProblems,
    rootDeclarationSpans,
    perturbsUserTypes,
    userScriptDiagnosticsIntroduced: introduced,
    notes: build.notes,
  };
}

function unavailabilityResponse(
  request: AnalyzeRequest,
  registration: AnalyzeResponse["registration"],
  states: CapabilityStates,
): AnalyzeResponse {
  return {
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
  };
}

/**
 * Fill `viewRevision`/`backendFileVersion` from the request.
 *
 * `viewRevision` comes from `analysisStamp.viewRevision`; `backendFileVersion`
 * is the `sourceRevision` of the file the insertion was applied to (script
 * revision as fallback, never below 1).
 */
export function stampInsertions(build: LayoutBuild, request: AnalyzeRequest, scriptPath: string): void {
  const projectRoot = path.resolve(request.project.projectRoot);
  const revisionOf = (file: string): number => {
    const resolved = path.resolve(file);
    const match = request.files.find((entry) => path.resolve(projectRoot, entry.path) === resolved);
    if (match) return Math.max(1, match.sourceRevision);
    const script = request.files.find((entry) => path.resolve(projectRoot, entry.path) === path.resolve(scriptPath));
    return Math.max(1, script?.sourceRevision ?? 1);
  };
  for (const insertion of build.insertions) {
    insertion.viewRevision = request.analysisStamp.viewRevision;
    insertion.backendFileVersion = revisionOf(insertion.file);
  }
}

export function evidenceInsertions(build: LayoutBuild, request: AnalyzeRequest): AnalyzeResponse["insertions"] {
  return build.insertions.map((insertion) => ({
    insertionId: insertion.insertionId,
    file: fileIdOf(request, insertion.file),
    kind: insertion.kind,
    startUtf16: insertion.startUtf16,
    lengthUtf16: insertion.lengthUtf16,
    replaceLengthUtf16: insertion.replaceLengthUtf16,
    viewRevision: insertion.viewRevision,
    backendFileVersion: insertion.backendFileVersion,
  }));
}

export function evidenceMounts(build: LayoutBuild, request: AnalyzeRequest): AnalyzeResponse["mounts"] {
  return build.fragments.flatMap((fragment) =>
    fragment.mounts.map((mount) => ({
      fragmentId: fragment.fragmentId,
      mountId: mount.mountId,
      insertionId: mount.insertionId,
      file: fileIdOf(request, mount.file),
      startUtf16: mount.startUtf16,
      targetStartUtf16: mount.targetStartUtf16,
      targetLengthUtf16: mount.targetLengthUtf16,
      text: mount.text,
      target: mount.target,
      segments: mount.segments,
    })),
  );
}

/* ------------------------------------------------------------------ states */

interface StateInput {
  build: LayoutBuild;
  policy: "inherit" | "pragma" | "off";
  probeProblems: string[];
  introduced: Array<{ code: number; message: string }>;
  noCheck: boolean;
  droppedDefinitions: number;
  /** In-mount diagnostics that could not be attributed to any WXML source span. */
  unmappedDiagnostics: number;
  missing: number;
  unexpected: number;
}

export function finishedStates(input: StateInput): CapabilityStates {
  return computeStates(input, input.missing, input.unexpected);
}

/** Harness hook: apply expected-diagnostic mismatches on top of analysis states. */
export function applyMismatch(
  states: CapabilityStates,
  buildStatus: CapabilityStatus,
  missing: number,
  unexpected: number,
): CapabilityStates {
  if (missing === 0 && unexpected === 0) return states;
  const mismatch = `expected ${missing} diagnostic(s) were not observed and ${unexpected} were unexpected`;
  const diagnostics =
    buildStatus === "supported" && states.diagnostics.status === "supported"
      ? { status: "partial" as const, reason: mismatch }
      : { status: states.diagnostics.status, reason: `${states.diagnostics.reason ?? "check incomplete"}; ${mismatch}` };
  return { ...states, diagnostics };
}

function computeStates(input: StateInput, missing: number, unexpected: number): CapabilityStates {
  const reason = input.build.reason;
  const base = (status: CapabilityStatus): CapabilityState => ({ status, ...(reason ? { reason } : {}) });

  if (input.build.buildStatus === "unsupported") {
    return { hover: base("unsupported"), diagnostics: base("unsupported"), definition: base("unsupported") };
  }

  const problems = input.probeProblems;
  const rootBroken = problems.some((problem) => problem.includes("probe root"));
  let hover: CapabilityState =
    input.build.buildStatus === "partial"
      ? base("partial")
      : problems.length > 0
        ? { status: rootBroken ? "unavailable" : "partial", reason: problems.join("; ") }
        : { status: "supported" };
  let diagnostics: CapabilityState =
    input.build.buildStatus === "partial" ? base("partial") : { status: "supported" };
  let definition: CapabilityState =
    input.build.buildStatus === "partial" ? base("partial") : { status: "supported" };

  const checkOff = input.build.jsCheckApplied === "off" || input.noCheck;
  if (checkOff) {
    diagnostics = { status: "disabled", reason: checkOffReason(input.policy, input.noCheck) };
  } else if (input.introduced.length > 0) {
    diagnostics = {
      status: "partial",
      reason: `derived view reports ${input.introduced.length} script diagnostic(s) outside the fragments; they are suppressed, not published`,
    };
  }
  if (problems.length > 0) {
    const gate: CapabilityState = {
      status: rootBroken ? "unavailable" : "partial",
      reason: [reason, ...problems].filter(Boolean).join("; "),
    };
    if (!checkOff) diagnostics = gate;
    if (hover.status === "supported") hover = gate;
    definition = gate;
  }
  if ((input.build.unresolvedBindings ?? 0) + (input.build.unresolvedGuards ?? 0) > 0) {
    // A layout that reported unresolved bindings/guards must never claim
    // Supported. Layouts already downgrade their own `buildStatus`, so this only
    // catches an inconsistent report without duplicating the reason.
    if (input.build.buildStatus === "supported") {
      const unresolvedReason = "some template bindings or guards could not be resolved and were not checked";
      hover = mergeLocal(hover, unresolvedReason);
      diagnostics =
        diagnostics.status === "disabled"
          ? { status: "disabled", reason: [diagnostics.reason, unresolvedReason].filter(Boolean).join("; ") }
          : mergeLocal(diagnostics, unresolvedReason);
      definition = mergeLocal(definition, unresolvedReason);
    }
  }
  if (input.droppedDefinitions > 0) {
    definition = mergeLocal(
      definition,
      "a definition target could not be mapped back to the original source; it is not published",
    );
  }
  if (input.unmappedDiagnostics > 0) {
    const reason = `${input.unmappedDiagnostics} diagnostic(s) cannot be attributed to WXML source; they are suppressed`;
    diagnostics =
      diagnostics.status === "disabled"
        ? {
            status: "disabled",
            reason: [diagnostics.reason, reason].filter(Boolean).join("; "),
          }
        : mergeLocal(diagnostics, reason);
  }
  if (missing > 0 || unexpected > 0) {
    return applyMismatch({ hover, diagnostics, definition }, input.build.buildStatus, missing, unexpected);
  }
  return { hover, diagnostics, definition };
}

/** Downgrade `supported`/`partial` to `partial`; keep unsupported/unavailable. */
function mergeLocal(state: CapabilityState, reason: string): CapabilityState {
  if (state.status === "unsupported" || state.status === "unavailable") return state;
  const merged = [state.reason, reason].filter(Boolean).join("; ");
  return { status: "partial", reason: merged };
}

/**
 * Downgrade a diagnostics state for partial coverage.
 *
 * `disabled`/`unsupported`/`unavailable` already publish nothing, so they are
 * left alone; anything else becomes Partial with the coverage reason.
 */
function coverPartial(state: CapabilityState, reason: string): CapabilityState {
  if (state.status === "disabled" || state.status === "unsupported" || state.status === "unavailable") return state;
  return mergeLocal(state, reason);
}

/** Coverage reason for events that could not be resolved (never per-event noise). */
function eventCoverageReason(details: string[]): string {
  const suffix = details.length > 0 ? ` (${details.join(", ")})` : "";
  return `some event handlers could not be resolved and were not checked${suffix}`;
}

function checkOffReason(policy: "inherit" | "pragma" | "off", noCheck: boolean): string {
  if (policy === "off") return "template check policy is off";
  if (noCheck) return "`@ts-nocheck` in the source";
  return "template checking is disabled for this view";
}

/* --------------------------------------------------------------- queries */

/** Diagnostics of the derived script that the original script did not have. */
export function introducedDiagnostics(
  project: VirtualProject,
  build: LayoutBuild,
  scriptPath: string,
  originalScript: string,
  baseline: Array<{ code: number; start: number; length: number }>,
): Array<{ code: number; message: string; range?: SourceByteRange }> {
  const remaining = new Map<string, number>();
  for (const diagnostic of baseline) {
    const key = `${diagnostic.code}:${diagnostic.start}:${diagnostic.length}`;
    remaining.set(key, (remaining.get(key) ?? 0) + 1);
  }
  const mounts = build.fragments.flatMap((fragment) => fragment.mounts).filter((mount) => mount.file === scriptPath);
  const introduced: Array<{ code: number; message: string; range?: SourceByteRange }> = [];
  for (const diagnostic of semanticDiagnostics(project, scriptPath)) {
    const insideMount = mounts.some(
      (mount) => diagnostic.start >= mount.startUtf16 && diagnostic.start < mount.startUtf16 + mount.text.length,
    );
    if (insideMount) continue;
    const originalOffset = shadowToOriginalOffset(build.edits, diagnostic.start);
    const mapped =
      originalOffset === undefined ? undefined : byteRange(originalScript, originalOffset, diagnostic.length);
    const key =
      originalOffset === undefined
        ? `${diagnostic.code}:synthetic:${diagnostic.length}`
        : `${diagnostic.code}:${originalOffset}:${diagnostic.length}`;
    const available = remaining.get(key) ?? 0;
    if (available > 0) {
      remaining.set(key, available - 1);
      continue;
    }
    introduced.push({
      code: diagnostic.code,
      message: sanitizeMessage(diagnostic.message),
      ...(mapped ? { range: mapped } : {}),
    });
  }
  return introduced.slice(0, 8);
}

/** Bound the number of hover candidates returned for one expression. */
const MAX_HOVER_CANDIDATES = 32;
/** Bound language-service queries spent on candidates across one analysis. */
const MAX_TOTAL_HOVER_CANDIDATES = 4096;

export function collectHovers(project: VirtualProject, build: LayoutBuild): AnalysisQuery[] {
  let remaining = MAX_TOTAL_HOVER_CANDIDATES;
  return build.fragments.map((fragment) => {
    const mount = fragment.mounts[0];
    if (!mount) return { fragmentId: fragment.fragmentId, query: fragment.expr, observation: null };
    const observation = hoverAt(project, mount.file, mount.targetStartUtf16);
    const candidates: AnalysisHoverCandidate[] = [];
    const seen = new Set<string>();
    for (const target of mount.hoverTargets ?? []) {
      if (remaining === 0) break;
      const key = `${target.source.start}:${target.source.end}`;
      if (seen.has(key)) continue;
      const candidate = hoverAt(project, mount.file, target.startUtf16);
      if (!candidate) continue;
      remaining -= 1;
      seen.add(key);
      candidates.push({
        range: target.source,
        observation: withProvenance(project, build, mount.file, target.startUtf16, candidate),
      });
      if (candidates.length >= MAX_HOVER_CANDIDATES) break;
    }
    if (!observation) {
      return {
        fragmentId: fragment.fragmentId,
        query: fragment.expr,
        observation: null,
        ...(candidates.length > 0 ? { candidates } : {}),
      };
    }
    return {
      fragmentId: fragment.fragmentId,
      query: fragment.expr,
      observation: withProvenance(project, build, mount.file, mount.targetStartUtf16, observation),
      ...(candidates.length > 0 ? { candidates } : {}),
    };
  });
}

/**
 * Documentation that resolves to a *generated* declaration is copied text, not
 * the original field documentation; the C1 contract must report that honestly.
 */
function withProvenance(
  project: VirtualProject,
  build: LayoutBuild,
  file: string,
  offset: number,
  observation: HoverObservation,
): HoverObservation {
  const definitions = definitionsAt(project, file, offset);
  const generated = definitions.some((definition) =>
    build.generatedDeclarations.some(
      (declaration) =>
        declaration.file === definition.file &&
        definition.start >= declaration.start &&
        definition.start < declaration.start + declaration.length,
    ),
  );
  if (!generated) return observation;
  return {
    ...observation,
    documentationSource: observation.documentation ? "copied" : "none",
  };
}

export function mapDiagnostics(
  project: VirtualProject,
  build: LayoutBuild,
): {
  mapped: DiagnosticObservation[];
  suppressed: number;
  suppressedDetails: Array<{ code: number; message: string }>;
  /** Diagnostics inside a mount that had no publishable WXML span. */
  unmapped: number;
} {
  const mounts = build.fragments.flatMap((fragment) =>
    fragment.mounts.map((mount) => ({ fragmentId: fragment.fragmentId, mount })),
  );
  const files = [...new Set(mounts.map((entry) => entry.mount.file))];
  const mapped: DiagnosticObservation[] = [];
  const suppressedDetails: Array<{ code: number; message: string }> = [];
  let suppressed = 0;
  let unmapped = 0;
  const seen = new Set<string>();
  for (const file of files) {
    for (const diagnostic of semanticDiagnostics(project, file)) {
      const hit = mounts.find(
        (candidate) =>
          candidate.mount.file === file &&
          diagnostic.start >= candidate.mount.startUtf16 &&
          diagnostic.start < candidate.mount.startUtf16 + candidate.mount.text.length,
      );
      if (!hit) {
        suppressed += 1;
        const message = sanitizeMessage(diagnostic.message);
        if (suppressedDetails.length < 5 && !suppressedDetails.some((entry) => entry.message === message)) {
          suppressedDetails.push({ code: diagnostic.code, message });
        }
        continue;
      }
      const span = mapSpanToSource(hit.mount, diagnostic.start, diagnostic.length);
      if (span.unattributable || !span.range) {
        suppressed += 1;
        unmapped += 1;
        continue;
      }
      const key = `${diagnostic.code}:${span.range.start}-${span.range.end}`;
      if (seen.has(key)) continue;
      seen.add(key);
      mapped.push({
        fragmentId: hit.fragmentId,
        range: span.range,
        span: { file, start: diagnostic.start, length: diagnostic.length },
        code: diagnostic.code,
        message: sanitizeMessage(diagnostic.message),
        rawMessage: diagnostic.message,
        trimmedSyntheticPrefix: span.trimmedSyntheticPrefix,
      });
    }
  }
  return { mapped, suppressed, suppressedDetails, unmapped };
}

/* --------------------------------------------------------------- definitions */

/**
 * Classify one TypeScript definition target.
 *
 * Targets in the derived query file itself are dropped: publishing a location
 * inside a generated file is never allowed (C1 §4.4).
 */
export function classifyDefinitionTarget(
  target: { file: string; start: number; length: number },
  build: LayoutBuild,
  scriptPath: string,
  scriptText: string,
): DefinitionObservation {
  const generated = build.generatedDeclarations.find(
    (declaration) =>
      declaration.file === target.file &&
      target.start >= declaration.start &&
      target.start < declaration.start + declaration.length,
  );
  if (generated) {
    if (generated.source) {
      return {
        kind: "generated",
        file: target.file,
        start: target.start,
        length: target.length,
        range: { start: generated.source.start, end: generated.source.end },
        sourceFile: generated.source.file,
      };
    }
    return {
      kind: "dropped",
      file: target.file,
      start: target.start,
      length: target.length,
      reason: "generated declaration has no source mapping",
    };
  }
  if (target.file === path.resolve(scriptPath)) {
    const originalOffset = shadowToOriginalOffset(build.edits, target.start);
    const range = originalOffset === undefined ? undefined : byteRange(scriptText, originalOffset, target.length);
    return range
      ? { kind: "script", file: target.file, start: target.start, length: target.length, range }
      : {
          kind: "dropped",
          file: target.file,
          start: target.start,
          length: target.length,
          reason: "target is inside an inserted helper",
        };
  }
  if (target.file === path.resolve(build.queryFile) || build.files.some((file) => path.resolve(file.path) === target.file)) {
    return {
      kind: "dropped",
      file: target.file,
      start: target.start,
      length: target.length,
      reason: "target is a derived file; a derived path is never published",
    };
  }
  return { kind: "external", file: target.file, start: target.start, length: target.length };
}

export function collectDefinitions(
  project: VirtualProject,
  build: LayoutBuild,
  scriptPath: string,
  scriptText: string,
): { observations: AnalysisDefinition[]; dropped: number } {
  const observations: AnalysisDefinition[] = [];
  let dropped = 0;
  for (const fragment of build.fragments) {
    const mount = fragment.mounts[0];
    if (!mount) continue;
    const raw = definitionsAt(project, mount.file, mount.targetStartUtf16);
    const first = raw[0];
    if (!first) {
      observations.push({ fragmentId: fragment.fragmentId, query: fragment.expr, observation: null });
      continue;
    }
    const observation = { ...classifyDefinitionTarget(first, build, scriptPath, scriptText), fragmentId: fragment.fragmentId };
    if (observation.kind === "external") {
      const externalText = project.text(first.file);
      if (externalText !== undefined) {
        observation.range = byteRange(externalText, first.start, first.length);
      } else {
        observations.push({
          fragmentId: fragment.fragmentId,
          query: fragment.expr,
          observation: { ...observation, kind: "dropped", reason: "target file is outside the snapshot" },
        });
        dropped += 1;
        continue;
      }
    }
    if (observation.kind === "dropped") dropped += 1;
    observations.push({ fragmentId: fragment.fragmentId, query: fragment.expr, observation });
  }
  return { observations, dropped };
}

export function probeHealth(
  project: VirtualProject,
  build: LayoutBuild,
  moduleBefore: "script" | "module",
  checkDiagnostics: boolean,
): string[] {
  const anchors = build.probeAnchors;
  if (!anchors) return [];
  const mounts = build.fragments.flatMap((fragment) => fragment.mounts);
  const health = checkProbeHealth({
    project,
    scriptPath: anchors.file,
    helperOffsets: anchors.helperOffsets,
    ...(anchors.rootOffset !== undefined ? { rootOffset: anchors.rootOffset } : {}),
    insertions: build.insertions,
    mounts,
    moduleBefore,
    moduleAfter: build.moduleAfter,
    checkDiagnostics,
  });
  return health.problems;
}

/* ------------------------------------------------------------------ helpers */

export interface Fingerprint {
  offsets: number[];
  types: string[];
}

/** `this.<member>` member types of the original script: the non-interference check. */
export function thisFingerprint(project: VirtualProject, scriptPath: string, text: string): Fingerprint {
  const sourceFile = parseSource(text, scriptPath);
  const offsets: number[] = [];
  const visit = (node: ts.Node): void => {
    if (ts.isPropertyAccessExpression(node) && node.expression.kind === ts.SyntaxKind.ThisKeyword) {
      offsets.push(node.name.getStart(sourceFile));
    }
    node.forEachChild(visit);
  };
  visit(sourceFile);
  const types = offsets.map((offset) => hoverAt(project, scriptPath, offset)?.typeText ?? "<none>");
  return { offsets, types };
}

export function thisFingerprintAtOffsets(
  project: VirtualProject,
  scriptPath: string,
  edits: TextEdit[],
  offsets: number[],
): Fingerprint {
  const shadowOffsets = offsets.map((offset) => originalToShadowOffset(edits, offset));
  const types = shadowOffsets.map((offset) => hoverAt(project, scriptPath, offset)?.typeText ?? "<none>");
  return { offsets: shadowOffsets, types };
}

export function sameFingerprint(left: string[], right: string[]): boolean {
  if (left.length !== right.length) return false;
  return left.every((value, index) => value === right[index]);
}

export function shadowToOriginalOffset(edits: TextEdit[], shadowOffset: number): number | undefined {
  let shift = 0;
  for (const edit of edits) {
    if (shadowOffset >= edit.shadowStart && shadowOffset < edit.shadowStart + edit.text.length) return undefined;
    if (shadowOffset >= edit.shadowStart + edit.text.length) shift += edit.text.length - edit.length;
    else break;
  }
  return shadowOffset - shift;
}

export function originalToShadowOffset(edits: TextEdit[], originalOffset: number): number {
  let shift = 0;
  for (const edit of edits) {
    if (edit.at <= originalOffset) shift += edit.text.length - edit.length;
    else break;
  }
  return originalOffset + shift;
}

export function isSyntacticTypeReference(sourceFile: ts.SourceFile, offset: number | undefined): boolean {
  if (offset === undefined) return false;
  let identifier: string | undefined;
  const visit = (node: ts.Node): void => {
    if (node.getStart(sourceFile) === offset && ts.isIdentifier(node)) identifier = node.text;
    if (identifier === undefined) node.forEachChild(visit);
  };
  visit(sourceFile);
  if (identifier === undefined) return false;
  return sourceFile.statements.some(
    (statement) =>
      (ts.isInterfaceDeclaration(statement) || ts.isTypeAliasDeclaration(statement)) && statement.name.text === identifier,
  );
}

export function byteRange(text: string, start: number, length: number): SourceByteRange | undefined {
  if (start < 0 || length < 0 || start + length > text.length) return undefined;
  const startByte = byteOffset(text, start);
  const endByte = byteOffset(text, start + length);
  return startByte === undefined || endByte === undefined ? undefined : { start: startByte, end: endByte };
}

export function byteOffset(text: string, utf16Index: number): number {
  return Buffer.byteLength(text.slice(0, utf16Index), "utf8");
}

export { locateExpression };
export type { RequestCompilerOptions };
