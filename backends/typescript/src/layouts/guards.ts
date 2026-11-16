/**
 * Conditional narrowing blocks (`wx:if` / `wx:elif` / `wx:else`).
 *
 * Turning a conditional chain into real `if`/`else if`/`else` statements lets
 * TypeScript's own control-flow analysis narrow the mounted expression: a
 * discriminated union narrows on its discriminant, and a truthiness test removes
 * `null`/`undefined`. Only the branches above the selected one are rendered, so a
 * fragment in the `wx:else` branch is checked with every previous condition false
 * — never with a later condition that does not apply to it.
 */

import type { RequestGuardChain } from "../protocol.ts";

export interface GuardRenderOptions {
  /** Indent of the outermost `if` line. */
  indent: string;
  /**
   * Rewrite a condition expression; `undefined` when it cannot be rewritten
   * safely. An unresolved condition never narrows and never fabricates a check.
   */
  rewriteCondition: (exprText: string) => string | undefined;
}

export interface RenderedGuards {
  /** `if (...) {` plus `} else if (...) {` lines; empty when unresolved. */
  open: string;
  /** The matching closing brace; empty when unresolved. */
  close: string;
  /** True when a condition could not be rendered. */
  unresolved: boolean;
}

/**
 * Render one conditional chain up to (and including) the selected branch.
 *
 * `selected < conditions.length` selects that branch; `selected ===
 * conditions.length` means the `else` branch, rendered as a final `} else {`.
 * Rendering stops at the selected branch: a later condition must not narrow the
 * fragment.
 */
export function renderGuards(chain: RequestGuardChain, options: GuardRenderOptions): RenderedGuards {
  const conditions = chain.conditions;
  if (conditions.length === 0) return { open: "", close: "", unresolved: false };
  const selected = Math.max(0, Math.min(chain.selected, conditions.length));
  const last = selected < conditions.length ? selected : conditions.length - 1;
  const lines: string[] = [];
  let terminated = false;

  for (let index = 0; index <= last; index += 1) {
    const condition = conditions[index]!;
    if (condition.branch === "else") {
      // An `else` inside the condition list terminates the chain; it never has a
      // condition to check.
      lines.push(index === 0 ? `${options.indent}if (true) {` : `${options.indent}} else {`);
      terminated = true;
      break;
    }
    const rewritten = options.rewriteCondition(condition.exprText);
    // TypeScript's control-flow analysis cannot prove every legal `||` guard
    // (a discriminated union behind `a !== 'x' || a.b` is a false positive in
    // the narrowed branch). Do not publish diagnostics under a narrowing we
    // cannot justify: the fragment stays mounted without a guard and the view
    // is reported Partial.
    if (rewritten === undefined || containsLogicalOr(rewritten)) {
      return { open: "", close: "", unresolved: true };
    }
    lines.push(`${options.indent}${index === 0 ? "if" : "} else if"} (${rewritten}) {`);
  }

  if (selected === conditions.length && !terminated) lines.push(`${options.indent}} else {`);
  return { open: `${lines.join("\n")}\n`, close: `${options.indent}}\n`, unresolved: false };
}

/** True when `text` contains `||` outside strings and comments. */
function containsLogicalOr(text: string): boolean {
  let quote: string | undefined;
  let index = 0;
  while (index < text.length) {
    const character = text[index]!;
    if (quote !== undefined) {
      if (character === "\\") {
        index += 2;
        continue;
      }
      if (character === quote) quote = undefined;
      index += 1;
      continue;
    }
    if (character === "'" || character === '"' || character === "`") {
      quote = character;
      index += 1;
      continue;
    }
    if (character === "/" && text[index + 1] === "/") {
      const end = text.indexOf("\n", index);
      if (end < 0) return false;
      index = end + 1;
      continue;
    }
    if (character === "/" && text[index + 1] === "*") {
      const end = text.indexOf("*/", index + 2);
      if (end < 0) return false;
      index = end + 2;
      continue;
    }
    if (character === "|" && text[index + 1] === "|") return true;
    index += 1;
  }
  return false;
}
