import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { BACKEND_DIR } from "../src/env.ts";
import { assertEvidenceDir, EVIDENCE_RELATIVE_DIR, sanitizeEvidence, EvidencePathError } from "../src/evidence.ts";

const repoRoot = path.resolve(BACKEND_DIR, "../..");
const fullCheckout = fs.existsSync(path.join(repoRoot, "docs", "reference", "type-bridge-c1.md"));

test("sanitizer replaces POSIX and Windows path forms", () => {
  const backend = path.join(repoRoot, "backends", "typescript");
  const value = {
    posix: `${backend}/samples/page-basic/index.ts`,
    windows: `${backend.replaceAll("/", "\\")}\\samples\\page-basic\\index.ts`,
    repo: `${repoRoot}/docs/evidence/type-bridge-c1.md`,
  };
  const clean = sanitizeEvidence(value, { backendDir: backend, repoRoot }) as typeof value;
  assert.equal(clean.posix, "<backend-root>/samples/page-basic/index.ts");
  assert.equal(clean.windows, "<backend-root>\\samples\\page-basic\\index.ts");
  assert.equal(clean.repo, "<repo-root>/docs/evidence/type-bridge-c1.md");
  assert.ok(!JSON.stringify(clean).includes("home/"));
});

test("evidence output must stay inside the repository layout", (context) => {
  if (!fullCheckout) {
    context.skip("docs tree is not part of this checkout");
    return;
  }
  assert.doesNotThrow(() => assertEvidenceDir(path.join(repoRoot, EVIDENCE_RELATIVE_DIR), repoRoot));
  assert.throws(
    () => assertEvidenceDir("/tmp/docs/evidence/data/type-bridge-c1", repoRoot),
    (error: unknown) => error instanceof EvidencePathError,
  );
  assert.throws(
    () => assertEvidenceDir(path.join(repoRoot, "docs", "elsewhere"), repoRoot),
    (error: unknown) => error instanceof EvidencePathError,
  );
  assert.throws(
    () => assertEvidenceDir(repoRoot, repoRoot),
    (error: unknown) => error instanceof EvidencePathError,
  );
  // A copied tree without the project markers is refused.
  assert.throws(
    () => assertEvidenceDir(path.resolve(BACKEND_DIR, "../../../tmp-copy/docs/evidence/data/type-bridge-c1"), path.resolve(BACKEND_DIR, "../../../tmp-copy")),
    (error: unknown) => error instanceof EvidencePathError,
  );
});

test("the committed evidence contains no absolute paths", (context) => {
  const matrixPath = path.join(repoRoot, EVIDENCE_RELATIVE_DIR, "matrix.json");
  if (!fs.existsSync(matrixPath)) {
    // A backend-only checkout does not carry the evidence data.
    context.skip("docs/evidence/data/type-bridge-c1/matrix.json is not part of this checkout");
    return;
  }
  const text = fs.readFileSync(matrixPath, "utf8");
  assert.ok(text.includes("<backend-root>"), "matrix.json must be sanitized");
  assert.ok(!text.includes(repoRoot), "matrix.json must not contain the repository root");
  const data = JSON.parse(text) as { runs: Array<{ scenario: string }> };
  assert.ok(data.runs.length >= 40);
});
