/** Fragment planning: turn a WXML expression into one or more mounted TypeScript expressions. */

import { describeExpression, identifierOffset, rightmostIdentifierToken, rootIdentifier } from "./expression.ts";
import { RequestError } from "./protocol.ts";
import { rewriteExpression } from "./rewrite.ts";
import type { RequestFragment } from "./protocol.ts";
import type { FragmentDraft } from "./types.ts";

export const ROOT_PREFIX = "__wxml_root.";
export const PROPS_PREFIX = "__wxml_props_0.";

export interface FragmentPlanOptions {
  /** Names that must be read from the declaration-derived properties overlay. */
  propertyOverlay: ReadonlySet<string>;
  /** Root symbol prefix of this candidate (`__wxml_root.` / `__wxml_root_0.`). */
  rootPrefix: string;
  /** Prefix of the declaration-derived properties overlay. */
  propsPrefix?: string;
}

export { identifierOffset, rootIdentifier };

/**
 * Plan one mount per request fragment.
 *
 * The mounted set is exactly `request.fragments`. C1.1 adds two optional fields:
 * `target` (whole identifier to hover/diagnose, defaulting to the rightmost
 * identifier) and `locals` (template scope bindings whose roots get no
 * `__wxml_root.` prefix).
 */
export function planFragments(requestFragments: RequestFragment[], options: FragmentPlanOptions): FragmentDraft[] {
  const propsPrefix = options.propsPrefix ?? PROPS_PREFIX;
  return requestFragments.map((fragment) => {
    const expr = fragment.exprText;
    const fragmentId = fragment.fragmentId;
    const locals = fragment.locals ?? [];
    const defaultTarget = fragment.target === undefined ? rightmostIdentifierToken(expr) : undefined;
    if (fragment.target === undefined && defaultTarget === undefined) {
      throw new RequestError(
        "bad-fragment",
        `fragment ${fragmentId}: no identifier to target in ${describeExpression(expr)}`,
      );
    }
    const target = fragment.target ?? defaultTarget!.name;
    const rewritten = rewriteExpression(expr, fragment.target ?? defaultTarget, {
      rootPrefix: options.rootPrefix,
      propsPrefix,
      propertyOverlay: options.propertyOverlay,
      locals: new Set(locals),
    });
    const segments = rewritten.segments.map((segment) => ({
      ...segment,
      ...(segment.source
        ? {
            source: {
              start: fragment.wxmlRange.start + segment.source.start,
              end: fragment.wxmlRange.start + segment.source.end,
            },
          }
        : {}),
    }));
    return {
      fragmentId,
      expr,
      mountedText: rewritten.text,
      target,
      targetOffset: rewritten.targetOffset,
      segments,
      source: { start: fragment.wxmlRange.start, end: fragment.wxmlRange.end },
      locals,
      root: rewritten.roots[0] ?? expr,
      localRoot: rewritten.localRoot,
    };
  });
}

/** Locate the single `{{ expr }}` occurrence of an expression. */
export function locateExpression(wxml: string, expr: string, scenarioId: string): { start: number } {
  const pattern = new RegExp(`\\{\\{[ \\t\\r\\n]*${escapeRegExp(expr)}[ \\t\\r\\n]*\\}\\}`, "g");
  const matches = [...wxml.matchAll(pattern)];
  if (matches.length === 0) {
    throw new Error(`scenario ${scenarioId}: expression not found in WXML: ${expr}`);
  }
  if (matches.length > 1) {
    throw new Error(`scenario ${scenarioId}: expression is not unique in WXML: ${expr}`);
  }
  const match = matches[0]!;
  return { start: match.index + match[0].indexOf(expr) };
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
