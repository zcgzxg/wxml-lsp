/**
 * Completion for the two kinds that need the TypeScript language service.
 *
 * Static tag/attribute/value completion happens in Rust; only mustache member
 * completion (`{{ user.na }}`) and event-handler value completion
 * (`bindtap="on"`) are answered here.
 *
 * Mustache completion never feeds the raw, incomplete WXML expression into the
 * layout: a completion-safe expression with a synthetic identifier is mounted
 * instead, and the query point is derived from the mount's actual text.
 */

import ts from "typescript";
import { docTextOf, findRegistrations, memberMap, propertyName, type Registration } from "./ast.ts";
import { buildView, type PreparedView, type UnavailableView } from "./analysis.ts";
import { parseSource, VirtualProject } from "./compiler.ts";
import { ROOT_NAME } from "./layouts/inPlaceShadow.ts";
import { rootIdentifier } from "./expression.ts";
import {
  PROTOCOL_VERSION,
  RequestError,
  validateCompleteRequest,
  type CompleteItem,
  type CompleteRequest,
  type CompleteResponse,
  type RequestFragment,
} from "./protocol.ts";
import type { CapabilityState } from "./types.ts";

/** Hard cap on published items (both completion kinds). */
export const MAX_COMPLETION_ITEMS = 256;
/** Documentation is capped in UTF-16 code units. */
export const MAX_COMPLETION_DOCUMENTATION = 2048;
/** Detail strings are capped in UTF-16 code units. */
export const MAX_COMPLETION_DETAIL = 120;

const COMPLETION_PLACEHOLDER = "__wxml_completion";

/** Names that describe the registration container, not hand-written handlers. */
const PAGE_RESERVED = new Set([
  "data",
  "properties",
  "methods",
  "observers",
  "lifetimes",
  "pageLifetimes",
  "behaviors",
  "options",
  "externalClasses",
  "relations",
]);

export interface CompletionConfig {
  backendDir: string;
}

export interface CompletionResult {
  response: CompleteResponse;
}

interface Collected {
  items: CompleteItem[];
  truncated: boolean;
}

/**
 * Answer a `complete` request.
 *
 * Only shape problems are thrown (`validateCompleteRequest`); a missing or
 * unsupported derived view degrades to an empty `unsupported` response.
 */
export function complete(request: CompleteRequest, config: CompletionConfig): CompleteResponse {
  validateCompleteRequest(request);

  if (request.kind === "eventHandler") {
    const view = buildQuietly({ ...request, fragments: [] }, config);
    if (!view) return unavailable(request, "the layout could not be prepared");
    if (!view.ok) return unavailable(request, view.reason);
    if (view.build.buildStatus === "unsupported") {
      return unavailable(request, view.build.reason ?? "layout is unsupported");
    }
    const collected = collectEventHandlers(request, view);
    return {
      protocolVersion: PROTOCOL_VERSION,
      requestId: request.requestId,
      state: { status: "supported" },
      truncated: collected.truncated,
      items: collected.items,
    };
  }

  const collected = collectExpressionMembers(request, config);
  if (collected === undefined) {
    return unavailable(request, "no mustache expression at this offset");
  }
  if (!collected.ok) {
    return unavailable(request, collected.reason);
  }
  return {
    protocolVersion: PROTOCOL_VERSION,
    requestId: request.requestId,
    state: { status: "supported" },
    truncated: collected.truncated,
    items: collected.items,
  };
}

/** Exported for tests: event-handler candidates of a prepared view. */
export function completeEventHandlers(request: CompleteRequest, view: PreparedView): CompleteItem[] {
  return collectEventHandlers(request, view).items;
}

/* --------------------------------------------------------------- event handlers */

function collectEventHandlers(request: CompleteRequest, view: PreparedView): Collected {
  const sourceFile = parseSource(view.scriptText, view.scriptPath);
  const registration = findRegistrations(sourceFile).registrations.find(
    (candidate) => candidate.kind === view.registration.kind,
  );
  const members = registration ? eventMembers(registration) : [];
  const replace = eventReplace(request);
  const seen = new Set<string>();
  const items: CompleteItem[] = [];

  for (const member of members) {
    const name = propertyName(member);
    if (name === undefined) continue;
    if (name.startsWith("__wxml_")) continue;
    if (registration?.kind === "page" && PAGE_RESERVED.has(name)) continue;
    if (seen.has(name)) continue;
    seen.add(name);
    const detail = bounded(member.getText(sourceFile).replace(/^async\s+/, ""), MAX_COMPLETION_DETAIL);
    const documentation = bounded(docTextOf(member).trim(), MAX_COMPLETION_DOCUMENTATION);
    items.push({
      label: name,
      kind: memberKind(member),
      ...(detail.length > 0 ? { detail } : {}),
      ...(documentation.length > 0 ? { documentation } : {}),
      insertText: name,
      replace,
      sortText: `0_${name}`,
    });
  }

  items.sort((left, right) => left.label.localeCompare(right.label));
  const truncated = items.length > MAX_COMPLETION_ITEMS;
  return { items: items.slice(0, MAX_COMPLETION_ITEMS), truncated };
}

function eventMembers(registration: Registration): ts.ObjectLiteralElementLike[] {
  if (registration.kind === "page") return [...registration.options.properties];
  const methods = memberMap(registration.options).get("methods");
  if (methods && ts.isPropertyAssignment(methods) && ts.isObjectLiteralExpression(methods.initializer)) {
    return [...methods.initializer.properties];
  }
  return [];
}

function memberKind(member: ts.ObjectLiteralElementLike): "method" | "property" {
  if (ts.isMethodDeclaration(member)) return "method";
  if (ts.isPropertyAssignment(member)) {
    const initializer = member.initializer;
    return ts.isArrowFunction(initializer) || ts.isFunctionExpression(initializer) ? "method" : "property";
  }
  return "property";
}

function eventReplace(request: CompleteRequest): { start: number; end: number } {
  const prefixBytes = Buffer.byteLength(request.prefix, "utf8");
  return { start: Math.max(0, request.offset - prefixBytes), end: request.offset };
}

/* ------------------------------------------------------------ expression members */

type ExpressionOutcome =
  | ({ ok: true } & Collected)
  | { ok: false; reason: string }
  | undefined;

/**
 * Member completion for the mustache that contains the cursor.
 *
 * Returns `undefined` when no fragment covers the offset, `{ ok: false }` when
 * the synthetic view cannot be prepared, and the items otherwise.
 */
function collectExpressionMembers(
  request: CompleteRequest,
  config: CompletionConfig,
): ExpressionOutcome {
  const fragment = request.fragments.find(
    (candidate) =>
      candidate.wxmlRange !== undefined &&
      request.offset >= candidate.wxmlRange.start &&
      request.offset <= candidate.wxmlRange.end,
  );
  if (!fragment) return undefined;

  const fragmentBytes = Buffer.byteLength(fragment.exprText, "utf8");
  const cutBytes = Math.max(0, Math.min(request.offset - fragment.wxmlRange.start, fragmentBytes));
  // Leading whitespace inside `{{ ... }}` is not part of the expression root.
  const exprSoFar = cutUtf8(fragment.exprText, cutBytes).replace(/^\s+/, "");

  const endsWithDot = exprSoFar.endsWith(".");
  let identifierPrefix = "";
  let needsPlaceholder: boolean;
  if (endsWithDot) {
    needsPlaceholder = true;
  } else {
    const match = /[A-Za-z0-9_$]*$/.exec(exprSoFar);
    identifierPrefix = match ? match[0] : "";
    needsPlaceholder = identifierPrefix.length === 0;
  }

  const syntheticText = exprSoFar + (needsPlaceholder ? COMPLETION_PLACEHOLDER : "");
  const candidateRoot = exprSoFar.length > 0 ? rootIdentifier(exprSoFar) : "";
  const target = isIdentifierText(candidateRoot) ? candidateRoot : COMPLETION_PLACEHOLDER;
  if (target === COMPLETION_PLACEHOLDER && !syntheticText.includes(COMPLETION_PLACEHOLDER)) {
    return { ok: false, reason: "the expression before the cursor has no completable member" };
  }

  const synthetic: RequestFragment = {
    fragmentId: "complete-0",
    exprText: syntheticText,
    target,
    ...(fragment.locals !== undefined ? { locals: fragment.locals } : {}),
    wxmlRange: {
      start: fragment.wxmlRange.start,
      end: fragment.wxmlRange.start + Buffer.byteLength(syntheticText, "utf8"),
    },
    fragmentRevision: fragment.fragmentRevision,
    root: fragment.root,
  };

  const view = buildQuietly({ ...request, fragments: [synthetic] }, config);
  if (!view) return { ok: false, reason: "the completion expression could not be mounted" };
  if (!view.ok) return { ok: false, reason: view.reason };
  if (view.build.buildStatus === "unsupported") {
    return { ok: false, reason: view.build.reason ?? "layout is unsupported" };
  }
  const mount = view.build.fragments[0]?.mounts[0];
  if (!mount) return { ok: false, reason: "the completion expression could not be mounted" };

  const placeholderIndex = mount.text.lastIndexOf(COMPLETION_PLACEHOLDER);
  const queryOffset = needsPlaceholder
    ? placeholderIndex >= 0
      ? placeholderIndex
      : Math.max(0, mount.text.length - COMPLETION_PLACEHOLDER.length)
    : mount.text.length;
  const queryPosition = mount.startUtf16 + queryOffset;

  const project = view.project;
  project.refresh([
    ...[...view.files].map(([filePath, text]) => ({ path: filePath, text })),
    ...view.build.files,
  ]);

  // Completing a bare root (`{{ us`) asks for the keys of `this.data`, not for
  // members of a symbol named `us`; the language service cannot answer that
  // from a property-access position, so enumerate the data-root type.
  const rootOnly =
    !exprSoFar.includes(".") &&
    !exprSoFar.includes("[") &&
    (exprSoFar.length === 0 || /^[\p{L}_$][\p{L}\p{N}_$]*$/u.test(exprSoFar));
  if (rootOnly) {
    const roots = collectRootMembers(request, view, project, identifierPrefix);
    if (roots !== undefined) return { ok: true, ...roots };
  }

  const completions =
    project.languageService().getCompletionsAtPosition(view.scriptPath, queryPosition, {
      includeCompletionsForModuleExports: false,
      includeCompletionsWithInsertText: true,
    }) ?? { entries: [] };

  const replace = needsPlaceholder
    ? { start: request.offset, end: request.offset }
    : { start: Math.max(0, request.offset - Buffer.byteLength(identifierPrefix, "utf8")), end: request.offset };

  const seen = new Set<string>();
  const items: CompleteItem[] = [];
  let truncated = false;
  for (const entry of completions.entries) {
    const name = entry.name;
    if (name.startsWith("__") || name.startsWith("#")) continue;
    if (seen.has(name)) continue;
    seen.add(name);
    if (items.length >= MAX_COMPLETION_ITEMS) {
      truncated = true;
      break;
    }
    const detail = entry.source ? bounded(`from ${entry.source}`, MAX_COMPLETION_DETAIL) : undefined;
    items.push({
      label: name,
      kind: completionKind(entry.kind),
      ...(detail !== undefined && detail.length > 0 ? { detail } : {}),
      insertText: entry.insertText !== undefined && !entry.insertText.includes("$") ? entry.insertText : name,
      replace,
      sortText: entry.sortText ?? name,
    });
  }

  return { ok: true, items, truncated };
}

/** Data-root keys for a bare `{{ prefix` completion. */
function collectRootMembers(
  request: CompleteRequest,
  view: PreparedView,
  project: VirtualProject,
  identifierPrefix: string,
): Collected | undefined {
  const sourceFile = project.program().getSourceFile(view.scriptPath);
  if (!sourceFile) return undefined;
  let declaration: ts.VariableDeclaration | undefined;
  const visit = (node: ts.Node): void => {
    if (
      ts.isVariableDeclaration(node) &&
      ts.isIdentifier(node.name) &&
      node.name.text === ROOT_NAME
    ) {
      declaration = node;
      return;
    }
    node.forEachChild(visit);
  };
  visit(sourceFile);
  if (!declaration) return undefined;

  const checker = project.checker();
  const rootType = checker.getTypeAtLocation(declaration.name);
  const symbols = checker.getPropertiesOfType(checker.getApparentType(rootType));
  const replace = {
    start: Math.max(0, request.offset - Buffer.byteLength(identifierPrefix, "utf8")),
    end: request.offset,
  };
  const items: CompleteItem[] = [];
  const seen = new Set<string>();
  let truncated = false;
  for (const symbol of symbols) {
    const name = symbol.getName();
    if (name.startsWith("__") || name.startsWith("#") || seen.has(name)) continue;
    seen.add(name);
    if (items.length >= MAX_COMPLETION_ITEMS) {
      truncated = true;
      break;
    }
    const typeText = bounded(
      checker.typeToString(checker.getTypeOfSymbolAtLocation(symbol, declaration)),
      MAX_COMPLETION_DETAIL,
    );
    const documentation = bounded(
      symbol.getDocumentationComment(checker).map((part) => part.text).join("\n"),
      MAX_COMPLETION_DOCUMENTATION,
    );
    const isMethod = (symbol.flags & ts.SymbolFlags.Method) !== 0;
    items.push({
      label: name,
      kind: isMethod ? "method" : "property",
      ...(typeText.length > 0 ? { detail: typeText } : {}),
      ...(documentation.length > 0 ? { documentation } : {}),
      insertText: name,
      replace,
      sortText: `0_${name}`,
    });
  }
  items.sort((left, right) => left.label.localeCompare(right.label));
  return { items, truncated };
}

function completionKind(kind: ts.ScriptElementKind): CompleteItem["kind"] {
  switch (kind as string) {
    case "method":
      return "method";
    case "keyword":
      return "keyword";
    case "property":
    case "getter":
    case "setter":
    case "field":
    case "var":
    case "const":
    case "parameter":
      return "property";
    default:
      return "property";
  }
}

/* ------------------------------------------------------------------------ helpers */

function unavailable(request: CompleteRequest, reason: string): CompleteResponse {
  const state: CapabilityState = { status: "unsupported", reason };
  return {
    protocolVersion: PROTOCOL_VERSION,
    requestId: request.requestId,
    state,
    truncated: false,
    items: [],
  };
}

/**
 * `buildView` throws `RequestError` for fragments it cannot plan (the synthetic
 * expression is still untrusted input); completion must degrade, not fail the
 * request, so the error is turned into "no view".
 */
function buildQuietly(
  request: CompleteRequest,
  config: CompletionConfig,
): PreparedView | UnavailableView | undefined {
  try {
    return buildView(request, config);
  } catch (error) {
    if (error instanceof RequestError) return undefined;
    throw error;
  }
}

/** Cut a UTF-8 byte prefix without splitting a code point. */
function cutUtf8(text: string, maxBytes: number): string {
  const buffer = Buffer.from(text, "utf8");
  let end = Math.max(0, Math.min(maxBytes, buffer.length));
  while (end > 0 && (buffer[end]! & 0xc0) === 0x80) end -= 1;
  return buffer.subarray(0, end).toString("utf8");
}

function isIdentifierText(value: string): boolean {
  return /^[\p{L}_$][\p{L}\p{N}_$]*$/u.test(value);
}

function bounded(text: string, limit: number): string {
  return text.length <= limit ? text : text.slice(0, limit);
}
