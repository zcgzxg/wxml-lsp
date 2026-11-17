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
import ts from "typescript";
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

test("nested compiler options such as paths are part of the key", { concurrency: false }, () => {
  resetProjectPool();
  const first = init(["import { x } from '@alias/x';"]);
  first.compilerOptions = {
    ...first.compilerOptions,
    baseUrl: ".",
    paths: { "@alias/*": ["src/first/*"] },
  };
  const projectA = acquireProject(first);

  const second = init(["import { x } from '@alias/x';"]);
  second.compilerOptions = {
    ...second.compilerOptions,
    baseUrl: ".",
    paths: { "@alias/*": ["src/second/*"] },
  };
  const projectB = acquireProject(second);
  assert.notEqual(projectB, projectA, "different paths must not reuse a project");
  assert.deepEqual(projectB.compilerOptions.paths, { "@alias/*": ["src/second/*"] });
  assert.deepEqual(projectA.compilerOptions.paths, { "@alias/*": ["src/first/*"] });
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

test("a path-only change resolves the new alias instead of the old project", { concurrency: false }, () => {
  resetProjectPool();
  const root = "/tmp/opencode/q2-compiler-test/paths";
  const index = path.join(root, "index.ts");
  const files = [
    { path: index, text: "import { value } from '@alias/y'; export const result: number = value;" },
    { path: path.join(root, "src/first/x.ts"), text: "export const value = 1;" },
    { path: path.join(root, "src/second/y.ts"), text: "export const value = 2;" },
  ];
  const first = init([], index);
  first.files = files;
  first.compilerOptions = {
    ...first.compilerOptions,
    baseUrl: root,
    paths: { "@alias/*": ["src/first/*"] },
  };
  const projectA = acquireProject(first);
  void projectA;

  const second = init([], index);
  second.files = files;
  second.compilerOptions = {
    ...second.compilerOptions,
    baseUrl: root,
    paths: { "@alias/*": ["src/second/*"] },
  };
  const projectB = acquireProject(second);
  assert.notEqual(projectB, projectA);
  const program = projectB.languageService().getProgram();
  assert.ok(program);
  const unresolved = ts
    .getPreEmitDiagnostics(program)
    .filter((diagnostic) => diagnostic.code === 2307);
  assert.deepEqual(unresolved, [], "the new paths alias must resolve");
  resetProjectPool();
});

test("project typeRoots still load the pinned mini program typings", { concurrency: false }, () => {
  resetProjectPool();
  const root = "/tmp/opencode/q2-compiler-test/type-roots";
  const index = path.join(root, "index.ts");
  const project = acquireProject({
    rootDir: BACKEND_DIR,
    compilerOptions: {
      ...baseCompilerOptions(),
      // A project-specific root replaces TypeScript's default @types lookup.
      // The backend cannot read this directory, so the pinned package must
      // stay reachable through the appended backend node_modules root.
      typeRoots: [path.join(root, "typings")],
    },
    files: [
      {
        path: index,
        text: "const page = Component({ data: { flag: false } });\nvoid page;\n",
      },
    ],
    readPolicy: defaultReadPolicy(BACKEND_DIR),
  });
  const program = project.languageService().getProgram();
  assert.ok(program);
  const missing = ts
    .getPreEmitDiagnostics(program)
    .filter((diagnostic) => diagnostic.code === 2304 || diagnostic.code === 2688);
  assert.deepEqual(missing, [], "Component and the pinned globals must stay declared");

  const checker = program.getTypeChecker();
  const source = program.getSourceFile(index);
  assert.ok(source);
  let flag: ts.Node | undefined;
  const visit = (node: ts.Node): void => {
    if (ts.isIdentifier(node) && node.text === "flag") flag = node;
    ts.forEachChild(node, visit);
  };
  visit(source);
  assert.ok(flag);
  const typeText = checker.typeToString(checker.getTypeAtLocation(flag));
  assert.notEqual(
    typeText,
    "any",
    "template data must keep its declared type instead of degrading to any",
  );
  assert.ok(typeText === "false" || typeText === "boolean", typeText);
  resetProjectPool();
});
