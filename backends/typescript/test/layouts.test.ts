/**
 * Micro-facts that the C1 layout rules depend on.
 *
 * These tests do not rely on the sample matrix: they pin down how
 * `miniprogram-api-typings@5.3.1` treats injected members, `this.data`,
 * `optionalTypes` and JSDoc overlays, so the layout rules are justified by
 * behaviour rather than by memory of the typings.
 */

import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import { findRegistrations, scanDeclaredProperties } from "../src/ast.ts";
import { identifierOffset } from "../src/fragments.ts";
import { baseCompilerOptions, parseSource, VirtualProject, defaultReadPolicy } from "../src/compiler.ts";
import { BACKEND_DIR } from "../src/env.ts";
import { hoverAt, semanticDiagnostics } from "../src/queries.ts";

function project(file: string, text: string): VirtualProject {
  const instance = new VirtualProject({
    rootDir: BACKEND_DIR,
    compilerOptions: baseCompilerOptions(),
    files: [{ path: path.join(BACKEND_DIR, "samples", "page-basic", file), text }],
    readPolicy: defaultReadPolicy(BACKEND_DIR),
  });
  return instance;
}

function fileOf(file: string): string {
  return path.join(BACKEND_DIR, "samples", "page-basic", file);
}

function codesOf(instance: VirtualProject, file: string): number[] {
  return semanticDiagnostics(instance, fileOf(file)).map((diagnostic) => diagnostic.code);
}

const component = (probe: string): string => `Component({
  properties: {
    /** 标题，支持字符串或数字 */
    title: { type: String, optionalTypes: [Number], value: '' },
  },
  data: {
    /** 内部状态 */
    state: 'idle',
  },${probe}
});
`;

test("page options accept an injected probe member", () => {
  const file = "probe-page.ts";
  const text = `Page({
  data: { n: 1 },
  __wxml_check_0() {
    const __wxml_root = this.data;
    return [__wxml_root.n];
  },
});
`;
  assert.deepEqual(codesOf(project(file, text), file), []);
});

test("component options reject an injected probe member (2353)", () => {
  const file = "probe-component.ts";
  const text = component(`
  __wxml_check_0() {
    const __wxml_root = this.data;
    return [__wxml_root.title];
  },`);
  assert.ok(codesOf(project(file, text), file).includes(2353), "expected excess-property diagnostic");
});

test("component `methods` accepts the injected probe", () => {
  const file = "probe-component-methods.ts";
  const text = component(`
  methods: {
    __wxml_check_0() {
      const __wxml_root = this.data;
      return [__wxml_root.state];
    },
  },`);
  assert.deepEqual(codesOf(project(file, text), file), []);
});

test("`this.data` keeps documentation of data fields", () => {
  const file = "probe-doc.ts";
  const text = component(`
  methods: {
    __wxml_check_0() {
      const __wxml_root = this.data;
      return [__wxml_root.state];
    },
  },`);
  const instance = project(file, text);
  const offset = text.indexOf("__wxml_root.state") + "__wxml_root.".length;
  const hover = hoverAt(instance, fileOf(file), offset);
  assert.equal(hover?.typeOnly, "string");
  assert.equal(hover?.documentation, "内部状态");
  assert.equal(hover?.documentationSource, "quickinfo");
});

test("`optionalTypes` is invisible on the instance data type", () => {
  const file = "probe-optional.ts";
  const text = component(`
  methods: {
    __wxml_check_0() {
      const __wxml_root = this.data;
      return [__wxml_root.title];
    },
  },`);
  const instance = project(file, text);
  const offset = text.indexOf("__wxml_root.title") + "__wxml_root.".length;
  const hover = hoverAt(instance, fileOf(file), offset);
  assert.equal(hover?.typeOnly, "string", "typings report `type` only; the union needs the overlay");
  assert.equal(hover?.documentation, "标题，支持字符串或数字");
});

test("JavaScript overlay uses a JSDoc typedef and keeps documentation", () => {
  const file = "probe-js.js";
  const text = `// @ts-check
Component({
  properties: {
    /** 标题，支持字符串或数字 */
    title: { type: String, optionalTypes: [Number], value: '' },
  },
  methods: {
    __wxml_check_0() {
      /**
       * @typedef {Object} __WxmlProps_0
       * @property {string | number} title 标题，支持字符串或数字
       */
      /** @type {unknown} */
      const __wxml_props_raw_0 = null;
      const __wxml_props_0 = /** @type {__WxmlProps_0} */ (__wxml_props_raw_0);
      return [__wxml_props_0.title];
    },
  },
});
`;
  const instance = project(file, text);
  const offset = text.indexOf("__wxml_props_0.title") + "__wxml_props_0.".length;
  const hover = hoverAt(instance, fileOf(file), offset);
  assert.equal(hover?.typeOnly, "string | number");
  assert.equal(hover?.documentation, "标题，支持字符串或数字");
  assert.deepEqual(codesOf(instance, file), []);
});

test("an undeclared property is reported with 2339 and hover quality Missing", () => {
  const file = "probe-missing.ts";
  const text = component(`
  methods: {
    __wxml_check_0() {
      const __wxml_root = this.data;
      return [__wxml_root.subtitle];
    },
  },`);
  const instance = project(file, text);
  const diagnostics = semanticDiagnostics(instance, fileOf(file));
  assert.deepEqual(diagnostics.map((diagnostic) => diagnostic.code), [2339]);
  const start = text.indexOf("__wxml_root.subtitle") + "__wxml_root.".length;
  assert.equal(diagnostics[0]?.start, start, "diagnostic starts exactly at the property name");
  assert.equal(diagnostics[0]?.length, "subtitle".length);
  const hover = hoverAt(instance, fileOf(file), start);
  assert.equal(hover?.typeOnly, "any", "quick info degrades to any");
  assert.equal(hover?.quality, "missing", "quality must not be reported as a known type");
});

test("optionalTypes boundary mapping", () => {
  const text = `Component({
  properties: {
    a: { type: null },
    b: { type: Object },
    c: { type: Array },
    d: { type: Function },
    e: { type: Number, optionalTypes: [String, Number] },
  },
});
`;
  const sourceFile = parseSource(text, "boundaries.ts");
  const scan = findRegistrations(sourceFile);
  const result = scanDeclaredProperties(scan.registrations[0]?.options);
  assert.equal(result.complete, true, result.problems.join("; "));
  const types = new Map(result.properties.map((property) => [property.name, property.typeText]));
  assert.equal(types.get("a"), "any", "`type: null` allows any value");
  assert.equal(types.get("b"), "Record<string, any>");
  assert.equal(types.get("c"), "any[]");
  assert.equal(types.get("d"), "(...args: any[]) => any");
  assert.equal(types.get("e"), "number | string", "duplicates are removed and order follows `type` first");
});

test("unknown property shapes degrade instead of guessing", () => {
  const text = `Component({
  properties: {
    a: { type: SomeConstructor },
    b: { type: String },
  },
});
`;
  const sourceFile = parseSource(text, "unknown.ts");
  const scan = findRegistrations(sourceFile);
  const result = scanDeclaredProperties(scan.registrations[0]?.options);
  assert.equal(result.complete, false, "an unreadable constructor makes the declaration incomplete");
  assert.ok(result.problems.some((problem) => problem.includes("no readable")));
  assert.deepEqual(result.properties.map((property) => property.name), ["b"]);
});

test("non-literal `properties` is reported, never narrowed silently", () => {
  const text = `Component({ properties: sharedProperties });
`;
  const sourceFile = parseSource(text, "shared.ts");
  const scan = findRegistrations(sourceFile);
  const result = scanDeclaredProperties(scan.registrations[0]?.options);
  assert.equal(result.complete, false);
  assert.ok(result.problems.some((problem) => problem.includes("not an object literal")));
});

test("the mounted target is a whole identifier outside string literals", () => {
  assert.equal(identifierOffset("user.name", "name"), 5);
  assert.equal(identifierOffset('user["name"].name', "name"), 13, "the string literal occurrence is skipped");
  assert.equal(identifierOffset("username + name", "name"), 11, "the longer identifier is skipped");
  assert.equal(identifierOffset('"name".length', "name"), -1, "only a literal contains the text");
  assert.equal(identifierOffset("user.name", "nmae"), -1);
});

test("quoted property names keep the definition target inside the quotes", async () => {
  const { renderPropsType } = await import("../src/propsOverlay.ts");
  const text = `Component({
  properties: {
    "a-b": { type: String, optionalTypes: [Number], value: "" },
  },
});
`;
  const sourceFile = parseSource(text, "quoted.ts");
  const scan = findRegistrations(sourceFile);
  const result = scanDeclaredProperties(scan.registrations[0]?.options);
  const rendered = renderPropsType(result.properties, "");
  const member = rendered.members[0]!;
  assert.equal(rendered.text.slice(member.offset, member.offset + member.length), "a-b");
});
