/**
 * Request/response model for the C1 type bridge (Rust -> Node).
 *
 * The harness builds a request exactly like the LSP coordinator would: an
 * explicit file snapshot, the effective compiler options resolved by the project
 * layer, the fragments to mount, and the policy. Node must not read project
 * sources from disk and must not invent compiler options.
 *
 * Protocol v2 adds the optional structured `bindings`/`guards` fragment inputs,
 * top-level `components`/`events` inputs, and reserves a `complete` method. Phase
 * 0 freezes only these shapes; `complete` is handled by a later worktree.
 */

import path from "node:path";
import ts from "typescript";
import { describeExpression, identifierOffset, isBindingIdentifier } from "./expression.ts";
import type {
  CapabilityState,
  CapabilityStates,
  InsertionRecord,
  JsCheckMode,
  MountRecord,
} from "./types.ts";

export const PROTOCOL_VERSION = 2;

export const DEFAULT_LIMITS: RequestLimits = {
  maxFiles: 512,
  maxTotalBytes: 8 * 1024 * 1024,
  maxFragments: 2048,
};

export interface RequestLimits {
  maxFiles: number;
  maxTotalBytes: number;
  maxFragments: number;
}

export type FileKind = "ts" | "js" | "jsx" | "tsx" | "d.ts";

export interface RequestFile {
  fileId: string;
  /** Project-relative path with `/` separators. */
  path: string;
  kind: FileKind;
  text: string;
  sourceRevision: number;
}

export interface RequestFragment {
  fragmentId: string;
  /** Expression text as written in WXML, for example `user.name`. */
  exprText: string;
  /**
   * Optional hover/diagnostic target token (a whole identifier in `exprText`).
   * Defaults to the first identifier of the expression.
   */
  target?: string;
  /**
   * Template scope bindings (for example `wx:for` item/index names). The root
   * identifier of the expression is mounted without a `__wxml_root.` prefix
   * when it appears here; expression text is never rewritten.
   */
  locals?: string[];
  /**
   * Structured `wx:for` bindings (protocol v2). Optional during the transition;
   * `locals` stays the authoritative name list until the template worktree
   * fills this in.
   */
  bindings?: RequestLocalBinding[];
  /** Enclosing `wx:if`/`wx:elif`/`wx:else` chains, outermost first (protocol v2). */
  guards?: RequestGuardChain[];
  /** Byte range of `exprText` in the WXML document. */
  wxmlRange: { start: number; end: number };
  fragmentRevision: number;
  /** Which registration owns the root. */
  root: "page" | "component";
}

/** Template role of one structured `wx:for` binding. */
export type LocalRole = "item" | "index";

/** One `wx:for` binding in scope at a fragment (protocol v2). */
export interface RequestLocalBinding {
  /** Binding name as written, for example `item` or `row`. */
  name: string;
  role: LocalRole;
  /** Exact `wx:for` expression text that produced the binding. */
  sourceExpr: string;
  /** Byte range of the `wx:for` expression in the WXML document. */
  sourceRange: { start: number; end: number };
}

/** Branch kind inside a `wx:if`/`wx:elif`/`wx:else` chain. */
export type GuardBranch = "if" | "elif" | "else";

/** One condition of a `wx:if`/`wx:elif`/`wx:else` chain (protocol v2). */
export interface RequestGuardCondition {
  branch: GuardBranch;
  /** Condition expression text without `{{` and `}}`. */
  exprText: string;
  /** Byte range of the condition expression in the WXML document. */
  range: { start: number; end: number };
}

/** Every condition of one conditional chain, in source order (protocol v2). */
export interface RequestGuardChain {
  conditions: RequestGuardCondition[];
  /** Selected branch index; `conditions.length` means `wx:else`. */
  selected: number;
}

/** How the value of a custom-component attribute is written. */
export type AttributeValueKind = "literal" | "mustache" | "mixed" | "boolean";

/** One attribute on a custom-component usage (protocol v2). */
export interface RequestComponentAttribute {
  name: string;
  valueKind: AttributeValueKind;
  /** Sole mustache expression text for `mustache` values. */
  exprText?: string;
  exprRange?: { start: number; end: number };
  /** Byte range of the whole attribute. */
  attributeRange: { start: number; end: number };
}

/** One custom-component usage in the template (protocol v2). */
export interface RequestComponentUsage {
  tag: string;
  /** Resolved local component script, when `usingComponents` points at one. */
  componentFileId?: string;
  attributes: RequestComponentAttribute[];
}

/** One statically bound event handler (protocol v2). */
export interface RequestEvent {
  eventId: string;
  /** Raw attribute name, for example `bind:tap`. */
  attribute: string;
  /** Normalized event name, for example `tap`. */
  event: string;
  /** Static handler name as written, trimmed. */
  handler: string;
  /** Byte range of the whole attribute. */
  attributeRange: { start: number; end: number };
  /** Byte range of the handler token only. */
  handlerRange: { start: number; end: number };
}

export type TargetName = "ES2020" | "ES2022" | "ESNext";
export type ModuleKindName = "ESNext" | "CommonJS" | "Preserve";
export type ModuleResolutionName = "bundler" | "node16" | "nodenext" | "node10";
export type JsxName = "preserve" | "react" | "react-jsx" | "react-jsxdev" | "react-native";
export type ModuleDetectionName = "auto" | "legacy" | "force";

/**
 * Effective compiler options resolved by the project layer (P3).
 *
 * This is intentionally close to `ts.CompilerOptions`: type-affecting switches
 * such as `strictNullChecks`, `noImplicitThis` and `noUncheckedIndexedAccess`
 * must travel verbatim, otherwise the derived view would silently check under
 * different rules than the project (false negatives and false positives).
 *
 * Node converts the named enums and copies every other field unchanged. Unknown
 * keys are rejected instead of being dropped silently.
 */
export interface RequestCompilerOptions {
  strict?: boolean;
  strictNullChecks?: boolean;
  strictFunctionTypes?: boolean;
  strictBindCallApply?: boolean;
  strictPropertyInitialization?: boolean;
  noImplicitAny?: boolean;
  noImplicitThis?: boolean;
  noUncheckedIndexedAccess?: boolean;
  exactOptionalPropertyTypes?: boolean;
  strictBuiltinIteratorReturn?: boolean;
  useUnknownInCatchVariables?: boolean;
  allowJs?: boolean;
  checkJs?: boolean;
  esModuleInterop?: boolean;
  allowSyntheticDefaultImports?: boolean;
  resolveJsonModule?: boolean;
  isolatedModules?: boolean;
  verbatimModuleSyntax?: boolean;
  forceConsistentCasingInFileNames?: boolean;
  skipLibCheck?: boolean;
  target?: TargetName;
  module?: ModuleKindName;
  moduleResolution?: ModuleResolutionName;
  jsx?: JsxName;
  moduleDetection?: ModuleDetectionName;
  lib?: string[];
  baseUrl?: string;
  paths?: Record<string, string[]>;
  types?: string[];
  typeRoots?: string[];
  rootDir?: string;
}

/** Keys that the request may carry; anything else is rejected. */
export const REQUEST_COMPILER_OPTION_KEYS: ReadonlySet<string> = new Set([
  "strict",
  "strictNullChecks",
  "strictFunctionTypes",
  "strictBindCallApply",
  "strictPropertyInitialization",
  "noImplicitAny",
  "noImplicitThis",
  "noUncheckedIndexedAccess",
  "exactOptionalPropertyTypes",
  "strictBuiltinIteratorReturn",
  "useUnknownInCatchVariables",
  "allowJs",
  "checkJs",
  "esModuleInterop",
  "allowSyntheticDefaultImports",
  "resolveJsonModule",
  "isolatedModules",
  "verbatimModuleSyntax",
  "forceConsistentCasingInFileNames",
  "skipLibCheck",
  "target",
  "module",
  "moduleResolution",
  "jsx",
  "moduleDetection",
  "lib",
  "baseUrl",
  "paths",
  "types",
  "typeRoots",
  "rootDir",
]);

export interface AnalyzeRequest {
  protocolVersion: number;
  requestId: string;
  analysisStamp: {
    snapshotId: number;
    projectRevision: number;
    viewRevision: number;
    backendEpoch: number;
  };
  view: {
    viewId: string;
    owner: { kind: "page" | "component"; fileId: string };
  };
  project: {
    /** Absolute project root known to the LSP coordinator. */
    projectRoot: string;
    compilerOptions: RequestCompilerOptions;
    typings: { package: string; version: string };
  };
  scriptFileId: string;
  files: RequestFile[];
  /** Fragments mounted by Node; this is an input, not the response list. */
  fragments: RequestFragment[];
  /**
   * Custom-component usages in the template (protocol v2). Optional: a request
   * without component data is still valid and produces no component results.
   */
  components?: RequestComponentUsage[];
  /**
   * Static event bindings in the template (protocol v2). Optional, like
   * `components`.
   */
  events?: RequestEvent[];
  policy: {
    jsCheck: JsCheckMode;
    /** C1.1 only supports the declaration overlay; `off` is rejected explicitly. */
    propertiesOverlay: "declaration";
  };
  /** Request-declared bounds; missing fields fall back to `DEFAULT_LIMITS`. */
  limits?: Partial<RequestLimits>;
}

/** Completion kinds that the Node backend answers (static completion is in Rust). */
export type CompletionRequestKind = "expressionMember" | "eventHandler";

/** `complete` request: AnalyzeRequest plus query point. */
export interface CompleteRequest extends AnalyzeRequest {
  /** UTF-8 byte offset in the WXML document. */
  offset: number;
  kind: CompletionRequestKind;
  /** Identifier/value prefix immediately before `offset` (debug + filtering). */
  prefix: string;
}

export interface CompleteItem {
  label: string;
  kind: "tag" | "attribute" | "value" | "property" | "method" | "event" | "keyword";
  detail?: string;
  documentation?: string;
  insertText: string;
  /** WXML byte range replaced by the edit; must be a precise token range. */
  replace: { start: number; end: number };
  sortText?: string;
}

export interface CompleteResponse {
  protocolVersion: number;
  requestId: string;
  state: CapabilityState;
  truncated: boolean;
  items: CompleteItem[];
}

export const COMPLETION_KINDS: ReadonlySet<CompletionRequestKind> = new Set<CompletionRequestKind>([
  "expressionMember",
  "eventHandler",
]);

/** Upper bound for the debug-only prefix (UTF-16 code units). */
export const MAX_COMPLETION_PREFIX = 256;

/**
 * Validate a `complete` request.
 *
 * Runs the shared `analyze` shape checks first, then the query point. The WXML
 * document is not part of `files[]`, so `offset` is not range-checked here; an
 * out-of-range point simply yields an empty list later.
 */
export function validateCompleteRequest(request: CompleteRequest): void {
  validateRequest(request as AnalyzeRequest);
  if (typeof request !== "object" || request === null) {
    throw new RequestError("bad-params", "request must be an object");
  }
  if (!Number.isInteger(request.offset) || request.offset < 0) {
    throw new RequestError("bad-params", "offset must be a non-negative integer");
  }
  if (typeof request.kind !== "string" || !COMPLETION_KINDS.has(request.kind)) {
    throw new RequestError("bad-params", `unknown completion kind: ${String(request.kind)}`);
  }
  if (typeof request.prefix !== "string") {
    throw new RequestError("bad-params", "prefix must be a string");
  }
  if (request.prefix.length > MAX_COMPLETION_PREFIX) {
    throw new RequestError("bad-limit", `prefix has ${request.prefix.length} characters, limit ${MAX_COMPLETION_PREFIX}`);
  }
}

export interface RegistrationInfo {
  kind: "page" | "component";
  count: number;
  topLevel: boolean | null;
}

/** Response envelope. The harness fills the query results in `ScenarioRun`. */
export interface AnalyzeResponse {
  protocolVersion: number;
  requestId: string;
  jsCheckApplied: JsCheckMode;
  registration: RegistrationInfo;
  states: CapabilityStates;
  insertions: InsertionRecord[];
  mounts: MountRecord[];
  /** Per-fragment results; assembled by the adapter from the query layer. */
  fragments: Array<{
    fragmentId: string;
    hover?: {
      typeText: string;
      typeOnly: string;
      quality: "known" | "any" | "unknown" | "missing";
      documentation: string;
      documentationSource: "quickinfo" | "symbol" | "copied" | "none";
    };
    diagnostics: Array<{
      code: number;
      /** WXML byte range (the only published coordinate). */
      range?: { start: number; end: number };
      message: string;
      attribution: "source" | "trimmed-synthetic" | "unattributable";
    }>;
    definition?: {
      kind: "script" | "generated" | "external" | "dropped";
      fileId?: string;
      /** Byte range in the target source file. */
      range?: { start: number; end: number };
      reason?: string;
    };
  }>;
  moduleKind: { before: "script" | "module" | "unknown"; after: "script" | "module" | "unknown" };
  suppressed: { count: number; details: Array<{ code: number; message: string }> };
}

export type RequestErrorCode =
  | "too-many-files"
  | "too-many-bytes"
  | "too-many-fragments"
  | "bad-limit"
  | "bad-option"
  | "bad-protocol"
  | "bad-params"
  | "bad-fragment"
  | "unsupported-option";

/**
 * Server-side ceiling. The request declares its own limits, but Node must use
 * `min(request, ceiling)` so a caller cannot widen the bound.
 */
export const SERVER_LIMITS: RequestLimits = {
  maxFiles: 1024,
  maxTotalBytes: 16 * 1024 * 1024,
  maxFragments: 4096,
};

const TARGET_NAMES = new Set<string>(["ES2020", "ES2022", "ESNext"]);
const MODULE_NAMES = new Set<string>(["ESNext", "CommonJS", "Preserve"]);
const RESOLUTION_NAMES = new Set<string>(["bundler", "node16", "nodenext", "node10"]);
const JSX_NAMES = new Set<string>(["preserve", "react", "react-jsx", "react-jsxdev", "react-native"]);
const MODULE_DETECTION_NAMES = new Set<string>(["auto", "legacy", "force"]);

export class RequestError extends Error {
  readonly code: RequestErrorCode;

  constructor(code: RequestErrorCode, message: string) {
    super(message);
    this.name = "RequestError";
    this.code = code;
  }
}

export function totalBytes(files: RequestFile[], fragments: RequestFragment[] = []): number {
  const fileBytes = files.reduce((sum, file) => sum + Buffer.byteLength(file.text, "utf8"), 0);
  const fragmentBytes = fragments.reduce(
    (sum, fragment) =>
      sum +
      (fragment && typeof fragment === "object" && typeof fragment.exprText === "string"
        ? Buffer.byteLength(fragment.exprText, "utf8")
        : 0),
    0,
  );
  return fileBytes + fragmentBytes;
}

/** Enforce input bounds before any parsing or language-service work. */
export function validateRequest(request: AnalyzeRequest): void {
  if (typeof request !== "object" || request === null) {
    throw new RequestError("bad-params", "request must be an object");
  }
  if (!request.project || typeof request.project !== "object" || !request.project.compilerOptions) {
    throw new RequestError("bad-params", "request.project.compilerOptions is required");
  }
  if (typeof request.project.projectRoot !== "string" || request.project.projectRoot.length === 0) {
    throw new RequestError("bad-params", "request.project.projectRoot is required");
  }
  if (!Array.isArray(request.files)) {
    throw new RequestError("bad-params", "request.files must be an array");
  }
  if (!Array.isArray(request.fragments)) {
    throw new RequestError("bad-params", "request.fragments must be an array");
  }
  if (!request.view || typeof request.view !== "object" || !request.view.owner) {
    throw new RequestError("bad-params", "request.view.owner is required");
  }
  if (typeof request.scriptFileId !== "string" || request.scriptFileId.length === 0) {
    throw new RequestError("bad-params", "request.scriptFileId is required");
  }
  if (request.policy && request.policy.jsCheck !== undefined) {
    const allowed = new Set(["inherit", "pragma", "off"]);
    if (!allowed.has(request.policy.jsCheck)) {
      throw new RequestError("bad-option", `unknown jsCheck policy: ${String(request.policy.jsCheck)}`);
    }
  } else {
    throw new RequestError("bad-params", "request.policy.jsCheck is required");
  }
  if (typeof request.requestId !== "string" || request.requestId.length === 0) {
    throw new RequestError("bad-params", "requestId is required");
  }
  validateStamp(request.analysisStamp);
  if (request.view.owner.kind !== "page" && request.view.owner.kind !== "component") {
    throw new RequestError("bad-params", `view.owner.kind must be page or component: ${String(request.view.owner.kind)}`);
  }
  if (typeof request.view.viewId !== "string" || request.view.viewId.length === 0) {
    throw new RequestError("bad-params", "view.viewId is required");
  }
  if (typeof request.view.owner.fileId !== "string" || request.view.owner.fileId.length === 0) {
    throw new RequestError("bad-params", "view.owner.fileId is required");
  }
  if (request.view.owner.fileId !== request.scriptFileId) {
    throw new RequestError("bad-params", "view.owner.fileId must equal scriptFileId");
  }
  validateFiles(request.files);
  if (request.project.typings?.package !== "miniprogram-api-typings") {
    throw new RequestError("bad-option", "project.typings.package must be miniprogram-api-typings");
  }
  if (request.project.typings.version !== "5.3.1") {
    throw new RequestError(
      "bad-option",
      `project.typings.version ${String(request.project.typings.version)} is not supported (expected 5.3.1)`,
    );
  }
  if (request.policy.propertiesOverlay !== "declaration") {
    throw new RequestError(
      "unsupported-option",
      `policy.propertiesOverlay ${String(request.policy.propertiesOverlay)} is not supported`,
    );
  }
  if (request.protocolVersion !== PROTOCOL_VERSION) {
    throw new RequestError(
      "bad-protocol",
      `protocolVersion ${request.protocolVersion} is not supported (expected ${PROTOCOL_VERSION})`,
    );
  }
  // Missing fields fall back to the defaults; present fields are validated one by
  // one, and the effective bound is the stricter of request and server ceiling.
  const requested = request.limits ?? DEFAULT_LIMITS;
  const effective = effectiveLimits(requested);
  for (const name of ["maxFiles", "maxTotalBytes", "maxFragments"] as const) {
    const value = requested[name];
    if (value !== undefined && (!Number.isFinite(value) || value <= 0 || !Number.isInteger(value))) {
      throw new RequestError("bad-limit", `limit ${name} must be a positive integer`);
    }
  }
  if (request.files.length > effective.maxFiles) {
    throw new RequestError("too-many-files", `request has ${request.files.length} files, limit ${effective.maxFiles}`);
  }
  const bytes = totalBytes(request.files, request.fragments);
  if (bytes > effective.maxTotalBytes) {
    throw new RequestError("too-many-bytes", `request has ${bytes} bytes, limit ${effective.maxTotalBytes}`);
  }
  if (request.fragments.length > effective.maxFragments) {
    throw new RequestError(
      "too-many-fragments",
      `request has ${request.fragments.length} fragments, limit ${effective.maxFragments}`,
    );
  }
  validateCompilerOptions(request.project.compilerOptions);
  validateFragments(request.fragments);
  validateComponents(request.components);
  validateEvents(request.events);
}

/** Validate the C1.1 fragment inputs (shape, ids, target, locals). */
export function validateFragments(fragments: RequestFragment[]): void {
  const ids = new Set<string>();
  for (const fragment of fragments) {
    if (typeof fragment !== "object" || fragment === null) {
      throw new RequestError("bad-fragment", "fragments[] entries must be objects");
    }
    const id = fragment.fragmentId;
    if (typeof id !== "string" || id.length === 0) {
      throw new RequestError("bad-fragment", "fragmentId must be a non-empty string");
    }
    if (ids.has(id)) {
      throw new RequestError("bad-fragment", `request repeats fragmentId ${id}; ids must be unique`);
    }
    ids.add(id);
    if (typeof fragment.exprText !== "string" || fragment.exprText.length === 0) {
      throw new RequestError("bad-fragment", `fragment ${id} has an empty exprText`);
    }
    if (fragment.root !== "page" && fragment.root !== "component") {
      throw new RequestError("bad-fragment", `fragment ${id} has an unknown root kind: ${String(fragment.root)}`);
    }
    const range = fragment.wxmlRange;
    if (
      !range ||
      !Number.isInteger(range.start) ||
      !Number.isInteger(range.end) ||
      range.start < 0 ||
      range.end < range.start
    ) {
      throw new RequestError("bad-fragment", `fragment ${id} has an invalid wxmlRange`);
    }
    if (range.end - range.start !== Buffer.byteLength(fragment.exprText, "utf8")) {
      throw new RequestError(
        "bad-fragment",
        `fragment ${id}: wxmlRange length ${range.end - range.start} does not match exprText bytes ` +
          `${Buffer.byteLength(fragment.exprText, "utf8")}`,
      );
    }
    if (fragment.target !== undefined) {
      if (typeof fragment.target !== "string" || fragment.target.length === 0) {
        throw new RequestError("bad-fragment", `fragment ${id} has an empty target`);
      }
      if (identifierOffset(fragment.exprText, fragment.target) < 0) {
        throw new RequestError(
          "bad-fragment",
          `fragment ${id}: target ${fragment.target} is not a whole identifier in ${describeExpression(fragment.exprText)}`,
        );
      }
    }
    if (fragment.locals !== undefined) {
      if (!Array.isArray(fragment.locals)) {
        throw new RequestError("bad-fragment", `fragment ${id}: locals must be an array`);
      }
      const seen = new Set<string>();
      for (const local of fragment.locals) {
        if (typeof local !== "string" || local.length === 0) {
          throw new RequestError("bad-fragment", `fragment ${id}: locals must be non-empty strings`);
        }
        if (!isBindingIdentifier(local)) {
          throw new RequestError("bad-fragment", `fragment ${id}: local ${local} is not a valid binding name`);
        }
        if (seen.has(local)) {
          throw new RequestError("bad-fragment", `fragment ${id}: duplicate local ${local}`);
        }
        seen.add(local);
      }
    }
    validateLocalBindings(id, fragment.bindings);
    validateGuardChains(id, fragment.guards);
  }
}

const LOCAL_ROLES = new Set(["item", "index"]);
const GUARD_BRANCHES = new Set(["if", "elif", "else"]);
const ATTRIBUTE_VALUE_KINDS = new Set(["literal", "mustache", "mixed", "boolean"]);

/** True for a non-negative byte range with `start <= end`. */
function isSourceRange(value: unknown): value is { start: number; end: number } {
  if (typeof value !== "object" || value === null) return false;
  const start = (value as { start?: unknown }).start;
  const end = (value as { end?: unknown }).end;
  return (
    typeof start === "number" &&
    typeof end === "number" &&
    Number.isInteger(start) &&
    Number.isInteger(end) &&
    start >= 0 &&
    end >= start
  );
}

/** Validate optional protocol v2 structured `wx:for` bindings. */
function validateLocalBindings(id: string, bindings: RequestLocalBinding[] | undefined): void {
  if (bindings === undefined) return;
  if (!Array.isArray(bindings)) {
    throw new RequestError("bad-fragment", `fragment ${id}: bindings must be an array`);
  }
  const seen = new Set<string>();
  for (const binding of bindings) {
    if (typeof binding !== "object" || binding === null) {
      throw new RequestError("bad-fragment", `fragment ${id}: bindings entries must be objects`);
    }
    if (typeof binding.name !== "string" || !isBindingIdentifier(binding.name)) {
      throw new RequestError("bad-fragment", `fragment ${id}: binding name is not a valid identifier`);
    }
    if (seen.has(binding.name)) {
      throw new RequestError("bad-fragment", `fragment ${id}: duplicate binding ${binding.name}`);
    }
    seen.add(binding.name);
    if (typeof binding.role !== "string" || !LOCAL_ROLES.has(binding.role)) {
      throw new RequestError("bad-fragment", `fragment ${id}: binding ${binding.name} has an unknown role`);
    }
    if (typeof binding.sourceExpr !== "string" || binding.sourceExpr.length === 0) {
      throw new RequestError("bad-fragment", `fragment ${id}: binding ${binding.name} has an empty sourceExpr`);
    }
    if (!isSourceRange(binding.sourceRange)) {
      throw new RequestError("bad-fragment", `fragment ${id}: binding ${binding.name} has an invalid sourceRange`);
    }
  }
}

/** Validate optional protocol v2 conditional chains. */
function validateGuardChains(id: string, guards: RequestGuardChain[] | undefined): void {
  if (guards === undefined) return;
  if (!Array.isArray(guards)) {
    throw new RequestError("bad-fragment", `fragment ${id}: guards must be an array`);
  }
  for (const chain of guards) {
    if (typeof chain !== "object" || chain === null || !Array.isArray(chain.conditions)) {
      throw new RequestError("bad-fragment", `fragment ${id}: guard chain has no conditions array`);
    }
    if (
      !Number.isInteger(chain.selected) ||
      chain.selected < 0 ||
      chain.selected > chain.conditions.length
    ) {
      throw new RequestError("bad-fragment", `fragment ${id}: guard chain has an invalid selected index`);
    }
    for (const condition of chain.conditions) {
      if (typeof condition !== "object" || condition === null) {
        throw new RequestError("bad-fragment", `fragment ${id}: guard conditions must be objects`);
      }
      if (typeof condition.branch !== "string" || !GUARD_BRANCHES.has(condition.branch)) {
        throw new RequestError("bad-fragment", `fragment ${id}: guard condition has an unknown branch`);
      }
      if (typeof condition.exprText !== "string" || condition.exprText.length === 0) {
        throw new RequestError("bad-fragment", `fragment ${id}: guard condition has an empty exprText`);
      }
      if (!isSourceRange(condition.range)) {
        throw new RequestError("bad-fragment", `fragment ${id}: guard condition has an invalid range`);
      }
    }
  }
}

/** Validate optional protocol v2 custom-component usages. */
function validateComponents(components: RequestComponentUsage[] | undefined): void {
  if (components === undefined) return;
  if (!Array.isArray(components)) {
    throw new RequestError("bad-params", "request.components must be an array");
  }
  for (const usage of components) {
    if (typeof usage !== "object" || usage === null) {
      throw new RequestError("bad-params", "components[] entries must be objects");
    }
    if (typeof usage.tag !== "string" || usage.tag.length === 0) {
      throw new RequestError("bad-params", "components[].tag is required");
    }
    if (usage.componentFileId !== undefined && (typeof usage.componentFileId !== "string" || usage.componentFileId.length === 0)) {
      throw new RequestError("bad-params", `components[${usage.tag}].componentFileId must be a non-empty string`);
    }
    if (!Array.isArray(usage.attributes)) {
      throw new RequestError("bad-params", `components[${usage.tag}].attributes must be an array`);
    }
    for (const attribute of usage.attributes) {
      if (typeof attribute !== "object" || attribute === null || typeof attribute.name !== "string" || attribute.name.length === 0) {
        throw new RequestError("bad-params", `components[${usage.tag}].attributes[].name is required`);
      }
      if (typeof attribute.valueKind !== "string" || !ATTRIBUTE_VALUE_KINDS.has(attribute.valueKind)) {
        throw new RequestError("bad-params", `components[${usage.tag}].${attribute.name} has an unknown valueKind`);
      }
      if (attribute.exprText !== undefined && typeof attribute.exprText !== "string") {
        throw new RequestError("bad-params", `components[${usage.tag}].${attribute.name}.exprText must be a string`);
      }
      if (attribute.exprRange !== undefined && !isSourceRange(attribute.exprRange)) {
        throw new RequestError("bad-params", `components[${usage.tag}].${attribute.name} has an invalid exprRange`);
      }
      if (!isSourceRange(attribute.attributeRange)) {
        throw new RequestError("bad-params", `components[${usage.tag}].${attribute.name} has an invalid attributeRange`);
      }
    }
  }
}

/** Validate optional protocol v2 static event bindings. */
function validateEvents(events: RequestEvent[] | undefined): void {
  if (events === undefined) return;
  if (!Array.isArray(events)) {
    throw new RequestError("bad-params", "request.events must be an array");
  }
  for (const event of events) {
    if (typeof event !== "object" || event === null) {
      throw new RequestError("bad-params", "events[] entries must be objects");
    }
    for (const key of ["eventId", "attribute", "event", "handler"] as const) {
      if (typeof event[key] !== "string" || event[key].length === 0) {
        throw new RequestError("bad-params", `events[].${key} is required`);
      }
    }
    if (!isSourceRange(event.attributeRange) || !isSourceRange(event.handlerRange)) {
      throw new RequestError("bad-params", `events[${event.eventId}] has an invalid range`);
    }
  }
}

/** Stricter of the request-declared and the server-side limit. */
export function effectiveLimits(requested: Partial<RequestLimits> | undefined): RequestLimits {
  const limits = requested ?? {};
  return {
    maxFiles: Math.min(limits.maxFiles ?? DEFAULT_LIMITS.maxFiles, SERVER_LIMITS.maxFiles),
    maxTotalBytes: Math.min(limits.maxTotalBytes ?? DEFAULT_LIMITS.maxTotalBytes, SERVER_LIMITS.maxTotalBytes),
    maxFragments: Math.min(limits.maxFragments ?? DEFAULT_LIMITS.maxFragments, SERVER_LIMITS.maxFragments),
  };
}

/**
 * Reject options the bridge does not model.
 *
 * Silently dropping an option would check the template under different rules
 * than the project, producing false negatives or false positives.
 */
export function validateCompilerOptions(options: RequestCompilerOptions): void {
  if (typeof options !== "object" || options === null || Array.isArray(options)) {
    throw new RequestError("bad-params", "request.project.compilerOptions must be an object");
  }
  for (const key of Object.keys(options)) {
    if (!REQUEST_COMPILER_OPTION_KEYS.has(key)) {
      throw new RequestError("unsupported-option", `compiler option is not supported by C1: ${key}`);
    }
  }
  const enumChecks: Array<[keyof RequestCompilerOptions, Set<string>]> = [
    ["target", TARGET_NAMES],
    ["module", MODULE_NAMES],
    ["moduleResolution", RESOLUTION_NAMES],
    ["jsx", JSX_NAMES],
    ["moduleDetection", MODULE_DETECTION_NAMES],
  ];
  for (const [key, allowed] of enumChecks) {
    const value = options[key];
    if (value !== undefined && (typeof value !== "string" || !allowed.has(value))) {
      throw new RequestError("bad-option", `compiler option ${String(key)} has an unknown value: ${String(value)}`);
    }
  }
  for (const key of BOOLEAN_COMPILER_OPTIONS) {
    const value = options[key as keyof RequestCompilerOptions];
    if (value !== undefined && typeof value !== "boolean") {
      throw new RequestError("bad-params", `compiler option ${key} must be a boolean`);
    }
  }
  for (const key of STRING_ARRAY_COMPILER_OPTIONS) {
    const value = options[key];
    if (value === undefined) continue;
    if (!Array.isArray(value) || value.some((entry) => typeof entry !== "string")) {
      throw new RequestError("bad-params", `compiler option ${key} must be a string array`);
    }
  }
  for (const key of ["baseUrl", "rootDir"] as const) {
    const value = options[key];
    if (value !== undefined && typeof value !== "string") {
      throw new RequestError("bad-params", `compiler option ${key} must be a string`);
    }
  }
  if (options.paths !== undefined) {
    const paths: unknown = options.paths;
    if (typeof paths !== "object" || paths === null || Array.isArray(paths)) {
      throw new RequestError("bad-params", "compiler option paths must be an object of string arrays");
    }
    for (const [pattern, targets] of Object.entries(paths)) {
      if (!Array.isArray(targets) || targets.some((entry) => typeof entry !== "string")) {
        throw new RequestError("bad-params", `compiler option paths[${pattern}] must be a string array`);
      }
    }
  }
}

const BOOLEAN_COMPILER_OPTIONS = [
  "strict",
  "strictNullChecks",
  "strictFunctionTypes",
  "strictBindCallApply",
  "strictPropertyInitialization",
  "noImplicitAny",
  "noImplicitThis",
  "noUncheckedIndexedAccess",
  "exactOptionalPropertyTypes",
  "strictBuiltinIteratorReturn",
  "useUnknownInCatchVariables",
  "allowJs",
  "checkJs",
  "esModuleInterop",
  "allowSyntheticDefaultImports",
  "resolveJsonModule",
  "isolatedModules",
  "verbatimModuleSyntax",
  "forceConsistentCasingInFileNames",
  "skipLibCheck",
] as const;

const STRING_ARRAY_COMPILER_OPTIONS = ["lib", "types", "typeRoots"] as const;

const TARGETS: Record<TargetName, ts.ScriptTarget> = {
  ES2020: ts.ScriptTarget.ES2020,
  ES2022: ts.ScriptTarget.ES2022,
  ESNext: ts.ScriptTarget.ESNext,
};

const MODULES: Record<ModuleKindName, ts.ModuleKind> = {
  ESNext: ts.ModuleKind.ESNext,
  CommonJS: ts.ModuleKind.CommonJS,
  Preserve: ts.ModuleKind.Preserve,
};

const RESOLUTIONS: Record<ModuleResolutionName, ts.ModuleResolutionKind> = {
  bundler: ts.ModuleResolutionKind.Bundler,
  node16: ts.ModuleResolutionKind.Node16,
  nodenext: ts.ModuleResolutionKind.NodeNext,
  node10: ts.ModuleResolutionKind.Node10,
};

const JSX: Record<JsxName, ts.JsxEmit> = {
  preserve: ts.JsxEmit.Preserve,
  react: ts.JsxEmit.React,
  "react-jsx": ts.JsxEmit.ReactJSX,
  "react-jsxdev": ts.JsxEmit.ReactJSXDev,
  "react-native": ts.JsxEmit.ReactNative,
};

const MODULE_DETECTION: Record<ModuleDetectionName, ts.ModuleDetectionKind> = {
  auto: ts.ModuleDetectionKind.Auto,
  legacy: ts.ModuleDetectionKind.Legacy,
  force: ts.ModuleDetectionKind.Force,
};

/** Convert the request options into TypeScript compiler options. */
export function toCompilerOptions(options: RequestCompilerOptions): ts.CompilerOptions {
  const converted: ts.CompilerOptions = {
    noEmit: true,
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
    ...(options.target ? { target: TARGETS[options.target] } : {}),
    ...(options.module ? { module: MODULES[options.module] } : {}),
    ...(options.moduleResolution ? { moduleResolution: RESOLUTIONS[options.moduleResolution] } : {}),
    ...(options.jsx ? { jsx: JSX[options.jsx] } : {}),
    ...(options.moduleDetection ? { moduleDetection: MODULE_DETECTION[options.moduleDetection] } : {}),
    ...(options.lib ? { lib: options.lib } : {}),
    ...(options.types ? { types: options.types } : {}),
    ...(options.typeRoots ? { typeRoots: options.typeRoots } : {}),
    ...(options.rootDir ? { rootDir: options.rootDir } : {}),
    ...(options.baseUrl ? { baseUrl: options.baseUrl } : {}),
    ...(options.paths ? { paths: options.paths } : {}),
  };
  return converted;
}

/**
 * Stable digest of the effective options, for logs and cache keys.
 *
 * `baseUrl` is made relative to `projectRoot` first so the digest does not
 * change when the same project is checked out at a different absolute path.
 */
export function optionsDigest(options: RequestCompilerOptions, projectRoot?: string): string {
  const normalized: Record<string, unknown> = { ...options };
  if (projectRoot && options.baseUrl) {
    const relative = path.relative(projectRoot, options.baseUrl);
    normalized.baseUrl = relative.split(path.sep).join("/") || ".";
  }
  const shape = JSON.stringify(normalized, Object.keys(normalized).sort());
  let hash = 2166136261;
  for (let index = 0; index < shape.length; index += 1) {
    hash ^= shape.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return `fnv1a:${(hash >>> 0).toString(16)}`;
}

export function fileKindOf(fileName: string): FileKind {
  if (fileName.endsWith(".d.ts")) return "d.ts";
  if (fileName.endsWith(".tsx")) return "tsx";
  if (fileName.endsWith(".jsx")) return "jsx";
  if (fileName.endsWith(".js")) return "js";
  return "ts";
}

const FILE_KINDS = new Set(["ts", "js", "jsx", "tsx", "d.ts"]);

/** Project-relative, `/`-separated, no `.`/`..`, no absolute path or NUL. */
export function isCanonicalProjectPath(value: string): boolean {
  if (typeof value !== "string" || value.length === 0) return false;
  if (value.includes("\0") || value.includes("\\")) return false;
  if (value.startsWith("/") || /^[A-Za-z]:/.test(value)) return false;
  const parts = value.split("/");
  return parts.length > 0 && parts.every((part) => part.length > 0 && part !== "." && part !== "..");
}

function validateStamp(stamp: AnalyzeRequest["analysisStamp"] | undefined): void {
  if (!stamp || typeof stamp !== "object") {
    throw new RequestError("bad-params", "analysisStamp is required");
  }
  for (const key of ["snapshotId", "projectRevision", "viewRevision", "backendEpoch"] as const) {
    const value = stamp[key];
    if (!Number.isInteger(value) || value < 0) {
      throw new RequestError("bad-params", `analysisStamp.${key} must be a non-negative integer`);
    }
  }
}

function validateFiles(files: RequestFile[]): void {
  const ids = new Set<string>();
  for (const file of files) {
    if (typeof file !== "object" || file === null) {
      throw new RequestError("bad-params", "files[] entries must be objects");
    }
    if (typeof file.fileId !== "string" || file.fileId.length === 0) {
      throw new RequestError("bad-params", "files[].fileId is required");
    }
    if (ids.has(file.fileId)) {
      throw new RequestError("bad-params", `duplicate fileId ${file.fileId}`);
    }
    ids.add(file.fileId);
    if (typeof file.path !== "string" || !isCanonicalProjectPath(file.path)) {
      throw new RequestError("bad-params", `files[].path is not project-relative: ${String(file.path)}`);
    }
    if (file.fileId !== file.path) {
      throw new RequestError("bad-params", `files[].fileId must match path: ${file.fileId} != ${file.path}`);
    }
    if (!FILE_KINDS.has(file.kind)) {
      throw new RequestError("bad-params", `files[].kind is unknown: ${String(file.kind)}`);
    }
    if (fileKindOf(file.path) !== file.kind) {
      throw new RequestError("bad-params", `files[].kind does not match its extension: ${file.path}`);
    }
    if (typeof file.text !== "string") {
      throw new RequestError("bad-params", `files[].text must be a string: ${file.path}`);
    }
    if (!Number.isInteger(file.sourceRevision) || file.sourceRevision < 1) {
      throw new RequestError("bad-params", `files[].sourceRevision must be a positive integer: ${file.path}`);
    }
  }
}
