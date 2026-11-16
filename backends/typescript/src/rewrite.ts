/**
 * Expression root rewriting.
 *
 * WXML expressions are parsed as TypeScript expressions so every *free* root
 * identifier gets the data-root prefix (or none for template locals), while
 * property names, object keys, string/template text, comments, keywords and
 * global values are left untouched. The rewrite records one segment per emitted
 * piece so diagnostics can be mapped back to WXML bytes.
 */

import ts from "typescript";
import {
  describeExpression,
  identifierOffset,
  parseExpression,
  rightmostIdentifierToken,
  scanIdentifiers,
  type IdentifierToken,
} from "./expression.ts";
import { RequestError } from "./protocol.ts";
import { byteRangeOf } from "./text.ts";
import type { Segment, SourceByteRange } from "./types.ts";

/** Hard limits for untrusted expressions. */
const MAX_EXPRESSION_DEPTH = 256;
const MAX_EXPRESSION_NODES = 10_000;
/** Bound the per-expression hover candidates returned to the LSP. */
const MAX_HOVER_TARGETS = 32;

/** Explicit target name or a parsed token (default target keeps its offset). */
export type RewriteTarget = string | IdentifierToken | undefined;

/** Values that are not template data roots. */
const GLOBALS = new Set([
  "Math", "JSON", "Date", "Number", "String", "Boolean", "Array", "Object", "RegExp",
  "Symbol", "BigInt", "Map", "Set", "WeakMap", "WeakSet", "Promise", "console", "Intl",
  "parseInt", "parseFloat", "isNaN", "isFinite", "encodeURI", "decodeURI",
  "encodeURIComponent", "decodeURIComponent", "undefined", "NaN", "Infinity",
]);

export interface RewriteOptions {
  /** Prefix for a data root (`__wxml_root.`). */
  rootPrefix: string;
  /** Prefix for a declared `properties` field (`__wxml_props_0.`). */
  propsPrefix: string;
  /** Property names that read from the declaration-derived overlay. */
  propertyOverlay: ReadonlySet<string>;
  /** Template locals (no prefix). */
  locals: ReadonlySet<string>;
}

export interface RewrittenExpression {
  /** Mounted text with prefixes inserted. */
  text: string;
  /** Segments relative to `text`; synthetic prefixes have no `source`. */
  segments: Segment[];
  /** Target offset relative to `text` (UTF-16). */
  targetOffset: number;
  /** Identifier offsets relative to `text`, with ranges in `expr` bytes. */
  hoverTargets: Array<{ offset: number; source: SourceByteRange }>;
  /** Root identifiers in source order (deduplicated). */
  roots: string[];
  /** True when every root is a template local. */
  localRoot: boolean;
}

interface Occurrence {
  start: number;
  end: number;
  name: string;
  shorthand: boolean;
}

/** Parse `expr` as a TypeScript expression and rewrite free roots. */
export function rewriteExpression(
  expr: string,
  target: RewriteTarget,
  options: RewriteOptions,
): RewrittenExpression {
  const parsed = parseExpression(expr);
  if (!parsed) {
    throw new RequestError(
      "bad-fragment",
      `expression cannot be parsed as exactly one parenthesized expression: ${describeExpression(expr)}`,
    );
  }
  const { source, root: rootExpression, exprStart: prefixLength } = parsed;

  const occurrences: Occurrence[] = [];
  const roots: string[] = [];
  const rootSet = new Set<string>();
  let unsupported: string | undefined;
  let nodes = 0;

  const record = (node: ts.Identifier, shorthand: boolean): void => {
    occurrences.push({
      start: node.getStart(source) - prefixLength,
      end: node.getEnd() - prefixLength,
      name: node.text,
      shorthand,
    });
    if (!rootSet.has(node.text)) {
      rootSet.add(node.text);
      roots.push(node.text);
    }
  };

  const visit = (node: ts.Node, depth: number): void => {
    if (unsupported) return;
    nodes += 1;
    if (depth > MAX_EXPRESSION_DEPTH || nodes > MAX_EXPRESSION_NODES) {
      unsupported = "expression exceeds traversal depth/node limits";
      return;
    }
    if (
      ts.isArrowFunction(node) ||
      ts.isFunctionExpression(node) ||
      ts.isClassExpression(node) ||
      ts.isNewExpression(node) ||
      ts.isSatisfiesExpression(node) ||
      ts.isTypeNode(node) ||
      ts.isDeleteExpression(node) ||
      (ts.isBinaryExpression(node) && isAssignmentOperator(node.operatorToken.kind)) ||
      (ts.isPrefixUnaryExpression(node) &&
        (node.operator === ts.SyntaxKind.PlusPlusToken || node.operator === ts.SyntaxKind.MinusMinusToken)) ||
      (ts.isPostfixUnaryExpression(node) &&
        (node.operator === ts.SyntaxKind.PlusPlusToken || node.operator === ts.SyntaxKind.MinusMinusToken)) ||
      ts.isAwaitExpression(node) ||
      ts.isYieldExpression(node) ||
      ts.isJsxElement(node) ||
      ts.isJsxSelfClosingElement(node) ||
      ts.isTypeAssertionExpression(node) ||
      ts.isAsExpression(node) ||
      ts.isNonNullExpression(node)
    ) {
      unsupported = ts.SyntaxKind[node.kind];
      return;
    }
    if (ts.isPropertyAccessExpression(node)) {
      visit(node.expression, depth + 1);
      return;
    }
    if (ts.isPropertyAssignment(node)) {
      if (ts.isComputedPropertyName(node.name)) visit(node.name.expression, depth + 1);
      visit(node.initializer, depth + 1);
      return;
    }
    if (ts.isShorthandPropertyAssignment(node)) {
      // A template local may legally shadow a global builtin name.
      if (!GLOBALS.has(node.name.text) || options.locals.has(node.name.text)) {
        record(node.name, true);
      }
      return;
    }
    if (ts.isMethodDeclaration(node) || ts.isGetAccessor(node) || ts.isSetAccessor(node)) {
      unsupported = ts.SyntaxKind[node.kind];
      return;
    }
    if (ts.isIdentifier(node)) {
      if (node.text === "this") return;
      if (GLOBALS.has(node.text) && !options.locals.has(node.text)) return;
      // Template locals are recorded too: they get an empty prefix, which lets
      // the fragment stay mountable and marks the capability Partial.
      record(node, false);
      return;
    }
    node.forEachChild((child) => visit(child, depth + 1));
  };
  visit(rootExpression, 0);

  if (unsupported) {
    throw new RequestError("bad-fragment", `expression shape is not supported (${unsupported}): ${describeExpression(expr)}`);
  }
  if (occurrences.length === 0) {
    throw new RequestError("bad-fragment", `expression has no free root identifier: ${describeExpression(expr)}`);
  }

  occurrences.sort((left, right) => left.start - right.start);
  const pieces: Array<{ text: string; source?: { start: number; end: number } }> = [];
  const sourceMap: Array<{ sourceStartUtf16: number; sourceEndUtf16: number; mountedStartUtf16: number }> = [];
  let sourceBytes = 0;
  let mounted = 0;

  const pushSynthetic = (text: string): void => {
    if (text.length === 0) return;
    pieces.push({ text });
    mounted += text.length;
  };

  const pushSource = (
    from: number,
    to: number,
    map = true,
    reuse?: { start: number; end: number },
  ): { start: number; end: number } | undefined => {
    if (to <= from) return undefined;
    const text = expr.slice(from, to);
    const start = reuse?.start ?? sourceBytes;
    const end = reuse?.end ?? start + Buffer.byteLength(text, "utf8");
    pieces.push({ text, source: { start, end } });
    if (map) {
      sourceMap.push({ sourceStartUtf16: from, sourceEndUtf16: to, mountedStartUtf16: mounted });
    }
    mounted += text.length;
    if (!reuse) sourceBytes = end;
    return { start, end };
  };

  let cursor = 0;
  for (const occurrence of occurrences) {
    if (occurrence.start < cursor) {
      throw new RequestError("bad-fragment", `overlapping root identifiers in ${describeExpression(expr)}`);
    }
    pushSource(cursor, occurrence.start);
    const prefix = options.locals.has(occurrence.name)
      ? ""
      : options.propertyOverlay.has(occurrence.name)
        ? options.propsPrefix
        : options.rootPrefix;
    if (occurrence.shorthand) {
      // `{ title }` -> `{ title: <prefix>title }` (safe expansion). The key is not
      // a mapping target; hover/diagnostics follow the value copy. Reuse the
      // key's source range so the expression bytes are counted once.
      const keyRange = pushSource(occurrence.start, occurrence.end, false);
      pushSynthetic(prefix.length > 0 ? `: ${prefix}` : ": ");
      pushSource(occurrence.start, occurrence.end, true, keyRange);
    } else {
      pushSynthetic(prefix);
      pushSource(occurrence.start, occurrence.end);
    }
    cursor = occurrence.end;
  }
  pushSource(cursor, expr.length);

  const text = pieces.map((piece) => piece.text).join("");

  // Map any source UTF-16 offset (for example a member property name) into the
  // mounted text.
  const mapSourceOffset = (offset: number): number | undefined => {
    for (const entry of sourceMap) {
      if (offset >= entry.sourceStartUtf16 && offset < entry.sourceEndUtf16) {
        return entry.mountedStartUtf16 + (offset - entry.sourceStartUtf16);
      }
    }
    return undefined;
  };

  const targetToken = resolveTarget(expr, target);
  if (!targetToken) {
    throw new RequestError("bad-fragment", `expression has no valid target identifier: ${describeExpression(expr)}`);
  }
  const targetOffset = mapSourceOffset(targetToken.start);
  if (targetOffset === undefined) {
    throw new RequestError(
      "bad-fragment",
      `target ${targetToken.name} is not part of the mounted expression: ${describeExpression(expr)}`,
    );
  }

  // Merge adjacent source pieces: a local fragment must stay one contiguous
  // source segment (no synthetic gap), and diagnostics map over whole runs.
  const segments: Segment[] = [];
  let mountedCursor = 0;
  for (const piece of pieces) {
    const previous = segments[segments.length - 1];
    const canMerge =
      previous !== undefined &&
      previous.source !== undefined &&
      piece.source !== undefined &&
      previous.startUtf16 + previous.lengthUtf16 === mountedCursor &&
      previous.source.end === piece.source.start;
    if (canMerge) {
      previous.lengthUtf16 += piece.text.length;
      previous.source = { start: previous.source!.start, end: piece.source!.end };
    } else {
      segments.push({
        startUtf16: mountedCursor,
        lengthUtf16: piece.text.length,
        ...(piece.source ? { source: piece.source } : {}),
      });
    }
    mountedCursor += piece.text.length;
  }

  // Every identifier can answer hover at its own cursor position. The LSP
  // selects the candidate that contains the pointer; the explicit target above
  // remains the default for diagnostics and the v1-style single hover.
  const hoverTargets: Array<{ offset: number; source: SourceByteRange }> = [];
  const seenHoverTargets = new Set<number>();
  for (const token of scanIdentifiers(expr).sort((left, right) => left.start - right.start)) {
    if (seenHoverTargets.has(token.start)) continue;
    const offset = mapSourceOffset(token.start);
    const source = byteRangeOf(expr, token.start, token.end - token.start);
    if (offset === undefined || source === undefined) continue;
    seenHoverTargets.add(token.start);
    hoverTargets.push({ offset, source });
    if (hoverTargets.length >= MAX_HOVER_TARGETS) break;
  }

  return {
    text,
    segments,
    targetOffset,
    hoverTargets,
    roots,
    localRoot: roots.length > 0 && roots.every((root) => options.locals.has(root)),
  };
}

/** Resolve an explicit target or the rightmost value identifier (unique offset). */
function resolveTarget(expr: string, target: RewriteTarget): IdentifierToken | undefined {
  if (target === undefined) return rightmostIdentifierToken(expr);
  if (typeof target === "string") {
    const start = identifierOffset(expr, target);
    if (start < 0) return undefined;
    return { start, end: start + target.length, name: target };
  }
  if (
    target.start < 0 ||
    target.end > expr.length ||
    target.end <= target.start ||
    expr.slice(target.start, target.end) !== target.name
  ) {
    return undefined;
  }
  return target;
}

const ASSIGNMENT_OPERATORS = new Set<ts.SyntaxKind>([
  ts.SyntaxKind.EqualsToken,
  ts.SyntaxKind.PlusEqualsToken,
  ts.SyntaxKind.MinusEqualsToken,
  ts.SyntaxKind.AsteriskEqualsToken,
  ts.SyntaxKind.AsteriskAsteriskEqualsToken,
  ts.SyntaxKind.SlashEqualsToken,
  ts.SyntaxKind.PercentEqualsToken,
  ts.SyntaxKind.LessThanLessThanEqualsToken,
  ts.SyntaxKind.GreaterThanGreaterThanEqualsToken,
  ts.SyntaxKind.GreaterThanGreaterThanGreaterThanEqualsToken,
  ts.SyntaxKind.AmpersandEqualsToken,
  ts.SyntaxKind.BarEqualsToken,
  ts.SyntaxKind.CaretEqualsToken,
  ts.SyntaxKind.BarBarEqualsToken,
  ts.SyntaxKind.AmpersandAmpersandEqualsToken,
  ts.SyntaxKind.QuestionQuestionEqualsToken,
]);

function isAssignmentOperator(kind: ts.SyntaxKind): boolean {
  return ASSIGNMENT_OPERATORS.has(kind);
}
