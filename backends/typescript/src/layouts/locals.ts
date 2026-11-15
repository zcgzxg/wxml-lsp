/** Template-local bindings are unknown to Node; C1.1 binds them as `any`. */

import type { Piece } from "../text.ts";

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
 * Declarations for template locals.
 *
 * C1.1 only carries local *names*; their types belong to template semantics
 * (P6). Binding them as `any` keeps the expression mountable without inventing a
 * concrete type, and `TypeQuality::Any` reports that honestly.
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
