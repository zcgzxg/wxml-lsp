/**
 * Pragma scanning and the probe-health gate.
 *
 * A bridge view may look healthy while the injected helper failed to type-check:
 * a duplicate `__wxml_root`, a missing typings package, or a probe that resolves
 * to `any` all produce "no diagnostics". The gate turns those into
 * `Partial`/`Unavailable` instead of `Supported`.
 */

import path from "node:path";
import ts from "typescript";
import type { VirtualProject } from "./compiler.ts";
import { hoverAt } from "./queries.ts";
import type { InsertionRecord, MountRecord } from "./types.ts";

export interface PragmaScan {
  /** Effective `@ts-nocheck` (the last pragma wins, as TypeScript does). */
  noCheck: boolean;
  /** Effective `@ts-check` (the last pragma wins). */
  check: boolean;
  bomLength: number;
  /** Offset right after a hashbang line (or after the BOM when none). */
  shebangEnd: number;
  /** Where an injected `// @ts-check` line must go (original coordinates). */
  injectionOffset: number;
  /**
   * Where a neutralising `// @ts-nocheck` must go: after the leading comment
   * block, because TypeScript honours the *last* pragma in that block.
   */
  neutralizeOffset: number;
  /** True when the BOM has to be dropped for the pragma to be honoured. */
  dropBom: boolean;
}

export function scanPragmas(text: string, wantPragma: boolean): PragmaScan {
  const bomLength = text.startsWith("\uFEFF") ? 1 : 0;
  let shebangEnd = bomLength;
  if (text.startsWith("#!", bomLength)) {
    const newline = text.indexOf("\n", bomLength);
    shebangEnd = newline < 0 ? text.length : newline + 1;
  }
  let lastPragma: "check" | "nocheck" | undefined;
  let cursor = shebangEnd;
  const recordPragmas = (comment: string): void => {
    const matches = [...comment.matchAll(/@ts-(check|nocheck)\b/g)];
    for (const match of matches) {
      lastPragma = match[1] === "check" ? "check" : "nocheck";
    }
  };
  for (;;) {
    while (cursor < text.length && /\s/.test(text[cursor]!)) cursor += 1;
    if (text.startsWith("//", cursor)) {
      const newline = text.indexOf("\n", cursor);
      const end = newline < 0 ? text.length : newline;
      recordPragmas(text.slice(cursor, end));
      cursor = end;
      continue;
    }
    if (text.startsWith("/*", cursor)) {
      const close = text.indexOf("*/", cursor);
      const end = close < 0 ? text.length : close + 2;
      recordPragmas(text.slice(cursor, end));
      cursor = end;
      continue;
    }
    break;
  }
  const dropBom = wantPragma && bomLength > 0;
  return {
    noCheck: lastPragma === "nocheck",
    check: lastPragma === "check",
    bomLength,
    shebangEnd,
    injectionOffset: shebangEnd,
    neutralizeOffset: cursor,
    dropBom,
  };
}

export interface ProbeHealth {
  problems: string[];
}

export interface ProbeHealthInput {
  project: VirtualProject;
  scriptPath: string;
  /** Exact offsets recorded by the layout; never searched for by name. */
  helperOffsets: Array<{ name: string; offset: number; file?: string }>;
  rootOffset?: number;
  insertions: InsertionRecord[];
  mounts: MountRecord[];
  /** Module classification before/after the derived view. */
  moduleBefore?: "script" | "module";
  moduleAfter?: "script" | "module";
  /** False when template checking is off (no diagnostics are expected). */
  checkDiagnostics?: boolean;
}

/** Post-build gate for candidate A and B. */
export function checkProbeHealth(input: ProbeHealthInput): ProbeHealth {
  const problems: string[] = [];
  const program = input.project.program();
  const sourceFile = program.getSourceFile(path.resolve(input.scriptPath));
  if (!sourceFile) {
    return { problems: ["derived script is not part of the program"] };
  }
  const checker = program.getTypeChecker();

  for (const helper of input.helperOffsets) {
    const targetFile = helper.file ? path.resolve(helper.file) : path.resolve(input.scriptPath);
    const targetSource = targetFile === path.resolve(input.scriptPath) ? sourceFile : program.getSourceFile(targetFile);
    if (!targetSource) {
      problems.push(`probe helper ${helper.name} lives in a file outside the program`);
      continue;
    }
    const node = findNodeAt(targetSource, helper.offset);
    const symbol = node ? checker.getSymbolAtLocation(node) : undefined;
    if (!symbol) problems.push(`probe helper ${helper.name} has no symbol`);
  }

  if (input.rootOffset !== undefined) {
    const hover = hoverAt(input.project, input.scriptPath, input.rootOffset);
    if (!hover) problems.push("probe root has no quick info");
    else if (hover.quality === "any" || hover.quality === "missing") {
      problems.push(`probe root resolves to ${hover.quality}`);
    }
  }

  if (input.moduleBefore !== undefined && input.moduleAfter !== undefined && input.moduleBefore !== input.moduleAfter) {
    problems.push(`module classification flipped from ${input.moduleBefore} to ${input.moduleAfter}`);
  }

  if (input.checkDiagnostics === false) return { problems };

  const diagnostics = input.project
    .languageService()
    .getSemanticDiagnostics(path.resolve(input.scriptPath));
  for (const diagnostic of diagnostics) {
    if (diagnostic.start === undefined) continue;
    const start = diagnostic.start;
    const end = start + (diagnostic.length ?? 0);
    const insideMount = input.mounts.some(
      (mount) =>
        path.resolve(mount.file) === path.resolve(input.scriptPath) &&
        start >= mount.startUtf16 &&
        start < mount.startUtf16 + mount.text.length,
    );
    if (insideMount) continue;
    const hit = input.insertions.find(
      (insertion) =>
        path.resolve(insertion.file) === path.resolve(input.scriptPath) &&
        start < insertion.startUtf16 + insertion.lengthUtf16 &&
        end > insertion.startUtf16,
    );
    if (hit) {
      problems.push(`diagnostic ${diagnostic.code} intersects insertion ${hit.insertionId}`);
    }
  }

  return { problems };
}

function findNodeAt(sourceFile: ts.SourceFile, offset: number): ts.Node | undefined {
  let best: ts.Node | undefined;
  const visit = (node: ts.Node): void => {
    if (offset >= node.getStart(sourceFile) && offset < node.getEnd()) {
      best = node;
      node.forEachChild(visit);
    }
  };
  visit(sourceFile);
  return best;
}
