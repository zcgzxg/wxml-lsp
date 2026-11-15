import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import ts from "typescript";
import { BACKEND_DIR, installedVersions } from "../src/env.ts";
import { BACKEND_VERSION } from "../src/version.ts";

test("TypeScript and the typings package are pinned", () => {
  assert.equal(ts.version, "6.0.3");
  const installed = installedVersions();
  assert.equal(installed.typescript, "6.0.3");
  assert.equal(installed.typings, "5.3.1");

  const manifest = JSON.parse(fs.readFileSync(path.join(BACKEND_DIR, "package.json"), "utf8")) as {
    version: string;
    dependencies: Record<string, string>;
    devDependencies: Record<string, string>;
  };
  assert.equal(BACKEND_VERSION, manifest.version, "ping version must track package.json");
  // Runtime dependencies: the backend needs them after `npm ci --omit=dev`.
  assert.equal(manifest.dependencies.typescript, "6.0.3");
  assert.equal(manifest.dependencies["miniprogram-api-typings"], "5.3.1");
  assert.ok(manifest.devDependencies["@types/node"], "type definitions stay dev-only");
});

test("the C1 contract documents the pinned versions", (context) => {
  const contractPath = path.resolve(BACKEND_DIR, "../../docs/reference/type-bridge-c1.md");
  if (!fs.existsSync(contractPath)) {
    // A backend-only checkout (for example a copied harness) has no docs tree.
    context.skip("docs/reference/type-bridge-c1.md is not part of this checkout");
    return;
  }
  const contract = fs.readFileSync(contractPath, "utf8");
  assert.match(contract, /6\.0\.3/);
  assert.match(contract, /5\.3\.1/);
});
