import assert from "node:assert/strict";
import { test } from "node:test";
import ts from "typescript";
import { BACKEND_DIR } from "../src/env.ts";
import {
  DEFAULT_LIMITS,
  PROTOCOL_VERSION,
  RequestError,
  validateSymbolsRequest,
  type RequestFile,
  type SymbolsRequest,
} from "../src/protocol.ts";
import { symbols } from "../src/symbols.ts";
import { TEST_PROJECT_ROOT, byteIndexOf } from "./support.ts";

const config = { backendDir: BACKEND_DIR };
const SCRIPT = "pages/index/index.ts";

function file(path: string, text: string): RequestFile {
  return {
    fileId: path,
    path,
    kind: /\.js$/.test(path) ? "js" : "ts",
    text,
    sourceRevision: 1,
  };
}

function request(
  script: string,
  query: SymbolsRequest["query"],
  options: {
    root?: "page" | "component";
    subjectFileId?: string;
    extraFiles?: RequestFile[];
  } = {},
): SymbolsRequest {
  const subject = options.subjectFileId ?? SCRIPT;
  return {
    protocolVersion: PROTOCOL_VERSION,
    requestId: "symbols:1",
    project: {
      projectRoot: TEST_PROJECT_ROOT,
      compilerOptions: {
        strict: true,
        target: "ES2022",
        module: "ESNext",
        moduleResolution: "bundler",
        types: ["miniprogram-api-typings"],
      },
      typings: { package: "miniprogram-api-typings", version: "5.3.1" },
    },
    files: [file(subject, script), ...(options.extraFiles ?? [])],
    limits: DEFAULT_LIMITS,
    subjectFileId: subject,
    root: options.root ?? "page",
    query,
  };
}

test("page data path returns declaration, this.data references and setData segments", () => {
  const script = `Page({
  data: {
    user: { name: "x" },
    count: 1,
  },
  onLoad() {
    const label = this.data.user.name;
    this.setData({ "user.name": label });
  },
});`;
  const result = symbols(
    request(script, { kind: "dataPath", path: ["user", "name"] }),
    config,
  );
  assert.equal(result.state.status, "supported");
  assert.equal(result.complete, true);
  assert.ok(result.declaration);
  const declarationStart = byteIndexOf(script, "name: \"x\"");
  assert.deepEqual(result.declaration, {
    fileId: SCRIPT,
    start: declarationStart,
    end: declarationStart + "name".length,
  });
  const reference = result.references.find(
    (entry) => entry.start === byteIndexOf(script, "this.data.user.name") + "this.data.user.".length,
  );
  assert.ok(reference, "this.data.user.name reference");
  const setData = result.occurrences.find((entry) => entry.kind === "setDataKey");
  assert.ok(setData);
  assert.deepEqual(setData.path, ["user", "name"]);
  assert.equal(setData.segment, 1);
  const keyStart = byteIndexOf(script, "\"user.name\"") + 1;
  assert.equal(setData.start, keyStart + "user.".length);
  assert.equal(setData.end, keyStart + "user.name".length);
});

test("root rename covers the first setData path segment and sibling fields", () => {
  const script = `Page({
  data: { user: { name: "x" } },
  onLoad() { this.setData({ "user.name": "y" }); },
});`;
  const result = symbols(request(script, { kind: "dataPath", path: ["user"] }), config);
  assert.equal(result.complete, true);
  assert.ok(result.declaration);
  const occurrence = result.occurrences.find((entry) => entry.kind === "setDataKey");
  assert.ok(occurrence, "root setData segment");
  const keyStart = byteIndexOf(script, "\"user.name\"") + 1;
  assert.deepEqual({ start: occurrence.start, end: occurrence.end }, {
    start: keyStart,
    end: keyStart + "user".length,
  });
  assert.ok(result.fields.some((field) => field.name === "user"));
});

test("component properties expose observers and a safe missing-handler insertion", () => {
  const script = `Component({
  properties: {
    title: { type: String, observer: "title" },
  },
  data: { open: false },
  observers: { "title, open": function () {} },
  methods: {
    onTap() {},
  },
});`;
  const property = symbols(
    request(script, { kind: "dataPath", path: ["title"] }, { root: "component" }),
    config,
  );
  assert.equal(property.complete, true);
  assert.ok(property.declaration);
  assert.ok(property.occurrences.some((entry) => entry.kind === "observerPath"));
  const occurrence = property.occurrences.find((entry) => entry.kind === "observerPath");
  assert.equal(occurrence?.segment, 0);

  const missing = symbols(
    request(script, { kind: "eventHandler", name: "onSave" }, { root: "component" }),
    config,
  );
  assert.equal(missing.state.status, "supported");
  assert.equal(missing.complete, true);
  assert.equal(missing.declaration, undefined);
  assert.ok(missing.insertion);
  assert.match(missing.insertion.text, /onSave\(\)/);
  assert.ok(!missing.insertion.text.includes("methods: {"));
  assert.ok(missing.insertion.offset <= Buffer.byteLength(script, "utf8"));
});

test("missing component methods are created around a methods object", () => {
  const script = `Component({
  properties: { title: String },
});`;
  const missing = symbols(
    request(script, { kind: "eventHandler", name: "onSave" }, { root: "component" }),
    config,
  );
  assert.equal(missing.complete, true);
  assert.ok(missing.insertion);
  assert.match(missing.insertion.text, /methods: \{/);
  assert.match(missing.insertion.text, /onSave\(\)/);
});

test("shared data objects resolve to their declaration file", () => {
  const shared = `export const sharedData = { title: "x" };`;
  const script = `import { sharedData } from "./shared";
Page({ data: sharedData, onLoad() { const t = this.data.title; } });`;
  const result = symbols(
    request(
      script,
      { kind: "dataPath", path: ["title"] },
      { extraFiles: [file("pages/index/shared.ts", shared)] },
    ),
    config,
  );
  assert.equal(result.state.status, "partial");
  assert.equal(result.complete, false);
  assert.equal(result.declaration?.fileId, "pages/index/shared.ts");
  const titleStart = byteIndexOf(shared, "title: \"x\"");
  assert.deepEqual(result.declaration, {
    fileId: "pages/index/shared.ts",
    start: titleStart,
    end: titleStart + "title".length,
  });
  assert.ok(
    result.references.some(
      (reference) =>
        reference.fileId === SCRIPT &&
        reference.start === byteIndexOf(script, "this.data.title") + "this.data.".length,
    ),
    "cross-file this.data reference",
  );
  const roots = symbols(
    request(
      script,
      { kind: "dataRoots" },
      { extraFiles: [file("pages/index/shared.ts", shared)] },
    ),
    config,
  );
  assert.ok(roots.fields.some((field) => field.name === "title"));
});

test("dynamic data/setData shapes are reported as incomplete", () => {
  const script = `Page({
  data: { ...shared, [key]: 1, fn() {} },
  onLoad(value: unknown) { this.setData({ [String(value)]: 1 }); },
});`;
  const result = symbols(request(script, { kind: "dataRoots" }), config);
  assert.equal(result.complete, false);
  assert.equal(result.state.status, "partial");
  assert.ok(result.reason && result.reason.length > 0);
});

test("a missing event handler on an ambiguous owner table has no insertion", () => {
  const script = `const extra = {};
Page({ ...extra });`;
  const result = symbols(
    request(script, { kind: "eventHandler", name: "onTap" }),
    config,
  );
  assert.equal(result.complete, false);
  assert.equal(result.insertion, undefined);
});

test("UTF-8 offsets are byte ranges, not UTF-16 indices", () => {
  const script = `Page({ data: { 名称: "值", name: "x" } });`;
  const result = symbols(
    request(script, { kind: "dataRoots" }),
    config,
  );
  const name = result.fields.find((field) => field.name === "name");
  assert.ok(name);
  const expected = byteIndexOf(script, "name: \"x\"");
  assert.deepEqual(name.declaration, {
    fileId: SCRIPT,
    start: expected,
    end: expected + "name".length,
  });
});

test("symbols request validation rejects malformed queries", () => {
  const good = request(`Page({ data: {} });`, { kind: "dataRoots" });
  validateSymbolsRequest(good);
  for (const query of [
    { kind: "dataPath", path: [] },
    { kind: "dataPath", path: Array.from({ length: 17 }, (_, index) => `s${index}`) },
    { kind: "dataPath", path: ["a[b]"] },
    { kind: "eventHandler", name: "a.b" },
  ]) {
    const bad = { ...good, query } as SymbolsRequest;
    assert.throws(() => validateSymbolsRequest(bad), RequestError);
  }
  assert.throws(
    () => validateSymbolsRequest({ ...good, subjectFileId: "other.ts" }),
    RequestError,
  );
  assert.throws(
    () => validateSymbolsRequest({ ...good, protocolVersion: 1 }),
    RequestError,
  );
});

test("event handler references include this.method calls", () => {
  const script = `Page({
  onTap() {},
  onLoad() { this.onTap(); },
});`;
  const result = symbols(request(script, { kind: "eventHandler", name: "onTap" }), config);
  assert.equal(result.complete, true);
  const declarationStart = byteIndexOf(script, "onTap()");
  assert.deepEqual(result.declaration, {
    fileId: SCRIPT,
    start: declarationStart,
    end: declarationStart + "onTap".length,
  });
  assert.ok(
    result.references.some(
      (reference) =>
        reference.start === byteIndexOf(script, "this.onTap") + "this.".length,
    ),
    JSON.stringify(result.references),
  );
});

test("shorthand destructuring is reported incomplete and never edited", () => {
  const script = `Page({
  data: { user: { name: "x" } },
  onLoad() {
    const { user } = this.data;
    return user.name;
  },
});`;
  const result = symbols(request(script, { kind: "dataPath", path: ["user"] }), config);
  assert.equal(result.complete, false);
  assert.equal(result.state.status, "partial");
});

test("renamed destructuring binding stays safe", () => {
  const script = `Page({
  data: { user: { name: "x" } },
  onLoad() {
    const { user: localUser } = this.data;
    return localUser.name;
  },
});`;
  const result = symbols(request(script, { kind: "dataPath", path: ["user"] }), config);
  assert.equal(result.complete, true);
  assert.ok(result.declaration);
});

test("other.setData receivers are not edited or claimed complete", () => {
  const script = `Page({
  data: { user: { name: "x" } },
  onLoad() {
    other.setData({ user: 1 });
  },
});`;
  const result = symbols(request(script, { kind: "dataPath", path: ["user"] }), config);
  assert.equal(result.complete, false);
  assert.equal(result.occurrences.length, 0);
});

test("dynamic this.data access and spreads make coverage incomplete", () => {
  for (const body of [
    `const key = "user"; this.data[key];`,
    `const { ...rest } = this.data; return rest;`,
    `Object.keys(this.data);`,
  ]) {
    const script = `Page({ data: { user: 1 }, onLoad() { ${body} } });`;
    const result = symbols(request(script, { kind: "dataPath", path: ["user"] }), config);
    assert.equal(result.complete, false, body);
  }
});

test("sourceComplete=false forces a partial empty answer", () => {
  const script = `Page({ data: { user: 1 } });`;
  const req = request(script, { kind: "dataRoots" });
  req.sourceComplete = false;
  const result = symbols(req, config);
  assert.equal(result.complete, false);
  assert.equal(result.state.status, "partial");
  assert.equal(result.fields.length, 0);
});

test("component methods insertion with existing members parses without TS1005", () => {
  const script = `Component({
  properties: { title: { type: String } },
  data: { local: "x" },
});`;
  const missing = symbols(
    request(script, { kind: "eventHandler", name: "onSave" }, { root: "component" }),
    config,
  );
  assert.equal(missing.complete, true);
  assert.ok(missing.insertion);
  const applied =
    script.slice(0, missing.insertion.offset) +
    missing.insertion.text +
    script.slice(missing.insertion.offset);
  const parsed = ts.createSourceFile("component.ts", applied, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const diagnostics = (parsed as ts.SourceFile & { parseDiagnostics: readonly ts.Diagnostic[] })
    .parseDiagnostics;
  assert.deepEqual(diagnostics.map((diag) => diag.code), []);
});

test("component reserved names include data, properties and methods", () => {
  const script = `Component({
  properties: { title: { type: String }, heading: { type: String } },
  data: { local: "x", user: { name: "x" } },
  methods: { onTap() {} },
});`;
  const property = symbols(
    request(script, { kind: "dataPath", path: ["title"] }, { root: "component" }),
    config,
  );
  assert.equal(property.complete, true);
  for (const name of ["local", "heading", "onTap", "title"]) {
    assert.ok(property.reserved.includes(name), `${name} must be reserved`);
  }
  const data = symbols(
    request(script, { kind: "dataPath", path: ["user", "name"] }, { root: "component" }),
    config,
  );
  assert.equal(data.complete, true);
  for (const name of ["heading", "local", "onTap"]) {
    assert.ok(data.reserved.includes(name), `${name} must be reserved`);
  }
});

test("nested setData in another script is incomplete", () => {
  const script = `Page({ data: { user: { name: "x" } } });`;
  const other = file(
    "other.ts",
    `export function update(page: any) { page.setData({ user: { name: "y" } }); }`,
  );
  const result = symbols(
    request(script, { kind: "dataPath", path: ["user", "name"] }, { extraFiles: [other] }),
    config,
  );
  assert.equal(result.complete, false);
  assert.equal(result.state.status, "partial");
  assert.equal(result.occurrences.length, 0);
});

test("this aliases with dynamic data access are incomplete", () => {
  const script = `Page({
  data: { user: { name: "x" } },
  onLoad() {
    const that = this;
    const self = that;
    const key = "user";
    return self.data[key];
  },
});`;
  const result = symbols(
    request(script, { kind: "dataPath", path: ["user"] }),
    config,
  );
  assert.equal(result.complete, false);
  assert.equal(result.state.status, "partial");
  assert.equal(result.occurrences.length, 0);
});

test("another page's own this.setData does not block the current page", () => {
  const script = `Page({ data: { user: { name: "x" } } });`;
  const other = file(
    "pages/other/other.ts",
    `Page({ data: { user: 1 }, onLoad() { this.setData({ "user.name": 2 }); } });`,
  );
  const result = symbols(
    request(script, { kind: "dataPath", path: ["user"] }, { extraFiles: [other] }),
    config,
  );
  assert.equal(result.complete, true, JSON.stringify(result));
  assert.ok(result.declaration);
});

test("nested this.setData edits the nested target segment", () => {
  const script = `Page({
  data: { user: { name: "x" } },
  onLoad() { this.setData({ user: { name: "y" } }); },
});`;
  const result = symbols(
    request(script, { kind: "dataPath", path: ["user", "name"] }),
    config,
  );
  assert.equal(result.complete, true, JSON.stringify(result));
  const occurrence = result.occurrences.find((entry) => entry.kind === "setDataKey");
  assert.ok(occurrence, "nested setData occurrence");
  const start = byteIndexOf(script, `name: "y"`);
  assert.deepEqual({ start: occurrence.start, end: occurrence.end }, {
    start,
    end: start + "name".length,
  });
});
