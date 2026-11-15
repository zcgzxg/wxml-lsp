/** Expression-level helpers shared by the protocol validator and the layout planner. */

import ts from "typescript";

/** Hard limits for untrusted expressions (WXML input is not a TypeScript file). */
const MAX_AST_DEPTH = 256;
const MAX_AST_NODES = 10_000;

/**
 * First identifier of an expression (`user.name` -> `user`).
 *
 * Returns the whole expression when it does not start with an identifier.
 */
export function rootIdentifier(expr: string): string {
  const match = /^[\p{L}_$][\p{L}\p{N}_$]*/u.exec(expr);
  return match ? match[0] : expr;
}

/** Keep diagnostic messages bounded; raw WXML expressions can be MiB-sized. */
export function describeExpression(expr: string, limit = 120): string {
  if (expr.length <= limit) return expr;
  return `${expr.slice(0, limit)}… (total ${expr.length} UTF-16 chars)`;
}

export interface ParsedExpression {
  source: ts.SourceFile;
  initializer: ts.ParenthesizedExpression;
  root: ts.Expression;
  /** Offset of `expr` inside the synthetic `const __wxml_expr = (...)` text. */
  exprStart: number;
}

/**
 * Parse an expression inside the same wrapper the rewriter uses.
 *
 * `undefined` means the input is not exactly one parenthesized expression:
 * parse errors, extra statements, missing parens, or trailing text all fail
 * closed before any generated code is accepted.
 */
export function parseExpression(expr: string): ParsedExpression | undefined {
  let source: ts.SourceFile;
  try {
    source = ts.createSourceFile(
      "expression.ts",
      `const __wxml_expr = (${expr});`,
      ts.ScriptTarget.ES2022,
      true,
      ts.ScriptKind.TS,
    );
  } catch {
    return undefined;
  }
  const parseDiagnostics = (source as unknown as { parseDiagnostics?: readonly ts.Diagnostic[] }).parseDiagnostics ?? [];
  if (parseDiagnostics.length > 0) return undefined;
  if (source.statements.length !== 1) return undefined;
  const statement = source.statements[0];
  if (!statement || !ts.isVariableStatement(statement) || statement.declarationList.declarations.length !== 1) {
    return undefined;
  }
  const initializer = statement.declarationList.declarations[0]?.initializer;
  if (!initializer || !ts.isParenthesizedExpression(initializer)) return undefined;
  const exprStart = initializer.getStart(source) + 1;
  const root = initializer.expression;
  // The wrapper's closing paren must immediately follow the parsed expression
  // and the parsed expression must consume every UTF-16 code unit of `expr`.
  if (initializer.getEnd() !== root.getEnd() + 1) return undefined;
  if (root.getEnd() - exprStart !== expr.length) return undefined;
  return { source, initializer, root, exprStart };
}

export interface IdentifierToken {
  start: number;
  end: number;
  name: string;
}

/**
 * Identifiers in source order.
 *
 * Parsed as a TypeScript expression, so identifiers inside strings, template
 * text, comments and numeric literals are never returned; template
 * substitutions are. A lexical fallback covers expressions the parser rejects
 * (the rewriter reports those as `bad-fragment` with a better message).
 */
export function scanIdentifiers(expr: string): IdentifierToken[] {
  const parsed = parseExpression(expr);
  if (!parsed) return lexicalIdentifiers(expr);
  const found: IdentifierToken[] = [];
  let nodes = 0;
  let overflow = false;
  const visit = (node: ts.Node, depth: number): void => {
    if (overflow) return;
    nodes += 1;
    if (depth > MAX_AST_DEPTH || nodes > MAX_AST_NODES) {
      overflow = true;
      return;
    }
    if (ts.isIdentifier(node)) {
      found.push({
        start: node.getStart(parsed.source) - parsed.exprStart,
        end: node.getEnd() - parsed.exprStart,
        name: node.text,
      });
    }
    node.forEachChild((child) => visit(child, depth + 1));
  };
  visit(parsed.root, 0);
  return overflow ? lexicalIdentifiers(expr) : found;
}

/** Lexical fallback; skips strings, comments and numeric literals. */
function lexicalIdentifiers(expr: string): IdentifierToken[] {
  const found: IdentifierToken[] = [];
  let index = 0;
  while (index < expr.length) {
    const char = expr[index]!;
    if (char === "/" && expr[index + 1] === "/") {
      const newline = expr.indexOf("\n", index);
      index = newline < 0 ? expr.length : newline + 1;
      continue;
    }
    if (char === "/" && expr[index + 1] === "*") {
      const close = expr.indexOf("*/", index + 2);
      index = close < 0 ? expr.length : close + 2;
      continue;
    }
    if (char === '"' || char === "'" || char === "`") {
      const quote = char;
      index += 1;
      while (index < expr.length) {
        if (expr[index] === "\\") {
          index += 2;
          continue;
        }
        if (expr[index] === quote) {
          index += 1;
          break;
        }
        index += 1;
      }
      continue;
    }
    const decimalPoint = char === "." && expr[index + 1] !== undefined && /[0-9]/.test(expr[index + 1]!);
    if ((char >= "0" && char <= "9") || decimalPoint) {
      index = skipNumber(expr, index);
      continue;
    }
    if (!/[\p{L}_$]/u.test(char)) {
      index += 1;
      continue;
    }
    const start = index;
    index += 1;
    while (index < expr.length && /[\p{L}\p{N}_$]/u.test(expr[index]!)) index += 1;
    found.push({ start, end: index, name: expr.slice(start, index) });
  }
  return found;
}

/**
 * Offset of `target` as a whole identifier outside strings/comments.
 *
 * A plain `indexOf` would pick the substring inside `"name"`, a comment or a
 * longer identifier; the C1 contract requires the user token.
 */
export function identifierOffset(expr: string, target: string): number {
  return scanIdentifiers(expr).find((entry) => entry.name === target)?.start ?? -1;
}

/**
 * Rightmost value identifier (`users[0].name` -> `name`).
 *
 * Object-literal keys are not values, so `{ title: user.name, flag: true }`
 * targets `name`, while a shorthand `{ title }` targets `title`. The returned
 * start/end offsets distinguish repeated names such as `a.a`.
 */
export function rightmostIdentifierToken(expr: string): IdentifierToken | undefined {
  const parsed = parseExpression(expr);
  if (!parsed) {
    const found = lexicalIdentifiers(expr);
    return found[found.length - 1];
  }
  let last: IdentifierToken | undefined;
  let nodes = 0;
  let overflow = false;
  const visit = (node: ts.Node, depth: number): void => {
    if (overflow) return;
    nodes += 1;
    if (depth > MAX_AST_DEPTH || nodes > MAX_AST_NODES) {
      overflow = true;
      return;
    }
    if (ts.isPropertyAssignment(node)) {
      if (ts.isComputedPropertyName(node.name)) visit(node.name.expression, depth + 1);
      visit(node.initializer, depth + 1);
      return;
    }
    if (ts.isShorthandPropertyAssignment(node)) {
      last = {
        start: node.name.getStart(parsed.source) - parsed.exprStart,
        end: node.name.getEnd() - parsed.exprStart,
        name: node.name.text,
      };
      return;
    }
    if (ts.isPropertyAccessExpression(node)) {
      visit(node.expression, depth + 1);
      last = {
        start: node.name.getStart(parsed.source) - parsed.exprStart,
        end: node.name.getEnd() - parsed.exprStart,
        name: node.name.text,
      };
      return;
    }
    if (ts.isIdentifier(node)) {
      last = {
        start: node.getStart(parsed.source) - parsed.exprStart,
        end: node.getEnd() - parsed.exprStart,
        name: node.text,
      };
      return;
    }
    node.forEachChild((child) => visit(child, depth + 1));
  };
  visit(parsed.root, 0);
  return overflow ? undefined : last;
}

/** Rightmost value identifier as text (compatibility helper). */
export function rightmostIdentifier(expr: string): string | undefined {
  return rightmostIdentifierToken(expr)?.name;
}

/** True when `name` is a valid ECMAScript identifier (simple, unicode-aware form). */
export function isIdentifier(name: string): boolean {
  return /^[\p{L}_$][\p{L}\p{N}_$]*$/u.test(name);
}

const RESERVED_WORDS = new Set([
  // ECMAScript keywords and strict-mode reserved words
  "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else",
  "enum", "export", "extends", "false", "finally", "for", "function", "if", "import", "in", "instanceof",
  "new", "null", "return", "super", "switch", "this", "throw", "true", "try", "typeof", "var", "void",
  "while", "with", "yield", "let", "static", "implements", "interface", "package", "private", "protected",
  "public", "await",
  // global values and bindings that must not be shadowed by a template local
  "eval", "arguments", "undefined", "NaN", "Infinity",
]);

/**
 * True when `name` can be used as a template-local binding.
 *
 * Rejects keywords/strict reserved words, `eval`/`arguments`, global value names
 * and the bridge-reserved `__wxml_*` / `__WxmlProps_*` prefixes.
 */
export function isBindingIdentifier(name: string): boolean {
  if (!isIdentifier(name)) return false;
  if (RESERVED_WORDS.has(name)) return false;
  if (name.startsWith("__wxml_") || name.startsWith("__WxmlProps_")) return false;
  return true;
}

/** Skip a numeric literal so `1e10`/`0x1f`/`123n` do not leak fake identifiers. */
function skipNumber(expr: string, start: number): number {
  let index = start;
  const radix = expr[index] === "0" ? expr[index + 1]?.toLowerCase() : undefined;
  if (radix === "x" || radix === "b" || radix === "o") {
    index += 2;
    const digit = radix === "x" ? /[0-9a-fA-F_]/ : radix === "b" ? /[01_]/ : /[0-7_]/;
    while (index < expr.length && digit.test(expr[index]!)) index += 1;
  } else {
    while (index < expr.length && /[0-9_]/.test(expr[index]!)) index += 1;
    if (expr[index] === ".") {
      index += 1;
      while (index < expr.length && /[0-9_]/.test(expr[index]!)) index += 1;
    }
    if (expr[index] === "e" || expr[index] === "E") {
      const exponentStart = index;
      index += 1;
      if (expr[index] === "+" || expr[index] === "-") index += 1;
      if (!/[0-9]/.test(expr[index] ?? "")) {
        index = exponentStart;
      } else {
        while (index < expr.length && /[0-9_]/.test(expr[index]!)) index += 1;
      }
    }
  }
  if (expr[index] === "n") index += 1;
  return index;
}
