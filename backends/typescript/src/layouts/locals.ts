/**
 * Typed `wx:for` locals.
 *
 * The request carries the ordered binding chain of every fragment (`item`
 * optionally followed by `index`, per nesting level). Node turns that chain into
 * nested blocks whose declarations reuse the `wx:for` source expression, so
 * TypeScript itself derives the element/key type instead of Node guessing it:
 *
 *     {
 *       const item = __wxml_item(__wxml_root.groups);
 *       const index = __wxml_index(__wxml_root.groups);
 *       ...
 *     }
 *
 * The `__wxml_*` helper types keep `any` honest (an `any` source stays `any`
 * instead of collapsing to `never`) and map tuples, readonly arrays and object
 * maps (`Record<K, V>` -> value type / `string` key) without a special case per
 * shape. JavaScript projects get the old `any` declarations because helper
 * types are not available there.
 */

import ts from "typescript";
import { isBindingIdentifier, parseExpression } from "../expression.ts";
import type { RequestLocalBinding } from "../protocol.ts";
import type { Piece } from "../text.ts";
import type { FragmentDraft } from "../types.ts";
import { renderGuards, type RenderedGuards } from "./guards.ts";

/**
 * One-time helper block for typed locals (TypeScript only).
 *
 * `__WxmlIsAny` distinguishes `any` from every other type: without it
 * `__WxmlItem<any>` would distribute to `never`. A non-array object is treated
 * as a map (`item` = value union, `index` = string key).
 */
export const LOCAL_HELPERS_TS = [
  "type __WxmlIsAny<T> = 0 extends (1 & T) ? true : false;",
  "type __WxmlItem<T> = __WxmlIsAny<T> extends true ? any : T extends readonly (infer U)[] ? U : T extends object ? T[keyof T] : any;",
  "type __WxmlIndex<T> = __WxmlIsAny<T> extends true ? any : T extends readonly unknown[] ? number : T extends object ? Extract<keyof T, string> : number;",
  "function __wxml_item<T>(value: T): __WxmlItem<T> { return undefined as unknown as __WxmlItem<T>; }",
  "function __wxml_index<T>(value: T): __WxmlIndex<T> { return undefined as unknown as __WxmlIndex<T>; }",
].join("\n");

/** Helper block for the given syntax; JavaScript has no helper types. */
export function localHelpers(syntax: "ts" | "js"): string {
  return syntax === "js" ? "" : LOCAL_HELPERS_TS;
}

/** Prefix every line of `text` with `indent` (no trailing newline added). */
export function indentLines(text: string, indent: string): string {
  return text
    .split("\n")
    .map((line) => `${indent}${line}`)
    .join("\n");
}

/** Unique local names in request order. */
export function uniqueLocals(locals: readonly string[][]): string[] {
  const seen = new Set<string>();
  const names: string[] = [];
  for (const group of locals) {
    for (const name of group) {
      if (seen.has(name)) continue;
      seen.add(name);
      names.push(name);
    }
  }
  return names;
}

/**
 * Declarations for template locals whose type is unknown.
 *
 * Used when the request only carries the compatibility `locals` name list (no
 * structured binding, so no source expression to derive a type from) and as the
 * fallback for a binding that cannot be rewritten. Binding them as `any` keeps
 * the expression mountable without inventing a concrete type.
 */
export function localDeclarations(names: readonly string[], indent: string, syntax: "ts" | "js"): Piece[] {
  const pieces: Piece[] = [];
  for (const name of names) {
    pieces.push({
      kind: "text",
      text:
        syntax === "ts"
          ? `${indent}const ${name} = undefined as any;\n`
          : `${indent}const ${name} = /** @type {any} */ (undefined);\n`,
    });
  }
  return pieces;
}

/**
 * Rewrite a `wx:for` source expression (or a guard condition) so it is valid in
 * the generated scope.
 *
 * `outerNames` are the template locals already in scope; every other free root
 * gets the data-root prefix. `undefined` means the expression cannot be
 * rewritten safely (unparseable or an unsupported shape) — the caller falls back
 * to `any`/no narrowing instead of guessing.
 */
export type RewriteSource = (sourceExpr: string, outerNames: ReadonlySet<string>) => string | undefined;

/** Rendered locals: opening block text, the matching closers, and coverage. */
export interface RenderedLocals {
  /** Opening `{` + declarations, indented; empty when there are no levels. */
  text: string;
  /** Closing braces, innermost first; empty when there are no levels. */
  close: string;
  /** True when some declaration fell back to `any`. */
  unresolved: boolean;
}

/** One `wx:for` nesting level: an item, optionally followed by an index. */
export interface LocalLevel {
  item?: RequestLocalBinding;
  index?: RequestLocalBinding;
}
/**
 * Group structured bindings into `wx:for` nesting levels, by array order.
 *
 * A new level starts at every `item` binding; an `index` attaches to the level it
 * follows. Grouping deliberately does *not* compare `sourceExpr` values: nesting
 * order is the only thing the array encodes, and two levels may iterate the same
 * expression (nested repeats of one list are legal WXML).
 */
export function groupLocalLevels(bindings: readonly RequestLocalBinding[]): LocalLevel[] {
  const levels: LocalLevel[] = [];
  for (const binding of bindings) {
    if (!isBindingIdentifier(binding.name)) continue; // defensive: the validator enforces this
    if (binding.role !== "item" && binding.role !== "index") continue; // defensive: same
    const current = levels[levels.length - 1];
    if (binding.role === "item") {
      if (current && current.item === undefined) {
        if (current.index?.name !== binding.name) current.item = binding;
        continue;
      }
      levels.push({ item: binding });
      continue;
    }
    if (!current) {
      levels.push({ index: binding });
      continue;
    }
    // One index per level, and never two declarations with the same name.
    if (current.index !== undefined || current.item?.name === binding.name) continue;
    current.index = binding;
  }
  return levels;
}

const CAPTURE_PREFIX = "__wxml_capture_";

/** Alias for an outer binding that a deeper `wx:for` source expression reads. */
function captureName(name: string, outerLevel: number): string {
  return `${CAPTURE_PREFIX}${name}_${outerLevel}`;
}

/**
 * Render the nested `wx:for` blocks of one fragment.
 *
 * Every level declares `<item>` (and `<index>` when the request carries one) from
 * its own source expression. The source is rewritten with **outer** names only,
 * so a `wx:for` expression never sees the binding it is about to declare. When a
 * deeper level shadows an outer name that its source expression still reads
 * (`wx:for="{{ item.children }}"` with `wx:for-item="item"`), the outer value is
 * captured in the enclosing block and the source is renamed to the alias: the
 * declaration would otherwise hit the temporal dead zone.
 */
export function renderLocals(
  bindings: readonly RequestLocalBinding[],
  indent: string,
  syntax: "ts" | "js",
  rewriteSource: RewriteSource,
): RenderedLocals {
  const levels = groupLocalLevels(bindings);
  if (levels.length === 0) return { text: "", close: "", unresolved: false };

  const lines: string[] = [];
  const closers: string[] = [];
  const declared: Array<Set<string>> = [];
  let unresolved = false;

  for (let depth = 0; depth < levels.length; depth += 1) {
    const level = levels[depth]!;
    const levelIndent = `${indent}${"  ".repeat(depth)}`;
    const bodyIndent = `${levelIndent}  `;
    const outerNames = new Set<string>();
    for (let outer = 0; outer < depth; outer += 1) {
      for (const name of declared[outer]!) outerNames.add(name);
    }
    const ownNames = new Set<string>();
    if (level.item) ownNames.add(level.item.name);
    if (level.index) ownNames.add(level.index.name);
    declared.push(ownNames);

    const sourceExpr = level.item?.sourceExpr ?? level.index?.sourceExpr ?? "";
    let sourceText: string | undefined;
    let levelUnresolved = false;
    const captureLines: string[] = [];

    if (syntax === "js") {
      // A JavaScript project has no helper types, so every binding is `any`.
      // That is a coverage gap, not a clean check: report it as unresolved.
      sourceText = undefined;
      levelUnresolved = true;
    } else {
      // Capture outer values that this level's source reads but will shadow.
      const referenced = valueIdentifierNames(sourceExpr);
      const renames = new Map<string, string>();
      for (const name of ownNames) {
        if (!referenced?.has(name)) continue;
        const outerLevel = nearestOuterLevel(declared, depth, name);
        if (outerLevel === undefined) continue; // resolves to the data root, no shadow
        const alias = captureName(name, outerLevel);
        renames.set(name, alias);
        outerNames.add(alias);
        captureLines.push(`const ${alias} = ${name};`);
      }
      let prepared = sourceExpr;
      if (renames.size > 0) {
        const renamed = renameValueIdentifiers(sourceExpr, renames);
        if (renamed === undefined) levelUnresolved = true;
        else prepared = renamed;
      }
      if (!levelUnresolved) {
        sourceText = rewriteSource(prepared, outerNames);
        if (sourceText === undefined) levelUnresolved = true;
      }
      // Belt and braces: the emitted source must not read a name declared below.
      if (sourceText !== undefined) {
        const leftover = valueIdentifierNames(sourceText);
        if (leftover && [...ownNames].some((name) => leftover.has(name))) {
          sourceText = undefined;
          levelUnresolved = true;
        }
      }
    }
    if (levelUnresolved) unresolved = true;

    if (captureLines.length > 0) {
      for (const line of captureLines) lines.push(`${levelIndent}${line}`);
    }
    lines.push(`${levelIndent}{`);
    const fallback = syntax === "js" ? "undefined" : "undefined as any";
    if (level.item) {
      lines.push(`${bodyIndent}${declaration(syntax, level.item.name, "item", sourceText ?? fallback)}`);
    }
    if (level.index) {
      lines.push(`${bodyIndent}${declaration(syntax, level.index.name, "index", sourceText ?? fallback)}`);
    }
    closers.unshift(`${levelIndent}}`);
  }

  return { text: `${lines.join("\n")}\n`, close: `${closers.join("\n")}\n`, unresolved };
}

function declaration(syntax: "ts" | "js", name: string, role: "item" | "index", source: string): string {
  if (syntax === "js") return `const ${name} = /** @type {any} */ (undefined);`;
  return `const ${name} = __wxml_${role}(${source});`;
}

/** Innermost outer level that declares `name`, if any. */
function nearestOuterLevel(declared: readonly Set<string>[], depth: number, name: string): number | undefined {
  for (let outer = depth - 1; outer >= 0; outer -= 1) {
    if (declared[outer]!.has(name)) return outer;
  }
  return undefined;
}

/* -------------------------------------------------------- identifier walking */

interface ValueIdentifier {
  start: number;
  end: number;
  name: string;
  /** The token is the key *and* value of a shorthand property assignment. */
  shorthand: boolean;
}

/** Hard limits for untrusted expressions (mirrors `rewrite.ts`). */
const MAX_VALUE_DEPTH = 256;
const MAX_VALUE_NODES = 10_000;

/**
 * Value-position identifiers of a WXML expression.
 *
 * Mirrors the rewriter's rules: property names, object keys, strings, comments
 * and keywords are not free identifiers. `undefined` means the expression could
 * not be parsed, has a shape the rewriter rejects anyway, or exceeds the depth
 * or node budget (the walk must never throw a RangeError).
 */
function valueIdentifiers(expr: string): ValueIdentifier[] | undefined {
  const parsed = parseExpression(expr);
  if (!parsed) return undefined;
  const found: ValueIdentifier[] = [];
  let unsupported = false;
  let nodes = 0;

  const push = (node: ts.Identifier, shorthand: boolean): void => {
    found.push({
      start: node.getStart(parsed.source) - parsed.exprStart,
      end: node.getEnd() - parsed.exprStart,
      name: node.text,
      shorthand,
    });
  };

  const visit = (node: ts.Node, depth: number): void => {
    if (unsupported) return;
    nodes += 1;
    if (depth > MAX_VALUE_DEPTH || nodes > MAX_VALUE_NODES) {
      unsupported = true;
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
      unsupported = true;
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
      push(node.name, true);
      return;
    }
    if (ts.isMethodDeclaration(node) || ts.isGetAccessor(node) || ts.isSetAccessor(node)) {
      unsupported = true;
      return;
    }
    if (ts.isIdentifier(node)) {
      push(node, false);
      return;
    }
    node.forEachChild((child) => visit(child, depth + 1));
  };
  try {
    visit(parsed.root, 0);
  } catch {
    // A pathological input must degrade to `any`, never fail the analysis.
    return undefined;
  }
  return unsupported ? undefined : found;
}

/** Names of the value identifiers in `expr`, or `undefined` when unparseable. */
function valueIdentifierNames(expr: string): Set<string> | undefined {
  const identifiers = valueIdentifiers(expr);
  return identifiers === undefined ? undefined : new Set(identifiers.map((identifier) => identifier.name));
}

/**
 * Rename value identifiers in an expression, preserving every other byte.
 *
 * A shorthand property is expanded to `{ name: alias }` so the object key keeps
 * its meaning. `undefined` means the expression has an unsupported shape (the
 * caller then falls back to `any` rather than emitting dangerous text).
 */
function renameValueIdentifiers(expr: string, renames: ReadonlyMap<string, string>): string | undefined {
  const identifiers = valueIdentifiers(expr);
  if (!identifiers) return undefined;
  const edits = identifiers
    .filter((identifier) => renames.has(identifier.name))
    .sort((left, right) => left.start - right.start);
  if (edits.length === 0) return expr;
  let output = "";
  let cursor = 0;
  for (const edit of edits) {
    if (edit.start < cursor) return undefined;
    const alias = renames.get(edit.name)!;
    output += expr.slice(cursor, edit.start);
    output += edit.shorthand ? `${edit.name}: ${alias}` : alias;
    cursor = edit.end;
  }
  output += expr.slice(cursor);
  return output;
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

/* --------------------------------------------------------- fragment composition */

export interface FragmentScopeOptions {
  /** Indent of the fragment's outer wrapper block. */
  indent: string;
  syntax: "ts" | "js";
  /** Rewriter for `wx:for` source expressions (outer locals only). */
  rewriteSource: RewriteSource;
  /** Rewriter for `wx:if`/`wx:elif` conditions (all locals in scope). */
  rewriteCondition: (exprText: string) => string | undefined;
}

/** Opening/closing text that puts a mount inside its loops and guards. */
export interface RenderedFragmentScope {
  /** Wrapper block, nested loops, guard openings and any `@ts-ignore` line. */
  open: string;
  /** Indent at which the caller emits the mount expression statement. */
  mountIndent: string;
  /** Guard closers, loop closers and the wrapper closer. */
  close: string;
  /** Bindings that fell back to `any`. */
  unresolvedBindings: number;
  /** Guards that could not narrow. */
  unresolvedGuards: number;
}

/**
 * Compose the generated scope of one fragment: a private block per fragment (so
 * two fragments may declare the same local name), nested `wx:for` levels, then
 * the nested conditional chains. The caller mounts the expression right after
 * `open` at `mountIndent` and closes with `close`.
 */
export function renderFragmentScope(draft: FragmentDraft, options: FragmentScopeOptions): RenderedFragmentScope {
  const { indent, syntax } = options;
  const fragmentIndent = `${indent}  `;
  const openLine = `${indent}{`;
  let unresolvedBindings = 0;

  let localText = "";
  let localClose = "";
  let depth = 0;
  if (draft.bindings.length > 0) {
    const locals = renderLocals(draft.bindings, fragmentIndent, syntax, options.rewriteSource);
    localText = locals.text;
    localClose = locals.close;
    depth = groupLocalLevels(draft.bindings).length;
    if (locals.unresolved) unresolvedBindings += draft.bindings.length;
  } else if (draft.locals.length > 0) {
    // Compatibility path: only names arrived, so there is no source to type.
    for (const piece of localDeclarations(uniqueLocals([draft.locals]), fragmentIndent, syntax)) {
      if (piece.kind === "text") localText += piece.text;
    }
    unresolvedBindings += draft.locals.length;
  }

  const guardBase = `${fragmentIndent}${"  ".repeat(depth)}`;
  const guards: RenderedGuards[] = [];
  let unresolvedGuards = 0;
  // Indent every chain after the guards that actually opened a block, not after
  // its index: an unresolved chain renders nothing and must not shift the mount.
  let openedGuards = 0;
  for (const chain of draft.guards) {
    const rendered = renderGuards(chain, {
      indent: `${guardBase}${"  ".repeat(openedGuards)}`,
      rewriteCondition: options.rewriteCondition,
    });
    if (rendered.unresolved) unresolvedGuards += 1;
    if (rendered.open.length > 0) openedGuards += 1;
    guards.push(rendered);
  }
  const mountIndent = `${guardBase}${"  ".repeat(openedGuards)}`;
  const unresolved = unresolvedBindings > 0 || unresolvedGuards > 0;

  return {
    open:
      `${openLine}\n${localText}` +
      guards.map((guard) => guard.open).join("") +
      // `@ts-ignore` only covers the next line; a mounted expression that spans
      // several lines can still surface an error. WXML expressions are normally
      // single-line, so this is left as-is rather than emitting N ignores.
      (unresolved ? `${mountIndent}// @ts-ignore\n` : ""),
    mountIndent,
    close:
      [...guards]
        .reverse()
        .map((guard) => guard.close)
        .join("") +
      localClose +
      `${indent}}\n`,
    unresolvedBindings,
    unresolvedGuards,
  };
}
