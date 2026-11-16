import assert from "node:assert/strict";
import { test } from "node:test";
import { BACKEND_DIR } from "../src/env.ts";
import { complete } from "../src/completion.ts";
import { RequestError, type CompleteRequest } from "../src/protocol.ts";

const config = { backendDir: BACKEND_DIR };

const SCRIPT_PATH = "pages/index/index.ts";

interface FixtureOptions {
  script: string;
  fragments?: CompleteRequest["fragments"];
  offset?: number;
  kind?: CompleteRequest["kind"];
  prefix?: string;
  scriptFileId?: string;
  files?: CompleteRequest["files"];
  ownerKind?: "page" | "component";
}

function fixture(options: FixtureOptions): CompleteRequest {
  const scriptFileId = options.scriptFileId ?? SCRIPT_PATH;
  const files =
    options.files ??
    [
      {
        fileId: SCRIPT_PATH,
        path: SCRIPT_PATH,
        kind: "ts" as const,
        text: options.script,
        sourceRevision: 1,
      },
    ];
  return {
    protocolVersion: 2,
    requestId: "completion-test",
    analysisStamp: { snapshotId: 1, projectRevision: 1, viewRevision: 1, backendEpoch: 1 },
    view: { viewId: "view-1", owner: { kind: options.ownerKind ?? "page", fileId: scriptFileId } },
    project: {
      projectRoot: "/tmp/opencode/wxml-completion-fixture",
      compilerOptions: {
        strict: true,
        target: "ES2022",
        module: "ESNext",
        moduleResolution: "bundler",
        types: ["miniprogram-api-typings"],
      },
      typings: { package: "miniprogram-api-typings", version: "5.3.1" },
    },
    scriptFileId,
    files,
    fragments: options.fragments ?? [],
    policy: { jsCheck: "off", propertiesOverlay: "declaration" },
    limits: {},
    offset: options.offset ?? 0,
    kind: options.kind ?? "eventHandler",
    prefix: options.prefix ?? "",
  };
}

function throwsCode(fn: () => unknown, code: string): void {
  assert.throws(fn, (error: unknown) => error instanceof RequestError && error.code === code);
}

test("complete rejects malformed query points", () => {
  const request = fixture({ script: "Page({ data: {} })" });
  throwsCode(() => complete(null as unknown as CompleteRequest, config), "bad-params");
  throwsCode(() => complete({ ...request, offset: -1 }, config), "bad-params");
  throwsCode(() => complete({ ...request, offset: 1.5 }, config), "bad-params");
  throwsCode(
    () => complete({ ...request, kind: "unknown" as CompleteRequest["kind"] }, config),
    "bad-params",
  );
  throwsCode(() => complete({ ...request, prefix: "a".repeat(257) }, config), "bad-limit");
});

test("expression member completion lists data fields of a page", () => {
  const request = fixture({
    script: `Page({ data: { user: { name: "a", age: 1 }, title: "t" }, onTap() {} })`,
    kind: "expressionMember",
    offset: 15,
    fragments: [
      {
        fragmentId: "f0",
        exprText: "user.",
        wxmlRange: { start: 10, end: 15 },
        locals: [],
        fragmentRevision: 1,
        root: "page",
      },
    ],
  });
  const response = complete(request, config);
  assert.equal(response.state.status, "supported");
  const labels = response.items.map((item) => item.label);
  assert.ok(labels.includes("name"), `expected name in ${labels.join(", ")}`);
  assert.ok(labels.includes("age"), `expected age in ${labels.join(", ")}`);
  for (const item of response.items) {
    assert.deepEqual(item.replace, { start: 15, end: 15 });
  }
});

test("expression member completion replaces the partial member prefix", () => {
  const request = fixture({
    script: `Page({ data: { user: { name: "a", age: 1 }, title: "t" }, onTap() {} })`,
    kind: "expressionMember",
    offset: 17,
    fragments: [
      {
        fragmentId: "f0",
        exprText: "user.na",
        wxmlRange: { start: 10, end: 17 },
        locals: [],
        fragmentRevision: 1,
        root: "page",
      },
    ],
  });
  const response = complete(request, config);
  assert.equal(response.state.status, "supported");
  const name = response.items.find((item) => item.label === "name");
  assert.ok(name, "name must be offered");
  assert.deepEqual(name.replace, { start: 15, end: 17 });
});

test("bare root completion lists data keys", () => {
  const request = fixture({
    script: `Page({ data: { user: { name: "a" }, title: "t" }, onTap() {} })`,
    kind: "expressionMember",
    offset: 12,
    fragments: [
      {
        fragmentId: "f0",
        exprText: "us",
        wxmlRange: { start: 10, end: 12 },
        locals: [],
        fragmentRevision: 1,
        root: "page",
      },
    ],
  });
  const response = complete(request, config);
  const labels = response.items.map((item) => item.label);
  assert.ok(labels.includes("user"), JSON.stringify(labels));
  assert.ok(labels.includes("title"), JSON.stringify(labels));
  assert.ok(!labels.includes("name"), JSON.stringify(labels));
  for (const item of response.items) {
    assert.deepEqual(item.replace, { start: 10, end: 12 });
  }
});

test("empty mustache completes data keys", () => {
  const request = fixture({
    script: `Page({ data: { user: { name: "a" }, title: "t" } })`,
    kind: "expressionMember",
    offset: 12,
    fragments: [
      {
        fragmentId: "complete-0",
        exprText: " ",
        wxmlRange: { start: 12, end: 13 },
        locals: [],
        fragmentRevision: 1,
        root: "page",
      },
    ],
  });
  const response = complete(request, config);
  const labels = response.items.map((item) => item.label);
  assert.ok(labels.includes("user"), JSON.stringify(labels));
  assert.ok(labels.includes("title"), JSON.stringify(labels));
});

test("event handler completion lists page methods", () => {
  const request = fixture({
    script: `Page({ data: {}, onTap() {}, onLoad: function () {} })`,
    kind: "eventHandler",
    offset: 20,
    prefix: "on",
  });
  const response = complete(request, config);
  assert.equal(response.state.status, "supported");
  const labels = response.items.map((item) => item.label);
  assert.ok(labels.includes("onTap"));
  assert.ok(labels.includes("onLoad"));
  for (const item of response.items.filter((entry) => entry.label === "onTap" || entry.label === "onLoad")) {
    assert.equal(item.kind, "method");
    assert.equal(item.replace.end - item.replace.start, 2);
  }
});

test("event handler completion lists component methods", () => {
  const request = fixture({
    script: `Component({ methods: { save() {} }, data: {} })`,
    kind: "eventHandler",
    offset: 3,
    prefix: "sa",
    ownerKind: "component",
  });
  const response = complete(request, config);
  assert.equal(response.state.status, "supported");
  const labels = response.items.map((item) => item.label);
  assert.ok(labels.includes("save"));
});

test("an unavailable layout degrades to unsupported without throwing", () => {
  const request = fixture({
    script: "",
    kind: "eventHandler",
    files: [],
    scriptFileId: SCRIPT_PATH,
  });
  const response = complete(request, config);
  assert.equal(response.state.status, "unsupported");
  assert.equal(response.items.length, 0);
  assert.equal(response.truncated, false);
});

test("completion items are capped and report truncation", () => {
  const methods = Array.from({ length: 300 }, (_, index) => `m${index}() {}`).join(",\n");
  const request = fixture({
    script: `Page({ data: {}, ${methods} })`,
    kind: "eventHandler",
    offset: 0,
    prefix: "",
  });
  const response = complete(request, config);
  assert.equal(response.items.length, 256);
  assert.equal(response.truncated, true);
});
