/**
 * Evidence file hygiene: sanitize machine paths and keep writes inside the repo.
 *
 * The evidence emitter must be safe to run from a copied checkout: a stale
 * `BACKEND_DIR` must never make it write outside `docs/evidence/data/...`, and
 * no absolute path (POSIX or Windows) may reach the committed JSON.
 */

import fs from "node:fs";
import path from "node:path";

export const EVIDENCE_RELATIVE_DIR = path.join("docs", "evidence", "data", "type-bridge-c1");

export class EvidencePathError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "EvidencePathError";
  }
}

/**
 * Reject output directories outside `<repo>/docs/evidence/data/type-bridge-c1`.
 *
 * The repository root must also look like this project (contract + backend
 * manifest). A copied checkout that does not is refused, so a stale path can
 * never create `/tmp/docs/...`.
 */
export function assertEvidenceDir(outputDir: string, repoRoot: string): void {
  const resolvedOutput = path.resolve(outputDir);
  const resolvedRepo = path.resolve(repoRoot);
  const relative = path.relative(resolvedRepo, resolvedOutput);
  if (relative === "" || relative.startsWith("..") || path.isAbsolute(relative)) {
    throw new EvidencePathError(`evidence output must stay inside the repository: ${outputDir}`);
  }
  const markers = [
    path.join(resolvedRepo, "docs", "reference", "type-bridge-c1.md"),
    path.join(resolvedRepo, "backends", "typescript", "package.json"),
  ];
  if (markers.some((marker) => !fs.existsSync(marker))) {
    throw new EvidencePathError(`not a project checkout: ${repoRoot}`);
  }
  const expected = path.resolve(resolvedRepo, EVIDENCE_RELATIVE_DIR);
  if (resolvedOutput !== expected) {
    throw new EvidencePathError(`evidence output must be ${EVIDENCE_RELATIVE_DIR}: ${outputDir}`);
  }
}

function replaceAllForms(text: string, value: string, placeholder: string): string {
  const forms = new Set([value, value.split(path.sep).join("/"), value.split("/").join("\\")]);
  let output = text;
  for (const form of forms) {
    if (form.length === 0) continue;
    output = output.split(form).join(placeholder);
  }
  return output;
}

/** Replace absolute backend/repository roots (either separator style). */
export function sanitizeEvidence(
  value: unknown,
  roots: { backendDir: string; repoRoot: string },
): unknown {
  // Walk the data (not the JSON text) so Windows separators do not need escaping.
  const walk = (node: unknown): unknown => {
    if (typeof node === "string") {
      // Backend root first: it lives inside the repository root.
      return replaceAllForms(
        replaceAllForms(node, roots.backendDir, "<backend-root>"),
        roots.repoRoot,
        "<repo-root>",
      );
    }
    if (Array.isArray(node)) return node.map(walk);
    if (node !== null && typeof node === "object") {
      return Object.fromEntries(Object.entries(node).map(([key, child]) => [key, walk(child)]));
    }
    return node;
  };
  return walk(value);
}
