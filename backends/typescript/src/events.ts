/**
 * Event handler resolution for the template event chain (WT-C).
 *
 * The template analysis sends every statically bound event (`events` in the
 * request). This module resolves the handler against the view's page/component
 * registration using the AST only: it never mounts fragments, never creates a
 * language service and never reads project sources from disk. Resolution must
 * therefore stay honest about what the AST cannot prove:
 *
 * - A handler missing from a complete, unambiguous owner method table is an
 *   error at the exact WXML handler token.
 * - Multiple registrations, a non-literal registration, spread members, computed
 *   names or a non-literal `Component.methods` make the table incomplete: the
 *   result is `partial` and a missing name is *not* reported as an error.
 * - A handler found through an ambiguous table still resolves (definition and
 *   signature are published); only the state is downgraded.
 *
 * Nothing here throws on malformed template input, and no message contains a
 * file path.
 */

import path from "node:path";
import ts from "typescript";
import { findRegistrations, docTextOf, memberMap, propertyName, type Registration } from "./ast.ts";
import type { PreparedView } from "./analysis.ts";
import { parseSource } from "./compiler.ts";
import type { AnalyzeRequest, EventResult, RequestEvent } from "./protocol.ts";
import type { BehaviorMethod } from "./behaviors.ts";

/** Defensive cap; the Rust side already sends at most 1024 events. */
export const MAX_EVENT_RESULTS = 1024;
/** A signature is a one-line snippet; longer members are truncated. */
export const MAX_SIGNATURE_LENGTH = 512;
/** User-visible missing-handler messages are bounded in UTF-8 bytes. */
const MAX_MESSAGE_BYTES = 512;
const MAX_HANDLER_IN_MESSAGE = 160;
const MAX_EVENT_IN_MESSAGE = 80;

/** Page option keys that describe the framework container, not handlers. */
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

export interface EventCollection {
  results: EventResult[];
  /** Events that could not be resolved at all (Partial coverage). */
  skipped: number;
  /** Up to three examples of skipped events. */
  skippedDetails: string[];
}

interface HandlerMember {
  name: string;
  node: ts.ObjectLiteralElementLike;
  /** Property-name node whose byte range becomes the definition. */
  nameNode: ts.Node;
  /** File that contains the declaration (own script or a behavior). */
  sourceFile: ts.SourceFile;
  /** Absolute path of that file, projected to a project-relative file id. */
  path: string;
}

interface OwnerTable {
  members: Map<string, HandlerMember>;
  /**
   * Names that exist in the owner object but whose value is not a literal
   * function (`onTap: handlerRef`, shorthand, accessor). They are not treated as
   * missing, because the AST cannot prove the value is not callable.
   */
  opaque: Set<string>;
  state: { status: "supported" | "partial"; reason?: string };
}

/**
 * Resolve every request event against the prepared view.
 *
 * One `EventResult` is returned per accepted event, in request order. A
 * malformed entry degrades to `partial` instead of throwing.
 */
export function collectEventResults(request: AnalyzeRequest, view: PreparedView): EventCollection {
  const events = Array.isArray(request.events) ? request.events : [];
  if (events.length === 0) return { results: [], skipped: 0, skippedDetails: [] };

  let sourceFile: ts.SourceFile;
  try {
    sourceFile = parseSource(view.scriptText, view.scriptPath);
  } catch {
    return {
      results: [],
      skipped: events.length,
      skippedDetails: ["events could not be read"],
    };
  }

  const kind = view.registration.kind;
  const scan = findRegistrations(sourceFile);
  const registrations = scan.registrations.filter((candidate) => candidate.kind === kind);
  const owner = buildOwnerTable(
    kind,
    registrations,
    scan.unsupportedCalls,
    view.behavior?.methods ?? [],
    view.behavior?.methodsIncomplete ?? false,
    sourceFile,
    view.scriptPath,
  );

  const byteOffset = utf8ByteOffsets(view.scriptText);
  const results: EventResult[] = [];
  const skippedDetails: string[] = [];
  let skipped = 0;
  const remember = (detail: string): void => {
    if (skippedDetails.length < 3 && !skippedDetails.includes(detail)) skippedDetails.push(detail);
  };

  if (events.length > MAX_EVENT_RESULTS) {
    skipped += events.length - MAX_EVENT_RESULTS;
    remember("event list is truncated");
  }
  const limited = events.slice(0, MAX_EVENT_RESULTS);

  for (const event of limited) {
    try {
      results.push(resolveEvent(request, view, owner, event, sourceFile, byteOffset));
    } catch {
      // Template input is untrusted; one bad AST node must not fail the request.
      skipped += 1;
      const eventId = typeof event?.eventId === "string" ? event.eventId : "";
      remember(eventId.length > 0 ? eventId : "event");
      results.push({
        eventId,
        state: { status: "partial", reason: "the event binding could not be resolved" },
      });
    }
  }

  return { results, skipped, skippedDetails };
}

/* ------------------------------------------------------------------ owner table */

function buildOwnerTable(
  kind: "page" | "component",
  registrations: Registration[],
  unsupportedCalls: string[],
  behaviorMethods: BehaviorMethod[],
  behaviorIncomplete: boolean,
  sourceFile: ts.SourceFile,
  scriptPath: string,
): OwnerTable {
  const members = new Map<string, HandlerMember>();
  const opaque = new Set<string>();
  const reasons: string[] = [];
  const label = kind === "page" ? "Page" : "Component";

  if (registrations.length === 0) {
    const reason = unsupportedCalls.some((call) => call.startsWith(`${label} `))
      ? "registration is not a literal object"
      : `no local ${label} registration`;
    return { members, opaque, state: { status: "partial", reason } };
  }
  if (registrations.length > 1) reasons.push(`multiple ${label} registrations`);
  // A non-literal registration may be the one that wins at runtime, so even a
  // literal table cannot be treated as complete.
  if (unsupportedCalls.some((call) => call.startsWith(`${label} `))) {
    pushReason(reasons, "registration is not a literal object");
  }

  const registration = registrations[0];
  if (!registration) {
    return { members, opaque, state: { status: "partial", reason: `no local ${label} registration` } };
  }

  if (kind === "page") {
    collectObjectMembers(registration.options.properties, members, opaque, reasons, sourceFile, scriptPath);
  } else {
    collectComponentMethods(registration.options.properties, members, opaque, reasons, sourceFile, scriptPath);
  }

  // Behavior-contributed methods are inherited after the owner's own table, so
  // an own method always wins. An incomplete behavior graph downgrades the
  // table instead of reporting a false missing-handler error.
  for (const method of behaviorMethods) {
    if (members.has(method.name) || opaque.has(method.name)) continue;
    members.set(method.name, {
      name: method.name,
      node: method.node,
      nameNode: method.nameNode,
      sourceFile: method.sourceFile,
      path: method.path,
    });
  }
  if (behaviorIncomplete) {
    pushReason(reasons, "component behavior methods are partially resolved");
  }

  return {
    members,
    opaque,
    state: reasons.length > 0 ? { status: "partial", reason: reasons.join("; ") } : { status: "supported" },
  };
}

/** Page: top-level option members are the handler table. */
function collectObjectMembers(
  properties: ts.NodeArray<ts.ObjectLiteralElementLike>,
  members: Map<string, HandlerMember>,
  opaque: Set<string>,
  reasons: string[],
  sourceFile: ts.SourceFile,
  scriptPath: string,
): void {
  for (const property of properties) {
    if (ts.isSpreadAssignment(property)) {
      pushReason(reasons, "options contain spread members");
      continue;
    }
    const name = propertyName(property);
    if (name === undefined) {
      // A computed member may define any handler name at runtime.
      pushReason(reasons, "registration contains computed member names");
      continue;
    }
    if (name.startsWith("__wxml_") || PAGE_RESERVED.has(name)) continue;
    addMember(name, property, members, opaque, sourceFile, scriptPath);
  }
}

/** Component: only `methods` members are handlers. */
function collectComponentMethods(
  properties: ts.NodeArray<ts.ObjectLiteralElementLike>,
  members: Map<string, HandlerMember>,
  opaque: Set<string>,
  reasons: string[],
  sourceFile: ts.SourceFile,
  scriptPath: string,
): void {
  let methodsMember: ts.ObjectLiteralElementLike | undefined;
  for (const property of properties) {
    if (ts.isSpreadAssignment(property)) {
      pushReason(reasons, "options contain spread members");
      continue;
    }
    const name = propertyName(property);
    if (name === "methods") methodsMember = property;
  }
  // A component without `methods` genuinely has no handlers: the empty table is
  // complete, so a missing handler stays a real error.
  if (methodsMember === undefined) return;
  if (!ts.isPropertyAssignment(methodsMember) || !ts.isObjectLiteralExpression(methodsMember.initializer)) {
    pushReason(reasons, "component `methods` is not a literal object");
    return;
  }

  for (const property of methodsMember.initializer.properties) {
    if (ts.isSpreadAssignment(property)) {
      pushReason(reasons, "`methods` contains spread members");
      continue;
    }
    const name = propertyName(property);
    if (name === undefined) {
      pushReason(reasons, "`methods` contains computed member names");
      continue;
    }
    if (name.startsWith("__wxml_")) continue;
    addMember(name, property, members, opaque, sourceFile, scriptPath);
  }
}

function addMember(
  name: string,
  property: ts.ObjectLiteralElementLike,
  members: Map<string, HandlerMember>,
  opaque: Set<string>,
  sourceFile: ts.SourceFile,
  scriptPath: string,
): void {
  const nameNode = property.name ?? property;
  if (ts.isMethodDeclaration(property)) {
    members.set(name, { name, node: property, nameNode, sourceFile, path: scriptPath });
    return;
  }
  if (ts.isPropertyAssignment(property) && isFunctionValue(property.initializer)) {
    members.set(name, { name, node: property, nameNode, sourceFile, path: scriptPath });
    return;
  }
  // Shorthand/identifier/accessor member: the name is known, its value is not
  // statically a function. Never invent a resolution, never report it missing.
  opaque.add(name);
}

function isFunctionValue(node: ts.Expression): boolean {
  return ts.isArrowFunction(node) || ts.isFunctionExpression(node);
}

function pushReason(reasons: string[], reason: string): void {
  if (!reasons.includes(reason)) reasons.push(reason);
}

/* ------------------------------------------------------------------ handler facts */

/** Owner layout and resolution of one statically named event handler. */
export interface HandlerFacts {
  state: { status: "supported" | "partial"; reason?: string };
  /** Declaration name node of a literal function member, when found. */
  nameNode?: ts.Node;
  /** The name exists but its value is not a literal function. */
  opaque: boolean;
  /** Page/Component options object, when the registration is literal. */
  options?: ts.ObjectLiteralExpression;
  /** Component `methods` object literal, when present and literal. */
  methods?: ts.ObjectLiteralExpression;
  /** Object receiving a missing handler (`methods` or the registration options). */
  container?: ts.ObjectLiteralExpression;
  /** Handler names already present or explicitly opaque. */
  taken: string[];
}

/**
 * Resolve one handler name against the literal owner table.
 *
 * This is the same table that `collectEventResults` uses, so a code action and
 * a diagnostic cannot disagree about whether a handler exists. The caller must
 * still respect `state`: a partial table makes a missing name unprovable.
 */
export function handlerFacts(
  sourceFile: ts.SourceFile,
  kind: "page" | "component",
  name: string,
): HandlerFacts {
  const scan = findRegistrations(sourceFile);
  const matching = scan.registrations.filter((candidate) => candidate.kind === kind);
  const owner = buildOwnerTable(kind, matching, scan.unsupportedCalls, [], false, sourceFile, sourceFile.fileName);
  const registration = matching.length === 1 ? matching[0] : undefined;
  const options = registration?.options;
  let methods: ts.ObjectLiteralExpression | undefined;
  if (kind === "component" && options) {
    const member = memberMap(options).get("methods");
    if (member && ts.isPropertyAssignment(member) && ts.isObjectLiteralExpression(member.initializer)) {
      methods = member.initializer;
    }
  }
  const taken = [...owner.members.keys(), ...owner.opaque];
  const found = owner.members.get(name);
  return {
    state: owner.state,
    ...(found ? { nameNode: found.nameNode } : {}),
    opaque: owner.opaque.has(name),
    ...(options ? { options } : {}),
    ...(methods ? { methods } : {}),
    ...((methods ?? options) ? { container: methods ?? options } : {}),
    taken,
  };
}

/* --------------------------------------------------------------------- one event */

function resolveEvent(
  request: AnalyzeRequest,
  view: PreparedView,
  owner: OwnerTable,
  event: RequestEvent,
  sourceFile: ts.SourceFile,
  byteOffset: ByteOffset,
): EventResult {
  const eventId = typeof event?.eventId === "string" ? event.eventId : "";
  const handler = typeof event?.handler === "string" ? event.handler.trim() : "";
  if (!isIdentifierText(handler)) {
    // Dynamic/non-identifier handlers are filtered by Rust; stay defensive.
    return { eventId, state: { status: "partial", reason: "event handler is not a static identifier" } };
  }
  if (!isRange(event?.handlerRange)) {
    return { eventId, state: { status: "partial", reason: "event binding range is invalid" } };
  }

  const member = owner.members.get(handler);
  if (member) {
    const offset =
      member.sourceFile === sourceFile ? byteOffset : utf8ByteOffsets(member.sourceFile.text);
    const range = nameByteRange(member.nameNode, member.sourceFile, member.sourceFile.text, offset);
    const signature = signatureOf(member, member.sourceFile);
    const documentation = docTextOf(member.node);
    return {
      eventId,
      state: owner.state,
      ...(range
        ? {
            definition: {
              fileId:
                path.resolve(member.path) === path.resolve(view.scriptPath)
                  ? request.scriptFileId
                  : fileIdFor(request, member.path),
              range,
            },
          }
        : {}),
      ...(signature ? { signature } : {}),
      ...(documentation ? { documentation } : {}),
    };
  }

  if (owner.opaque.has(handler)) {
    return {
      eventId,
      state: {
        status: "partial",
        reason: `event handler "${boundedUtf8(handler, MAX_HANDLER_IN_MESSAGE)}" is not a literal function`,
      },
    };
  }

  if (owner.state.status === "partial") {
    // An ambiguous table cannot prove the handler is missing.
    return { eventId, state: owner.state };
  }

  const eventName = typeof event?.event === "string" ? event.event : "";
  const ownerLabel = view.registration.kind === "component" ? "component" : "page";
  return {
    eventId,
    state: { status: "supported" },
    diagnostic: {
      range: event.handlerRange,
      severity: "error",
      message: missingHandlerMessage(handler, eventName, ownerLabel),
    },
  };
}

function missingHandlerMessage(handler: string, eventName: string, ownerLabel: string): string {
  const safeHandler = boundedUtf8(sanitizeName(handler), MAX_HANDLER_IN_MESSAGE);
  const safeEvent = boundedUtf8(sanitizeName(eventName), MAX_EVENT_IN_MESSAGE);
  const message = `Event handler "${safeHandler}" for "${safeEvent}" is not defined on this ${ownerLabel}`;
  return boundedUtf8(message, MAX_MESSAGE_BYTES);
}

/** Project-relative file id matching `request.files[].fileId`. */
function fileIdFor(request: AnalyzeRequest, absolute: string): string {
  const relative = path.relative(
    path.resolve(request.project.projectRoot),
    path.resolve(absolute),
  );
  return relative === "" ? "." : relative.split(path.sep).join("/");
}

/** Remove characters that would break a quoted, single-line message. */
function sanitizeName(text: string): string {
  return text.replace(/["\\\u0000-\u001f\u007f]/g, " ");
}

/* ---------------------------------------------------------------------- helpers */

type ByteOffset = (utf16: number) => number;

/** One UTF-16 -> UTF-8 offset conversion table per request (O(text) once). */
function utf8ByteOffsets(text: string): ByteOffset {
  const boundaries: Array<{ at: number; delta: number }> = [];
  let delta = 0;
  let units = 0;
  for (const char of text) {
    const code = char.codePointAt(0) ?? 0;
    const charUnits = char.length;
    const bytes = code <= 0x7f ? 1 : code <= 0x7ff ? 2 : code <= 0xffff ? 3 : 4;
    if (bytes !== charUnits) {
      delta += bytes - charUnits;
      boundaries.push({ at: units + charUnits, delta });
    }
    units += charUnits;
  }
  if (boundaries.length === 0) return (offset) => offset;
  return (offset) => {
    let low = 0;
    let high = boundaries.length;
    while (low < high) {
      const mid = (low + high) >>> 1;
      if (boundaries[mid]!.at <= offset) low = mid + 1;
      else high = mid;
    }
    return offset + (low === 0 ? 0 : boundaries[low - 1]!.delta);
  };
}

function nameByteRange(
  node: ts.Node,
  sourceFile: ts.SourceFile,
  scriptText: string,
  byteOffset: ByteOffset,
): { start: number; end: number } | undefined {
  let start: number;
  let end: number;
  try {
    start = node.getStart(sourceFile);
    end = node.getEnd();
  } catch {
    return undefined;
  }
  if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end < start || end > scriptText.length) {
    return undefined;
  }
  return { start: byteOffset(start), end: byteOffset(end) };
}

function signatureOf(member: HandlerMember, sourceFile: ts.SourceFile): string | undefined {
  try {
    const node = member.node;
    let text: string;
    if (ts.isMethodDeclaration(node)) {
      const bodyStart = node.body?.getStart(sourceFile);
      text =
        bodyStart === undefined
          ? node.getText(sourceFile)
          : sourceFile.text.slice(node.getStart(sourceFile), bodyStart);
    } else if (
      ts.isPropertyAssignment(node) &&
      (ts.isArrowFunction(node.initializer) || ts.isFunctionExpression(node.initializer))
    ) {
      const bodyStart = node.initializer.body.getStart(sourceFile);
      text = `${member.name} = ${sourceFile.text.slice(node.initializer.getStart(sourceFile), bodyStart)}...`;
    } else {
      text = node.getText(sourceFile);
    }
    const oneLine = text.replace(/\s+/g, " ").trim();
    if (oneLine.length === 0) return undefined;
    if (oneLine.length <= MAX_SIGNATURE_LENGTH) return oneLine;
    // Bound by code points so a surrogate pair is never split.
    return `${Array.from(oneLine).slice(0, MAX_SIGNATURE_LENGTH - 1).join("").trimEnd()}…`;
  } catch {
    return undefined;
  }
}

function isIdentifierText(value: string): boolean {
  return /^[\p{Alphabetic}_$][\p{Alphabetic}\p{N}_$]*$/u.test(value);
}

function isRange(value: unknown): value is { start: number; end: number } {
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

/** Truncate to a UTF-8 byte budget without splitting a code point. */
function boundedUtf8(text: string, maxBytes: number): string {
  const buffer = Buffer.from(text, "utf8");
  if (buffer.length <= maxBytes) return text;
  let end = Math.max(0, maxBytes);
  while (end > 0 && (buffer[end]! & 0xc0) === 0x80) end -= 1;
  return buffer.subarray(0, end).toString("utf8");
}
