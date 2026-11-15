/** Shared vocabulary for the P4 type-bridge experiment (C1 candidate contract). */

/** Which bridge candidate produced a result. */
export type LayoutId = "in-place-shadow" | "symbol-bridge";

/** Capability status, mirroring the six C0 states. */
export type CapabilityStatus = "supported" | "partial" | "unsupported" | "disabled" | "unavailable" | "unknown";

export interface CapabilityState {
  status: CapabilityStatus;
  reason?: string;
}

/** Status is reported per capability: hover, diagnostics and definition differ. */
export interface CapabilityStates {
  hover: CapabilityState;
  diagnostics: CapabilityState;
  definition: CapabilityState;
}

/** Type quality mirrors the C0 `TypeQuality` contract. */
export type TypeQuality = "known" | "any" | "unknown" | "missing";

/** Where the hover documentation text came from. */
export type DocumentationSource = "quickinfo" | "symbol" | "copied" | "none";

/** A fragment before it is mounted into a generated file. */
export interface FragmentDraft {
  fragmentId: string;
  /** Source expression text. */
  expr: string;
  /** Mounted text with every free root rewritten. */
  mountedText: string;
  target: string;
  /** Offset of the target identifier inside `mountedText`. */
  targetOffset: number;
  /** Segments of `mountedText`; source ranges are absolute WXML bytes. */
  segments: Segment[];
  /** Byte range of `expr` in the original WXML text. */
  source: SourceByteRange;
  /** Template scope bindings from the request. */
  locals: string[];
  /** First free root identifier (`user.name` -> `user`). */
  root: string;
  /** True when every free root is a template local (no root prefix). */
  localRoot: boolean;
}

/** A half-open byte range into an original text (UTF-8). */
export interface SourceByteRange {
  start: number;
  end: number;
}

/** One piece of a mounted fragment. */
export interface Segment {
  /** Offset (UTF-16) inside the derived file. */
  startUtf16: number;
  /** Length in UTF-16 code units. */
  lengthUtf16: number;
  /** Original WXML byte range for verbatim copies; absent for synthetic text. */
  source?: SourceByteRange;
}

/** A single text insertion into a file (never a rewrite). */
export interface InsertionRecord {
  insertionId: string;
  file: string;
  /** Offset of the inserted/replaced text inside the derived file (UTF-16). */
  startUtf16: number;
  /** Inserted length in UTF-16 code units. */
  lengthUtf16: number;
  /** Original characters removed at `start` (0 for pure insertions). */
  replaceLengthUtf16: number;
  /** View revision supplied by the request (P5 passes it through for P7). */
  viewRevision: number;
  /** Version of the file the insertion was applied to (from `files[].sourceRevision`). */
  backendFileVersion: number;
  kind: "probe" | "pragma" | "anchor" | "capture" | "check-file" | "props-type" | "edit";
}

/** One mounted location of an expression. One fragment can have several. */
export interface MountRecord {
  mountId: string;
  fragmentId: string;
  insertionId: string;
  file: string;
  /** Full mounted text, for example `__wxml_root.user.name`. */
  text: string;
  /** Offset of `text` inside `file`. */
  startUtf16: number;
  /** Offset of the WXML target identifier inside `file`. */
  targetStartUtf16: number;
  /** Length of the WXML target identifier in UTF-16 code units. */
  targetLengthUtf16: number;
  target: string;
  segments: Segment[];
}

/** A WXML expression plus every place it was mounted. */
export interface FragmentRecord {
  /** Request fragment id (stable identity, echoed in the response). */
  fragmentId: string;
  /** Expression text as written in WXML. */
  expr: string;
  root: string;
  target: string;
  /** Template scope bindings from the request. */
  locals: string[];
  /** True when every free root is a template local (no root prefix). */
  localRoot: boolean;
  mounts: MountRecord[];
}

/** Generated type member that stands for an original script declaration. */
export interface GeneratedDeclaration {
  file: string;
  start: number;
  length: number;
  /** Original script byte range this generated member stands for. */
  source?: { file: string; start: number; end: number };
}

/** A text insertion used to derive a shadow file from the original script. */
export interface TextEdit {
  /** Offset in the original script. */
  at: number;
  /** Length of the replaced range (0 for pure insertions). */
  length: number;
  /** Inserted text. */
  text: string;
  /** Offset of the inserted text inside the derived file. */
  shadowStart: number;
}

export interface HoverObservation {
  /** Full quick-info text, for example `(property) title: string`. */
  typeText: string;
  /** Type part only, for example `string`. */
  typeOnly: string;
  documentation: string;
  documentationSource: DocumentationSource;
  quality: TypeQuality;
}

export interface DiagnosticObservation {
  /** Fragment this diagnostic belongs to. */
  fragmentId?: string;
  /** Mapped WXML byte range, or `undefined` when the span could not be attributed. */
  range?: SourceByteRange;
  /** Raw UTF-16 span in the derived file (internal). */
  span: { file: string; start: number; length: number };
  /** TypeScript diagnostic code (internal, never user-visible). */
  code: number;
  /** Sanitized, user-visible message. */
  message: string;
  /** Raw TypeScript message (internal). */
  rawMessage: string;
  /** True when the published span lost its synthetic prefix while mapping. */
  trimmedSyntheticPrefix: boolean;
}

export type DefinitionKind = "script" | "generated" | "external" | "dropped";

export interface DefinitionObservation {
  kind: DefinitionKind;
  /** Request fragment id this definition belongs to. */
  fragmentId?: string;
  /** Derived/external file that TypeScript reported (internal, never published). */
  file?: string;
  /** UTF-16 span in that file (internal). */
  start?: number;
  length?: number;
  /** Byte range in the target source file (the original script, a model file or a `.d.ts`). */
  range?: SourceByteRange;
  /** For `generated` targets: the original file the generated member stands for. */
  sourceFile?: string;
  /** Reason a definition was dropped. */
  reason?: string;
}

export interface QuerySpec {
  /** WXML expression text, exactly as written between `{{` and `}}`. */
  expr: string;
  /** Identifier to hover and to map diagnostics onto. */
  target: string;
  /** Expected type text. */
  expectType?: string;
  /** Expected hover documentation. */
  expectDoc?: string;
  /** Expected documentation source. */
  expectDocSource?: DocumentationSource;
  /** Query `getDefinitionAtPosition` for this identifier. */
  definition?: boolean;
  /** Expected definition kind when queried. */
  expectDefinition?: DefinitionKind;
}

export interface ExpectedDiagnostic {
  expr: string;
  target: string;
}

/** How JavaScript checking is enabled for a scenario. */
export type JsCheckMode = "inherit" | "pragma" | "off";

import type { AnalyzeResponse, RequestCompilerOptions } from "./protocol.ts";

export interface RunOverrides {
  /** Replace the script text (simulates an unsaved editor buffer). */
  scriptText?: string;
  /** Replace/add project files by relative path. */
  files?: Record<string, string>;
  /** Mount every expression this many times (one-to-many mapping test). */
  mountRepeat?: number;
}

export interface ScenarioSpec {
  id: string;
  /** Directory under `samples/`. */
  dir: string;
  /** Registration kind the scenario exercises. */
  registration: "page" | "component";
  /** Script file relative to `dir`. */
  script: string;
  /** Project files that must be part of the snapshot (relative to `dir`). */
  extraFiles?: string[];
  allowJs: boolean;
  checkJs: boolean;
  /** Requested JavaScript checking policy. */
  jsCheck: JsCheckMode;
  /** WXML text used for byte mapping. */
  wxml: string;
  queries: QuerySpec[];
  expectedDiagnostics: ExpectedDiagnostic[];
  /** tsconfig (relative to `dir`) whose effective options the request carries. */
  tsconfig?: string;
  /** Scenario-level overrides on top of the default/tsconfig options. */
  compilerOptions?: Partial<RequestCompilerOptions>;
  note?: string;
}

export interface ScenarioRun {
  scenario: string;
  layout: LayoutId;
  states: CapabilityStates;
  /** Effective JavaScript checking policy after applying the scenario. */
  jsCheckApplied: "inherit" | "pragma" | "off";
  /** Module classification of the original script (`unknown` on environment failure). */
  moduleBefore: "script" | "module" | "unknown";
  /** Module classification of the derived shadow script. */
  moduleAfter: "script" | "module" | "unknown";
  /** True when the derived view changes the type of user `this.<member>` accesses. */
  perturbsUserTypes: boolean;
  /** Diagnostics the derived view added to the *user script* (not fragments). */
  userScriptDiagnosticsIntroduced: Array<{ code: number; message: string; range?: SourceByteRange }>;
  /** Diagnostics suppressed before publication, with sanitized messages. */
  suppressedDiagnostics: number;
  suppressedDetails: Array<{ code: number; message: string }>;
  /** Probe/helper health problems found by the post-build gate. */
  probeProblems: string[];
  /** Registration found in the script (count is required for `Partial`). */
  registration: { kind: "page" | "component"; count: number; topLevel: boolean | null };
  /** Request summary that produced this result. */
  request: {
    viewId: string;
    ownerKind: "page" | "component";
    jsCheckPolicy: "inherit" | "pragma" | "off";
    fileCount: number;
    totalBytes: number;
    fragmentCount: number;
    compilerOptionsDigest: string;
  };
  /** Insertions that produced the derived view (contract: per insertion). */
  insertions: Array<{
    insertionId: string;
    file: string;
    kind: InsertionRecord["kind"];
    startUtf16: number;
    lengthUtf16: number;
    replaceLengthUtf16: number;
    viewRevision: number;
    backendFileVersion: number;
  }>;
  /** Mounted fragments with their synthetic/source segments. */
  mounts: Array<{
    fragmentId: string;
    mountId: string;
    insertionId: string;
    file: string;
    startUtf16: number;
    targetStartUtf16: number;
    targetLengthUtf16: number;
    /** Mounted expression text (`__wxml_root.user.name`). */
    text: string;
    /** WXML target identifier. */
    target: string;
    segments: Segment[];
  }>;
  hovers: Array<{ fragmentId: string; query: string; observation: HoverObservation | null }>;
  diagnostics: DiagnosticObservation[];
  expectedDiagnostics: ExpectedDiagnostic[];
  /** True when every expected diagnostic mapped to the exact target bytes. */
  diagnosticsExact: boolean;
  unexpectedDiagnostics: DiagnosticObservation[];
  missingDiagnostics: ExpectedDiagnostic[];
  definitions: Array<{ fragmentId: string; query: string; observation: DefinitionObservation | null }>;
  /** Original script byte ranges of data/properties declarations. */
  rootDeclarationSpans: Array<{ name: string; kind: "data" | "properties"; start: number; end: number }>;
  /** Contract response assembled from this run (C1 §6.3). */
  response: AnalyzeResponse;
  notes?: string[];
  timings?: TimingObservation;
}

export interface TimingObservation {
  coldStartMs: number;
  hoverP50Ms: number;
  hoverP95Ms: number;
  diagnosticsMs: number;
  samples: number;
}

/** Build result of one candidate for one scenario. */
export interface LayoutBuild {
  layout: LayoutId;
  /** Overall ability to mount and check the scenario. */
  buildStatus: CapabilityStatus;
  reason?: string;
  files: Array<{ path: string; text: string }>;
  /** File that hosts the mounted fragments (the script for A, the check file for B). */
  queryFile: string;
  insertions: InsertionRecord[];
  fragments: FragmentRecord[];
  generatedDeclarations: GeneratedDeclaration[];
  /** Effective JavaScript checking policy used by the build. */
  jsCheckApplied: "inherit" | "pragma" | "off";
  notes: string[];
  /** Insertions applied to the original script (shadow -> original mapping). */
  edits: TextEdit[];
  /** Declaration site of the exported root anchor (candidate B only). */
  anchor?: { file: string; offset: number };
  /**
   * Offsets inside the derived script used by the probe-health gate.
   *
   * The gate uses these exact offsets; it never searches the derived text for a
   * name (a user symbol with the same name would otherwise be picked up).
   */
  probeAnchors?: {
    file: string;
    rootOffset?: number;
    helperOffsets: Array<{ name: string; offset: number; file?: string }>;
  };
  moduleAfter: "script" | "module";
}
