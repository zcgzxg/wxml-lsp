import assert from "node:assert/strict";
import { test } from "node:test";
import { BACKEND_DIR } from "../src/env.ts";
import { analyze } from "../src/analysis.ts";
import type { PreparedView } from "../src/analysis.ts";
import { collectEventResults, MAX_EVENT_RESULTS, MAX_SIGNATURE_LENGTH } from "../src/events.ts";
import {
  DEFAULT_LIMITS,
  RequestError,
  totalBytes,
  validateRequest,
  type AnalyzeRequest,
  type RequestEvent,
} from "../src/protocol.ts";
import { analyzeRequest, byteIndexOf, TEST_PROJECT_ROOT } from "./support.ts";

const config = { backendDir: BACKEND_DIR };
const SCRIPT_PATH = "pages/index/index.ts";

/** A request over one in-memory script plus the event bindings under test. */
function requestWith(
  events: RequestEvent[],
  options: { script: string; kind?: "page" | "component" },
): AnalyzeRequest {
  const request = analyzeRequest({
    script: options.script,
    scriptPath: SCRIPT_PATH,
    kind: options.kind ?? "page",
  });
  request.events = events;
  return request;
}

/** Only the fields `collectEventResults` reads are needed for AST-only tests. */
function stubView(kind: "page" | "component", script: string): PreparedView {
  return {
    ok: true,
    scriptPath: SCRIPT_PATH,
    scriptText: script,
    registration: { kind, count: 1, topLevel: true },
  } as unknown as PreparedView;
}

function event(id: string, handler: string, overrides: Partial<RequestEvent> = {}): RequestEvent {
  return {
    eventId: id,
    attribute: "bindtap",
    event: "tap",
    handler,
    attributeRange: { start: 10, end: 22 },
    handlerRange: { start: 22, end: 22 + Buffer.byteLength(handler, "utf8") },
    ...overrides,
  };
}

function run(script: string, kind: "page" | "component", events: RequestEvent[]) {
  return collectEventResults(requestWith(events, { script, kind }), stubView(kind, script));
}

test("page method shorthand, function property and arrow property resolve", () => {
  const script = `Page({\n  onTap() {},\n  onLoad: function () {},\n  onArrow: () => {},\n})`;
  const collection = run(script, "page", [
    event("e0", "onTap"),
    event("e1", "onLoad"),
    event("e2", "onArrow"),
  ]);

  assert.equal(collection.skipped, 0);
  assert.equal(collection.results.length, 3);
  for (const [index, name] of ["onTap", "onLoad", "onArrow"].entries()) {
    const result = collection.results[index]!;
    assert.equal(result.eventId, `e${index}`);
    assert.equal(result.state.status, "supported");
    assert.equal(result.diagnostic, undefined);
    assert.equal(result.definition?.fileId, SCRIPT_PATH);
    const start = byteIndexOf(script, name);
    assert.deepEqual(result.definition?.range, { start, end: start + Buffer.byteLength(name) });
    assert.ok(result.signature && result.signature.includes(name), `signature: ${result.signature}`);
  }
});

test("component methods shorthand and arrow property resolve", () => {
  const script = `Component({ methods: { save() {}, load: () => {} }, data: {} })`;
  const collection = run(script, "component", [event("e0", "save"), event("e1", "load")]);

  assert.equal(collection.skipped, 0);
  for (const [index, name] of ["save", "load"].entries()) {
    const result = collection.results[index]!;
    assert.equal(result.state.status, "supported");
    assert.equal(result.definition?.fileId, SCRIPT_PATH);
    const start = byteIndexOf(script, name);
    assert.deepEqual(result.definition?.range, { start, end: start + Buffer.byteLength(name) });
    assert.ok(result.signature && result.signature.includes(name));
  }
});

test("every supported binding form maps to the same handler", () => {
  const script = `Page({ onTap() {} })`;
  const attributes = [
    "bindtap",
    "catchtap",
    "bind:tap",
    "catch:tap",
    "capture-bind:tap",
    "capture-catch:tap",
    "mut-bind:tap",
  ];
  const events = attributes.map((attribute, index) => event(`e${index}`, "onTap", { attribute }));
  const collection = run(script, "page", events);

  assert.equal(collection.results.length, attributes.length);
  const expected = { start: byteIndexOf(script, "onTap"), end: byteIndexOf(script, "onTap") + 5 };
  for (const result of collection.results) {
    assert.equal(result.state.status, "supported");
    assert.deepEqual(result.definition?.range, expected);
  }
});

test("definition ranges are UTF-8 bytes after multi-byte text", () => {
  const script = `const note = "中文注释😀";\nPage({\n  onTap() {},\n})`;
  const collection = run(script, "page", [event("e0", "onTap")]);
  const start = byteIndexOf(script, "onTap()");

  assert.ok(start > 0);
  assert.equal(collection.results[0]!.definition?.range.start, start);
  assert.equal(
    collection.results[0]!.definition?.range.end,
    start + Buffer.byteLength("onTap", "utf8"),
  );
});

test("a missing handler is supported with an error at the exact handler range", () => {
  const script = `Page({ data: {} })`;
  const handlerRange = { start: 25, end: 39 };
  const collection = run(script, "page", [
    event("e0", "missingHandler", { attribute: "bind:tap", handlerRange }),
  ]);
  const result = collection.results[0]!;

  assert.equal(result.state.status, "supported");
  assert.equal(result.definition, undefined);
  assert.deepEqual(result.diagnostic?.range, handlerRange);
  assert.equal(result.diagnostic?.severity, "error");
  assert.match(result.diagnostic?.message ?? "", /missingHandler/);
  assert.match(result.diagnostic?.message ?? "", /"tap"/);
  assert.match(result.diagnostic?.message ?? "", /page/);
  assert.ok(!result.diagnostic!.message.includes(TEST_PROJECT_ROOT));
  assert.ok(!result.diagnostic!.message.includes("/tmp/"));
});

test("multiple page registrations are partial, use the first and suppress missing diagnostics", () => {
  const script = `Page({ onTap() {} });\nPage({ onTap() {} });`;
  const collection = run(script, "page", [event("e0", "onTap"), event("e1", "noSuchHandler")]);
  const [known, missing] = collection.results;

  assert.equal(known!.state.status, "partial");
  assert.match(known!.state.reason ?? "", /multiple Page registrations/);
  assert.equal(known!.definition?.range.start, byteIndexOf(script, "onTap"));
  assert.equal(known!.diagnostic, undefined);

  assert.equal(missing!.state.status, "partial");
  assert.equal(missing!.diagnostic, undefined);
});

test("non-literal registrations are partial without a missing-handler diagnostic", () => {
  for (const script of [
    `const options = {};\nPage(options);`,
    `const options: any = {};\nPage(options);`,
  ]) {
    const collection = run(script, "page", [event("e0", "noSuchHandler")]);
    assert.equal(collection.results[0]!.state.status, "partial", script);
    assert.match(collection.results[0]!.state.reason ?? "", /registration is not a literal object/);
    assert.equal(collection.results[0]!.diagnostic, undefined);
  }
});

test("component behaviors may define handlers and suppress missing diagnostics", () => {
  const script = `const base = Behavior({});\nComponent({ behaviors: [base], methods: {} })`;
  const collection = run(script, "component", [event("e0", "fromBehavior")]);
  const result = collection.results[0]!;

  assert.equal(result.state.status, "partial");
  assert.match(result.state.reason ?? "", /behaviors/);
  assert.equal(result.diagnostic, undefined);
});

test("an empty literal behaviors array keeps the methods table complete", () => {
  const script = `Component({ behaviors: [], methods: {} })`;
  const collection = run(script, "component", [event("e0", "noSuchHandler")]);
  const result = collection.results[0]!;

  assert.equal(result.state.status, "supported");
  assert.equal(result.diagnostic?.severity, "error");
});

test("a literal registration is partial when a non-literal one also exists", () => {
  const script = `const options = {};\nPage({ onTap() {} });\nPage(options);`;
  const collection = run(script, "page", [event("e0", "onTap"), event("e1", "noSuchHandler")]);
  const [known, missing] = collection.results;

  assert.equal(known!.state.status, "partial");
  assert.ok(known!.definition);
  assert.equal(known!.diagnostic, undefined);
  assert.equal(missing!.state.status, "partial");
  assert.equal(missing!.diagnostic, undefined);
});

test("spread in options or methods is partial; known handlers still resolve", () => {
  const pageScript = `const base = { data: {} };\nPage({ ...base, onTap() {} })`;
  const page = run(pageScript, "page", [event("e0", "onTap"), event("e1", "noSuchHandler")]);
  assert.match(page.results[0]!.state.reason ?? "", /spread/);
  assert.equal(page.results[0]!.definition?.range.start, byteIndexOf(pageScript, "onTap"));
  assert.equal(page.results[0]!.diagnostic, undefined);
  assert.equal(page.results[1]!.state.status, "partial");
  assert.equal(page.results[1]!.diagnostic, undefined);

  const componentScript = `const base = {};\nComponent({ methods: { ...base, save() {} } })`;
  const component = run(componentScript, "component", [
    event("e0", "save"),
    event("e1", "noSuchHandler"),
  ]);
  assert.match(component.results[0]!.state.reason ?? "", /spread/);
  assert.equal(component.results[0]!.definition?.range.start, byteIndexOf(componentScript, "save"));
  assert.equal(component.results[0]!.diagnostic, undefined);
  assert.equal(component.results[1]!.state.status, "partial");
  assert.equal(component.results[1]!.diagnostic, undefined);
});

test("a component without methods reports a real missing handler", () => {
  const script = `Component({ data: {} })`;
  const collection = run(script, "component", [event("e0", "noSuchHandler")]);
  const result = collection.results[0]!;

  assert.equal(result.state.status, "supported");
  assert.equal(result.definition, undefined);
  assert.equal(result.diagnostic?.severity, "error");
  assert.match(result.diagnostic?.message ?? "", /component/);
});

test("non-literal component methods are partial without a diagnostic", () => {
  const script = `const buildMethods = () => ({});\nComponent({ methods: buildMethods() })`;
  const collection = run(script, "component", [event("e0", "noSuchHandler")]);
  const result = collection.results[0]!;

  assert.equal(result.state.status, "partial");
  assert.match(result.state.reason ?? "", /`methods` is not a literal object/);
  assert.equal(result.diagnostic, undefined);
});

test("reserved page keys and __wxml members are ignored", () => {
  const script = `Page({ data: {}, lifetimes: {}, __wxml_root: 1, onTap() {} })`;
  const collection = run(script, "page", [
    event("e0", "data"),
    event("e1", "lifetimes"),
    event("e2", "__wxml_root"),
    event("e3", "onTap"),
  ]);

  for (const result of collection.results.slice(0, 3)) {
    assert.equal(result.state.status, "supported");
    assert.equal(result.diagnostic?.severity, "error");
  }
  assert.equal(collection.results[3]!.state.status, "supported");
  assert.ok(collection.results[3]!.definition);
});

test("non-identifier and non-literal-function handlers never become diagnostics", () => {
  const script = `Page({ onTap: handlerRef })`;
  const collection = run(script, "page", [event("e0", "a.b"), event("e1", "onTap")]);

  assert.equal(collection.results[0]!.state.status, "partial");
  assert.equal(collection.results[0]!.diagnostic, undefined);

  assert.equal(collection.results[1]!.state.status, "partial");
  assert.match(collection.results[1]!.state.reason ?? "", /not a literal function/);
  assert.equal(collection.results[1]!.definition, undefined);
  assert.equal(collection.results[1]!.diagnostic, undefined);
});

test("long signatures are capped to one bounded line", () => {
  const parameters = Array.from({ length: 80 }, (_, index) => `arg${index}: string`).join(", ");
  const script = `Page({ onTap(${parameters}) {} })`;
  const collection = run(script, "page", [event("e0", "onTap")]);
  const signature = collection.results[0]!.signature ?? "";

  assert.ok(signature.length > 0);
  assert.ok(signature.length <= MAX_SIGNATURE_LENGTH, `signature length ${signature.length}`);
  assert.ok(!signature.includes("\n"));
});

test("event results are capped at the request length bound", () => {
  const script = `Page({ onTap() {} })`;
  const events = Array.from({ length: MAX_EVENT_RESULTS + 1 }, (_, index) =>
    event(`e${index}`, "onTap"),
  );
  const collection = run(script, "page", events);

  assert.equal(collection.results.length, MAX_EVENT_RESULTS);
  assert.equal(collection.skipped, 1);
  assert.ok(collection.skippedDetails.includes("event list is truncated"));
});

test("event strings count against the request byte budget", () => {
  const request = requestWith([event("e0", "on".repeat(4096))], { script: `Page({ data: {} })` });
  const limit = totalBytes(request.files, request.fragments) + 64;

  assert.throws(
    () => validateRequest({ ...request, limits: { ...DEFAULT_LIMITS, maxTotalBytes: limit } }),
    (error: unknown) => error instanceof RequestError && error.code === "too-many-bytes",
    "events[].handler bytes count toward maxTotalBytes",
  );
  assert.doesNotThrow(() => validateRequest({ ...request, limits: { ...DEFAULT_LIMITS } }));
});

test("the early-return analyze path returns eventResults for events only", () => {
  const script = `Page({ onTap() {} })`;
  const result = analyze(requestWith([event("e0", "onTap")], { script }), config);

  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.response.fragments.length, 0);
  assert.equal(result.response.eventResults?.length, 1);
  assert.equal(result.response.eventResults?.[0]?.state.status, "supported");
  assert.equal(result.response.eventResults?.[0]?.definition?.fileId, SCRIPT_PATH);
});

test("the full analyze path returns eventResults without altering fragment results", () => {
  const script = `Page({ data: { title: "t" }, onTap() {} })`;
  const request = requestWith([event("e0", "onTap"), event("e1", "noSuchHandler")], { script });
  request.fragments = [
    {
      fragmentId: "f0",
      exprText: "title",
      wxmlRange: { start: 0, end: 5 },
      fragmentRevision: 1,
      root: "page",
    },
  ];
  const result = analyze(request, config);

  assert.equal(result.ok, true);
  if (!result.ok) return;
  const response = result.response;
  assert.equal(response.fragments.length, 1);
  assert.equal(response.fragments[0]!.diagnostics.length, 0);
  assert.equal(response.eventResults?.length, 2);
  assert.equal(response.eventResults?.[0]?.state.status, "supported");
  assert.ok(response.eventResults?.[0]?.definition);
  assert.equal(response.eventResults?.[1]?.state.status, "supported");
  assert.equal(response.eventResults?.[1]?.diagnostic?.severity, "error");
});
