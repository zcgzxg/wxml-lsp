/**
 * Bounded TypeScript project pool.
 *
 * The pool is a performance optimization, so the tests pin the safety
 * properties: one project per exact shape, incremental text refresh without a
 * new language service, and a hard upper bound on retained projects.
 */

import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import { BACKEND_DIR } from "../src/env.ts";
import {
  acquireProject,
  baseCompilerOptions,
  defaultReadPolicy,
  MAX_POOLED_PROJECTS,
  projectPoolSize,
  resetProjectPool,
  type VirtualProjectInit,
} from "../src/compiler.ts";

const SCRIPT = path.join("/tmp/opencode/q2-compiler-test", "pages/index/index.ts");

function init(texts: string[], script = SCRIPT): VirtualProjectInit {
  return {
    rootDir: BACKEND_DIR,
    compilerOptions: baseCompilerOptions(),
    files: texts.map((text, index) => ({ path: `${script}.${index}.ts`, text })),
    readPolicy: defaultReadPolicy(BACKEND_DIR),
  };
}

test("the same shape reuses one project and refreshes text in place", { concurrency: false }, () => {
  resetProjectPool();
  const first = acquireProject(init(["const a: number = 1;"]));
  assert.equal(projectPoolSize(), 1);
  const service = first.languageService();
  assert.equal(first.text(`${SCRIPT}.0.ts`), "const a: number = 1;");

  const refreshed = acquireProject(init(["const a: string = 'x';"]));
  assert.equal(refreshed, first, "a text-only change keeps the project");
  assert.equal(refreshed.languageService(), service, "the language service stays warm");
  assert.equal(refreshed.text(`${SCRIPT}.0.ts`), "const a: string = 'x';");
  resetProjectPool();
  assert.equal(projectPoolSize(), 0);
});

test("a different file set or compiler option set gets a fresh project", { concurrency: false }, () => {
  resetProjectPool();
  const one = acquireProject(init(["const a = 1;"]));
  const two = acquireProject(init(["const a = 1;", "const b = 2;"]));
  assert.notEqual(two, one, "a file-set change must not leak the old program");

  const strict = init(["const a = 1;"]);
  strict.compilerOptions = { ...strict.compilerOptions, strict: false };
  const three = acquireProject(strict);
  assert.notEqual(three, one, "a compiler-option change must not reuse the old check");
  resetProjectPool();
});

test("the pool is bounded", { concurrency: false }, () => {
  resetProjectPool();
  for (let index = 0; index < MAX_POOLED_PROJECTS + 3; index += 1) {
    acquireProject(init([`const value${index} = ${index};`], path.join("/tmp/opencode/q2-compiler-test", `project-${index}`, "index.ts")));
  }
  assert.equal(projectPoolSize(), MAX_POOLED_PROJECTS);
  resetProjectPool();
});
