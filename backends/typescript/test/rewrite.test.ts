import assert from "node:assert/strict";
import { test } from "node:test";
import { analyze } from "../src/analysis.ts";
import { BACKEND_DIR, SAMPLES_DIR } from "../src/env.ts";
import { RequestError, type AnalyzeRequest } from "../src/protocol.ts";
import { rewriteExpression } from "../src/rewrite.ts";
import { inspectRequest, runScenario, type RunConfig } from "../src/run.ts";
import { mapSpanToSource } from "../src/text.ts";
import { scenarioById } from "../src/scenarios.ts";

const config: RunConfig = { backendDir: BACKEND_DIR, samplesDir: SAMPLES_DIR };
const analysisConfig = { backendDir: BACKEND_DIR };

function pageRequest(): AnalyzeRequest {
  return structuredClone(inspectRequest(scenarioById("page-basic"), "in-place-shadow", config));
}

function requestWith(expressions: Array<{ expr: string; target?: string; locals?: string[] }>): AnalyzeRequest {
  const request = pageRequest();
  request.fragments = expressions.map((entry, index) => ({
    fragmentId: `f${index}`,
    exprText: entry.expr,
    ...(entry.target ? { target: entry.target } : {}),
    ...(entry.locals ? { locals: entry.locals } : {}),
    wxmlRange: { start: 0, end: Buffer.byteLength(entry.expr, "utf8") },
    fragmentRevision: 1,
    root: "page",
  }));
  return request;
}

function run(request: AnalyzeRequest) {
  return analyze(request, analysisConfig);
}

function mounted(result: ReturnType<typeof analyze>, id: string): string | undefined {
  if (!result.ok) return undefined;
  return result.response.mounts.find((mount) => mount.fragmentId === id)?.text;
}

function hoverType(result: ReturnType<typeof analyze>, id: string): string | undefined {
  if (!result.ok) return undefined;
  return result.response.fragments.find((fragment) => fragment.fragmentId === id)?.hover?.typeOnly;
}

function errorCodes(result: ReturnType<typeof analyze>): number[] {
  if (!result.ok) return [];
  return result.diagnostics.map((diagnostic) => diagnostic.code);
}

test("unary, parenthesized and multi-root expressions are fully rewritten", () => {
  const result = run(
    requestWith([
      { expr: "!title" },
      { expr: "(title)" },
      { expr: "title + user.name" },
      { expr: "typeof title" },
      { expr: "-title" },
      { expr: "title ? title : user.name" },
    ]),
  );
  assert.equal(result.ok, true);
  assert.equal(mounted(result, "f0"), "!__wxml_root.title");
  assert.equal(mounted(result, "f1"), "(__wxml_root.title)");
  assert.equal(mounted(result, "f2"), "__wxml_root.title + __wxml_root.user.name");
  assert.equal(mounted(result, "f3"), "typeof __wxml_root.title");
  assert.equal(mounted(result, "f4"), "-__wxml_root.title");
  assert.equal(mounted(result, "f5"), "__wxml_root.title ? __wxml_root.title : __wxml_root.user.name");
  assert.equal(hoverType(result, "f0"), "string");
  assert.equal(hoverType(result, "f1"), "string");
  assert.equal(hoverType(result, "f2"), "string", "rightmost target `name`");
  assert.equal(hoverType(result, "f3"), "string");
  assert.equal(hoverType(result, "f4"), "string");
  assert.equal(hoverType(result, "f5"), "string", "conditional target is the rightmost value identifier");
  assert.ok(!errorCodes(result).includes(2304), "no unresolved root identifiers");
});

test("indexed array roots are rewritten before the index", () => {
  const request = structuredClone(inspectRequest(scenarioById("nullable"), "in-place-shadow", config));
  request.fragments = request.fragments.filter((fragment) => fragment.exprText === "users[0].name");
  const result = run(request);
  assert.equal(result.ok, true);
  assert.equal(mounted(result, "users[0].name"), "__wxml_root.users[0].name");
  assert.equal(hoverType(result, "users[0].name"), "string");
  assert.ok(!errorCodes(result).includes(2304));
});

test("template locals are not prefixed anywhere in the expression", () => {
  for (const expr of ["!item.done", "(item).name", "item.done + item.name"]) {
    const result = run(requestWith([{ expr, locals: ["item"] }]));
    assert.equal(result.ok, true);
    assert.equal(mounted(result, "f0"), expr, "locals keep the source text");
    assert.equal(result.ok && result.states.hover.status, "partial");
    assert.equal(result.ok && result.states.diagnostics.status, "partial");
    assert.equal(result.ok && result.states.definition.status, "partial");
    assert.equal(result.ok ? result.response.fragments[0]?.hover?.quality : undefined, "any");
    assert.deepEqual(errorCodes(result), [], "locals never publish fabricated diagnostics");
  }
});

test("template locals may shadow global builtins", () => {
  const result = run(
    requestWith([
      { expr: "Math", locals: ["Math"] },
      { expr: "Math.max(title)", target: "title", locals: ["Math"] },
      { expr: "{ Math }", locals: ["Math"] },
    ]),
  );
  assert.equal(result.ok, true);
  assert.equal(mounted(result, "f0"), "Math");
  assert.equal(mounted(result, "f1"), "Math.max(__wxml_root.title)");
  assert.equal(mounted(result, "f2"), "{ Math: Math }");
  assert.equal(hoverType(result, "f0"), "any", "the local shadows Math, so its type is any");
  assert.equal(hoverType(result, "f1"), "string", "the explicit target inside Math.max is the data root");
  assert.equal(result.ok && result.states.hover.status, "partial");
});

test("string and template text stay verbatim while substitutions are rewritten", () => {
  const result = run(requestWith([{ expr: "`title ${title}`" }, { expr: '"name" + user.name' }]));
  assert.equal(result.ok, true);
  assert.equal(mounted(result, "f0"), "`title ${__wxml_root.title}`");
  assert.equal(mounted(result, "f1"), '"name" + __wxml_root.user.name');
  assert.equal(hoverType(result, "f0"), "string");
  assert.equal(hoverType(result, "f1"), "string");
});

test("numeric literals do not become default targets", () => {
  const result = run(requestWith([{ expr: "title + 1e10" }, { expr: "title + 0x1f" }]));
  assert.equal(result.ok, true);
  assert.equal(mounted(result, "f0"), "__wxml_root.title + 1e10");
  assert.equal(mounted(result, "f1"), "__wxml_root.title + 0x1f");
  assert.equal(hoverType(result, "f0"), "string");
  assert.equal(hoverType(result, "f1"), "string");
});

test("member names, object keys, strings and globals are not rewritten", () => {
  const result = run(
    requestWith([
      { expr: "user.name" },
      { expr: "{ title: user.name, user }" },
      { expr: 'user.name + "title"' },
      { expr: "Math.max(title)" },
      { expr: "user?.name" },
      { expr: "format(user.name)" },
    ]),
  );
  assert.equal(result.ok, true);
  assert.equal(mounted(result, "f0"), "__wxml_root.user.name");
  assert.equal(mounted(result, "f1"), "{ title: __wxml_root.user.name, user: __wxml_root.user }");
  assert.equal(mounted(result, "f2"), '__wxml_root.user.name + "title"');
  assert.equal(mounted(result, "f3"), "Math.max(__wxml_root.title)");
  assert.equal(mounted(result, "f4"), "__wxml_root.user?.name");
  assert.equal(mounted(result, "f5"), "__wxml_root.format(__wxml_root.user.name)");
});

test("repeated identifiers keep the rightmost target offset", () => {
  const request = pageRequest();
  const script = request.files.find((file) => file.fileId === request.scriptFileId);
  assert.ok(script);
  script.text = script.text
    .replace("data: {", "data: {\n    a: { a: 'x' },")
    .replace("name: 'Ada',", "name: 'Ada',\n      user: 'u',");
  request.fragments = [
    { fragmentId: "a", exprText: "a.a", wxmlRange: { start: 0, end: 3 }, fragmentRevision: 1, root: "page" },
    { fragmentId: "u", exprText: "user.user", wxmlRange: { start: 0, end: 9 }, fragmentRevision: 1, root: "page" },
    { fragmentId: "t", exprText: "title + title", wxmlRange: { start: 0, end: 13 }, fragmentRevision: 1, root: "page" },
  ];
  const result = run(request);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const check = (id: string, expected: { start: number; end: number }, type: string) => {
    const mount = result.response.mounts.find((entry) => entry.fragmentId === id);
    assert.ok(mount, id);
    const mapped = mapSpanToSource(mount, mount.targetStartUtf16, mount.targetLengthUtf16);
    assert.deepEqual(mapped.range, expected, id);
    assert.equal(hoverType(result, id), type, id);
  };
  check("a", { start: 2, end: 3 }, "string"); // second `a`
  check("u", { start: 5, end: 9 }, "string"); // second `user`
  check("t", { start: 8, end: 13 }, "string"); // second `title`
});

test("object keys are not default targets", () => {
  const result = run(requestWith([{ expr: "{ title: user.name, flag: true }" }]));
  assert.equal(result.ok, true);
  assert.equal(mounted(result, "f0"), "{ title: __wxml_root.user.name, flag: true }");
  assert.equal(hoverType(result, "f0"), "string", "the rightmost value identifier is `name`, not the `flag` key");
});

test("object shorthand is expanded safely", () => {
  const result = run(requestWith([{ expr: "{ title }" }]));
  assert.equal(result.ok, true);
  assert.equal(mounted(result, "f0"), "{ title: __wxml_root.title }");
  assert.equal(hoverType(result, "f0"), "string");
});

test("a member typo keeps the exact WXML byte range", () => {
  const request = pageRequest();
  assert.ok(request.fragments.find((fragment) => fragment.fragmentId === "user.nmae"));
  const result = run(request);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const diagnostic = result.response.fragments.find((fragment) => fragment.fragmentId === "user.nmae")?.diagnostics[0];
  assert.deepEqual(diagnostic?.range, { start: 134, end: 138 });
});

test("wxmlRange must match the expression byte length", () => {
  const request = pageRequest();
  request.fragments = [
    {
      fragmentId: "f0",
      exprText: "title",
      wxmlRange: { start: 0, end: 99 },
      fragmentRevision: 1,
      root: "page",
    },
  ];
  assert.throws(
    () => run(request),
    (error: unknown) => error instanceof RequestError && error.code === "bad-fragment",
  );
});

test("targets inside comments or strings are rejected", () => {
  for (const [expr, target] of [
    ["title /* name */", "name"],
    ["title // name", "name"],
    ['title + "name"', "name"],
  ] as const) {
    const request = requestWith([{ expr, target }]);
    assert.throws(
      () => run(request),
      (error: unknown) => error instanceof RequestError && error.code === "bad-fragment",
      expr,
    );
  }
});

test("wrapper escapes and extra statements are rejected", () => {
  for (const expr of ["title); (user", "title) + (user", "title);"]) {
    const request = requestWith([{ expr }]);
    assert.throws(
      () => run(request),
      (error: unknown) => error instanceof RequestError && error.code === "bad-fragment",
      expr,
    );
  }
});

test("unsupported expression shapes fail closed", () => {
  for (const expr of ["new Foo()", "title satisfies string", "foo<Bar>()", "title as string"]) {
    assert.throws(
      () => run(requestWith([{ expr }])),
      (error: unknown) => error instanceof RequestError && error.code === "bad-fragment",
      expr,
    );
  }
});

test("depth limit is counted from the parsed root expression", () => {
  const atLimit = `${"(".repeat(256)}title${")".repeat(256)}`;
  const overLimit = `${"(".repeat(257)}title${")".repeat(257)}`;
  for (const target of [undefined, "title"] as const) {
    const ok = run(requestWith([{ expr: atLimit, ...(target ? { target } : {}) }]));
    assert.equal(ok.ok, true, `depth 256 should be accepted (target ${target ?? "default"})`);
    assert.throws(
      () => run(requestWith([{ expr: overLimit, ...(target ? { target } : {}) }])),
      (error: unknown) => error instanceof RequestError && error.code === "bad-fragment",
      `depth 257 should be rejected (target ${target ?? "default"})`,
    );
  }
});

test("deeply nested and malformed expressions are bad-fragment, not RangeError", () => {
  const deep = `${"(".repeat(300)}title${")".repeat(300)}`;
  for (const expr of [deep, "title)"]) {
    assert.throws(
      () => run(requestWith([{ expr }])),
      (error: unknown) => error instanceof RequestError && error.code === "bad-fragment",
      expr.slice(0, 20),
    );
  }
});

test("error messages truncate huge expressions", () => {
  const expr = "(".repeat(5000);
  assert.throws(
    () => run(requestWith([{ expr }])),
    (error: unknown) => {
      assert.ok(error instanceof RequestError);
      assert.equal(error.code, "bad-fragment");
      assert.ok(error.message.length < 300, `message was ${error.message.length} chars`);
      assert.match(error.message, /total 5000 UTF-16 chars/);
      return true;
    },
  );
});

test("expressions without a free root identifier are rejected", () => {
  for (const expr of ["1 + 2", "Math.random()"]) {
    assert.throws(
      () => run(requestWith([{ expr }])),
      (error: unknown) => error instanceof RequestError && error.code === "bad-fragment",
      expr,
    );
  }
});

test("rewritten mounts map target spans and trim any synthetic prefix", () => {
  const result = run(requestWith([{ expr: "!title" }, { expr: "title + user.name" }]));
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const bang = result.response.mounts.find((mount) => mount.fragmentId === "f0");
  assert.ok(bang);
  assert.deepEqual(
    mapSpanToSource(bang, bang.targetStartUtf16, bang.targetLengthUtf16).range,
    { start: 1, end: 6 },
    "`!title` maps the target after the operator",
  );
  const sum = result.response.mounts.find((mount) => mount.fragmentId === "f1");
  assert.ok(sum);
  assert.deepEqual(
    mapSpanToSource(sum, sum.targetStartUtf16, sum.targetLengthUtf16).range,
    { start: 13, end: 17 },
    "`title + user.name` maps the rightmost target",
  );

  // Start inside the first synthetic prefix, end in the first source piece:
  // the mapped span is trimmed to the source part.
  const trimmed = mapSpanToSource(sum, sum.startUtf16 + 5, 9);
  assert.equal(trimmed.trimmedSyntheticPrefix, true);
  assert.deepEqual(trimmed.range, { start: 0, end: 2 });

  // The second synthetic prefix (`user`) is trimmed the same way.
  const second = mapSpanToSource(sum, sum.startUtf16 + 25, 10);
  assert.equal(second.trimmedSyntheticPrefix, true);
  assert.deepEqual(second.range, { start: 8, end: 11 });

  // A span crossing a synthetic gap is cropped to the target (a real operand)
  // instead of being silently dropped.
  const crossed = mapSpanToSource(sum, sum.startUtf16 + 10, sum.text.length - 10);
  assert.equal(crossed.trimmedSyntheticPrefix, true);
  assert.deepEqual(crossed.range, { start: 13, end: 17 }, "`user.name` is the target operand");
  const unary = mapSpanToSource(bang, bang.startUtf16, bang.text.length);
  assert.equal(unary.trimmedSyntheticPrefix, true);
  assert.deepEqual(unary.range, { start: 1, end: 6 }, "`!title` crops to `title`");
});

test("cross-gap diagnostics are cropped to a real operand", () => {
  const cases: Array<[string, number, { start: number; end: number }]> = [
    ["title < user.age", 2365, { start: 13, end: 16 }],
    ["user.age < title", 2365, { start: 11, end: 16 }],
    ["user.age * user.name", 2363, { start: 11, end: 20 }],
  ];
  for (const [expr, code, range] of cases) {
    const result = run(requestWith([{ expr }]));
    assert.equal(result.ok, true, expr);
    if (!result.ok) continue;
    const diagnostics = result.response.fragments[0]?.diagnostics ?? [];
    assert.equal(diagnostics.length, 1, expr);
    assert.equal(diagnostics[0]?.code, code, expr);
    assert.deepEqual(diagnostics[0]?.range, range, expr);
    assert.equal(diagnostics[0]?.attribution, "trimmed-synthetic", expr);
    assert.equal(result.states.diagnostics.status, "supported", expr);
  }
});

test("number+string concatenation is not fabricated as an error", () => {
  const result = run(requestWith([{ expr: "user.age + user.name" }, { expr: "-title" }]));
  assert.equal(result.ok, true);
  if (!result.ok) return;
  for (const fragment of result.response.fragments) {
    assert.deepEqual(fragment.diagnostics, [], fragment.fragmentId);
  }
  assert.equal(result.states.diagnostics.status, "supported");
});

test("expression emission is linear for 1 MiB and 4 MiB / 9k roots", () => {
  const make = (bytes: number, roots: number): string => {
    const rootList = Array.from({ length: roots }, (_, index) => `a${index}`).join(",");
    // Put the padding BEFORE the roots: the old O(n²) emitter calls
    // `expr.slice(0, occurrence.start)`, so every root has to copy the entire
    // padding. Padding after the roots does not trigger that cost.
    const fixed = Buffer.byteLength(`[,${rootList}]`) + 2; // brackets, comma, JSON quotes
    const padding = Math.max(1, bytes - fixed);
    return `[${JSON.stringify("x".repeat(padding))},${rootList}]`;
  };
  const options = {
    rootPrefix: "__wxml_root.",
    propsPrefix: "__wxml_props_0.",
    propertyOverlay: new Set<string>(),
    dataOverlay: new Set<string>(),
    locals: new Set<string>(),
  };
  const measure = (bytes: number): number => {
    const expr = make(bytes, 9000);
    const start = performance.now();
    const result = rewriteExpression(expr, undefined, options);
    const elapsed = performance.now() - start;
    assert.equal(result.roots.length, 9000);
    return elapsed;
  };
  const small = measure(1024 * 1024);
  const large = measure(4 * 1024 * 1024);
  console.log(`[perf] rewrite 1 MiB ${small.toFixed(1)} ms, 4 MiB ${large.toFixed(1)} ms (padding first)`);
  // Thresholds are tight enough that the pre-fix O(n²) emitter fails
  // (manual worst-case measurement: ~4.1 s / ~15.6 s).
  assert.ok(small < 300, `1 MiB rewrite took ${small.toFixed(1)} ms (limit 300 ms)`);
  assert.ok(large < 600, `4 MiB rewrite took ${large.toFixed(1)} ms (limit 600 ms)`);
  assert.ok(large < small * 8 + 500, `rewrite should scale near-linearly: ${small.toFixed(1)} -> ${large.toFixed(1)} ms`);
});

test("rewritten roots keep mapping for hover and diagnostics", () => {
  const run = runScenario(scenarioById("nullable"), "in-place-shadow", config);
  assert.equal(run.diagnosticsExact, true);
  assert.deepEqual(
    run.diagnostics
      .map((diagnostic) => diagnostic.range)
      .sort((left, right) => (left?.start ?? 0) - (right?.start ?? 0)),
    [
      { start: 45, end: 54 },
      { start: 98, end: 106 },
    ],
  );
});
