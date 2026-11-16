/**
 * Custom-component property bindings (AC-012).
 *
 * The request carries the component usages and the component script; the backend
 * reads the declared `properties` from the same in-memory snapshot. Every
 * assertion below is on a real `checker.isTypeAssignableTo` / structural result.
 */

import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import ts from "typescript";
import { analyze, buildView } from "../src/analysis.ts";
import { typeOfInitializer } from "../src/behaviors.ts";
import { collectComponentDiagnostics } from "../src/componentBindings.ts";
import { defaultReadPolicy, VirtualProject } from "../src/compiler.ts";
import { BACKEND_DIR } from "../src/env.ts";
import { toCompilerOptions, type AnalyzeRequest, type RequestComponentAttribute, type RequestComponentUsage, type RequestFragment } from "../src/protocol.ts";
import { analyzeRequest, fragment, template, type TestFile } from "./support.ts";

const config = { backendDir: BACKEND_DIR };

const PAGE = `interface User { name: string }
interface Item { id: number }
Page({
  data: {
    pageTitle: 'hello',
    total: 3,
    flag: true,
    list: [] as string[],
    meta: {} as { a: string },
    maybeUser: undefined as User | undefined,
    maybeList: undefined as Item[] | undefined,
    nothing: undefined as undefined,
  },
});
`;

const CARD = `Component({
  properties: {
    title: { type: String },
    count: { type: Number },
    active: { type: Boolean },
    score: { type: String, optionalTypes: [Number] },
    loose: { type: null },
    items: { type: Array },
    meta: { type: Object },
    handler: { type: Function },
  },
});
`;

/** `properties` is not an object literal: the declaration is incomplete. */
const PLAIN = `const shared = {};
Component({ properties: shared });
`;

/** A spread hides declarations: never narrowed. */
const SPREAD = `const base = { title: { type: String } };
Component({ properties: { ...base, count: { type: Number } } });
`;

const doc = template();
const EXPRESSIONS = ["pageTitle", "total", "flag", "list", "meta", "maybeUser", "maybeList", "nothing"] as const;
const ranges = new Map<string, { start: number; end: number }>(
  EXPRESSIONS.map((expr) => [expr, doc.expr(expr)]),
);

const FRAGMENTS: RequestFragment[] = EXPRESSIONS.map((expr) =>
  fragment(expr, expr, ranges.get(expr)!),
);

function attribute(
  name: string,
  valueKind: RequestComponentAttribute["valueKind"],
  extra: Partial<RequestComponentAttribute> = {},
): RequestComponentAttribute {
  return {
    name,
    valueKind,
    attributeRange: { start: 0, end: Buffer.byteLength(name, "utf8") + 2 },
    ...extra,
  };
}

function mustache(name: string, expr: string): RequestComponentAttribute {
  return attribute(name, "mustache", { exprText: expr, exprRange: ranges.get(expr)! });
}

function usage(tag: string, attributes: RequestComponentAttribute[], componentFileId?: string): RequestComponentUsage {
  return { tag, attributes, ...(componentFileId === undefined ? {} : { componentFileId }) };
}

const USAGES: RequestComponentUsage[] = [
  usage("valid", [mustache("title", "pageTitle"), mustache("count", "total"), mustache("active", "flag"), mustache("score", "pageTitle")], "card.ts"),
  usage("valid-literal", [attribute("title", "literal")], "card.ts"),
  usage("valid-boolean", [attribute("active", "boolean")], "card.ts"),
  usage("bad-mustache", [mustache("title", "total")], "card.ts"),
  usage("bad-union", [mustache("score", "flag")], "card.ts"),
  usage("bad-literal", [attribute("count", "literal")], "card.ts"),
  usage("bad-boolean", [attribute("count", "boolean")], "card.ts"),
  usage("any-prop", [mustache("loose", "total")], "card.ts"),
  usage("function-prop", [mustache("handler", "pageTitle")], "card.ts"),
  usage("array-ok", [mustache("items", "list")], "card.ts"),
  usage("array-bad", [mustache("items", "pageTitle")], "card.ts"),
  usage("object-ok", [mustache("meta", "meta")], "card.ts"),
  usage("object-bad", [mustache("meta", "total")], "card.ts"),
  usage("undeclared", [mustache("title", "pageTitle"), attribute("class", "literal"), mustache("unknown", "total")], "card.ts"),
  usage("mixed", [attribute("title", "mixed", { exprText: "pageTitle", exprRange: ranges.get("pageTitle")! })], "card.ts"),
  usage("missing-component", [mustache("title", "pageTitle")], "missing.ts"),
  usage("no-properties", [mustache("title", "pageTitle")], "plain.ts"),
  usage("incomplete-properties", [mustache("title", "pageTitle")], "spread.ts"),
  usage("no-file-id", [mustache("title", "pageTitle")]),
  usage("no-match", [attribute("title", "mustache", { exprText: "total", exprRange: { start: 9999, end: 10005 } })], "card.ts"),
];

function requestWith(usages: RequestComponentUsage[], fragments: RequestFragment[] = FRAGMENTS) {
  return analyzeRequest({
    script: PAGE,
    extraFiles: [
      { path: "card.ts", text: CARD },
      { path: "plain.ts", text: PLAIN },
      { path: "spread.ts", text: SPREAD },
    ],
    fragments,
    components: usages,
  });
}

/** Run the collector directly (the response does not expose `skipped`). */
function componentOutcome(request: AnalyzeRequest) {
  const view = buildView(request, config);
  if (!view.ok) throw new Error(view.reason);
  const project = new VirtualProject({
    rootDir: BACKEND_DIR,
    compilerOptions: toCompilerOptions(request.project.compilerOptions),
    files: request.files.map((file) => ({
      path: path.resolve(request.project.projectRoot, file.path),
      text: file.text,
    })),
    readPolicy: defaultReadPolicy(BACKEND_DIR),
  });
  for (const file of view.build.files) project.set(file.path, file.text);
  return collectComponentDiagnostics({ request, build: view.build, project });
}

test("component bindings produce per-tag diagnostics and count skipped checks", () => {
  const result = analyze(requestWith(USAGES), config);
  assert.equal(result.ok, true);
  if (!result.ok) return;

  const results = new Map((result.response.components ?? []).map((entry) => [entry.tag, entry.diagnostics]));
  const diagnosticsOf = (tag: string) => results.get(tag) ?? [];

  assert.deepEqual(diagnosticsOf("valid"), []);
  assert.deepEqual(diagnosticsOf("valid-literal"), []);
  assert.deepEqual(diagnosticsOf("valid-boolean"), []);
  assert.deepEqual(diagnosticsOf("undeclared"), [], "class/style/undeclared attributes are not properties");

  const mismatch = diagnosticsOf("bad-mustache");
  assert.equal(mismatch.length, 1);
  assert.deepEqual(mismatch[0]?.range, ranges.get("total"));
  assert.match(mismatch[0]?.message ?? "", /component property "title" expects string, got number/);
  assert.match(mismatch[0]?.message ?? "", /bound expression "total"/);

  const union = diagnosticsOf("bad-union");
  assert.equal(union.length, 1);
  assert.match(union[0]?.message ?? "", /expects string \| number, got boolean/);

  assert.match(diagnosticsOf("bad-literal")[0]?.message ?? "", /expects number, got string literal/);
  assert.deepEqual(diagnosticsOf("bad-literal")[0]?.range, attribute("count", "literal").attributeRange);
  assert.match(diagnosticsOf("bad-boolean")[0]?.message ?? "", /expects number, got boolean literal/);

  // Array/Object are structural checks, not a permanent skip.
  assert.deepEqual(diagnosticsOf("array-ok"), []);
  assert.deepEqual(diagnosticsOf("object-ok"), []);
  assert.match(diagnosticsOf("array-bad")[0]?.message ?? "", /expects any\[\], got string/);
  assert.match(diagnosticsOf("object-bad")[0]?.message ?? "", /expects Record<string, any>, got number/);

  for (const skipped of ["any-prop", "function-prop", "mixed", "missing-component", "no-properties", "incomplete-properties", "no-file-id", "no-match"]) {
    assert.equal(results.has(skipped), false, `${skipped} must be skipped, not reported as clean`);
  }

  assert.equal(result.states.diagnostics.status, "partial");
  // The reason keeps its stable prefix and quotes up to three skipped pairs.
  assert.equal(
    result.states.diagnostics.reason,
    "some component bindings could not be resolved and were not checked " +
      "(any-prop.loose, function-prop.handler, mixed.title)",
  );
});

test("a fully checkable component request stays Supported", () => {
  const request = requestWith([
    usage("valid", [mustache("title", "pageTitle"), mustache("count", "total"), mustache("items", "list")], "card.ts"),
  ]);
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.states.diagnostics.status, "supported");
  assert.deepEqual(result.response.components, [{ tag: "valid", diagnostics: [] }]);
  assert.deepEqual(result.probeProblems, []);
  assert.equal(componentOutcome(request).skipped, 0);
});

test("literal bindings are checked even when the template has no mustache", () => {
  const component = `Component({ properties: { count: { type: Number } } });\n`;
  const request = analyzeRequest({
    script: PAGE,
    extraFiles: [{ path: "badge.ts", text: component }],
    fragments: [],
    components: [
      usage("badge", [attribute("count", "literal", { attributeRange: { start: 40, end: 52 } })], "badge.ts"),
    ],
  });
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.deepEqual(result.mounts, [], "no mustache, no mount");

  const badge = result.response.components?.find((entry) => entry.tag === "badge");
  assert.equal(badge?.diagnostics.length, 1);
  assert.deepEqual(badge?.diagnostics[0]?.range, { start: 40, end: 52 });
  assert.match(badge?.diagnostics[0]?.message ?? "", /expects number, got string literal/);
  assert.equal(result.states.diagnostics.status, "supported", "the literal check ran to completion");
  assert.equal(componentOutcome(request).skipped, 0);
});

test("optional data members may be absent for Array/Object bindings", () => {
  const request = requestWith([
    usage("optional-object", [mustache("meta", "maybeUser")], "card.ts"),
    usage("optional-array", [mustache("items", "maybeList")], "card.ts"),
    usage("absent", [mustache("count", "nothing")], "card.ts"),
    usage("optional-mismatch", [mustache("title", "maybeUser")], "card.ts"),
  ]);
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;

  const results = new Map((result.response.components ?? []).map((entry) => [entry.tag, entry.diagnostics]));
  assert.deepEqual(results.get("optional-object"), [], "`User | undefined` satisfies Object");
  assert.deepEqual(results.get("optional-array"), [], "`Item[] | undefined` satisfies Array");
  assert.deepEqual(results.get("absent"), [], "a pure `undefined` binding is absent, not a mismatch");

  const mismatch = results.get("optional-mismatch");
  assert.equal(mismatch?.length, 1, "a real mismatch through an optional member is still reported");
  assert.match(mismatch[0]?.message ?? "", /expects string, got User \| undefined/);
  assert.equal(componentOutcome(request).skipped, 0);
});

test("a component with no properties member is skipped, not guessed", () => {
  const noProperties = `Component({ data: { state: 'idle' } });\n`;
  const result = analyze(
    analyzeRequest({
      script: PAGE,
      extraFiles: [{ path: "bare.ts", text: noProperties }],
      fragments: FRAGMENTS,
      components: [usage("bare", [mustache("title", "pageTitle")], "bare.ts")],
    }),
    config,
  );
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.response.components, undefined);
  assert.equal(result.states.diagnostics.status, "partial");
});

test("component diagnostics never leak a file path", () => {
  const result = analyze(requestWith(USAGES), config);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  for (const entry of result.response.components ?? []) {
    for (const diagnostic of entry.diagnostics) {
      assert.ok(!diagnostic.message.includes("/"), `path-free message: ${diagnostic.message}`);
    }
  }
});

test("symbol-bridge component messages are path-free as well", () => {
  const request = requestWith([usage("bad", [mustache("title", "total")], "card.ts")]);
  const result = analyze(request, { backendDir: BACKEND_DIR, layoutId: "symbol-bridge" });
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const messages = (result.response.components ?? []).flatMap((entry) =>
    entry.diagnostics.map((diagnostic) => diagnostic.message),
  );
  assert.equal(messages.length, 1);
  assert.ok(!messages[0]!.includes("/"), `path-free message: ${messages[0]}`);
  assert.match(messages[0]!, /component property "title" expects string, got number/);
});

test("behavior properties and kebab/camel bindings are checked", () => {
  const component = `Component({
    behaviors: [Behavior({
      properties: {
        cardTitle: { type: String },
        score: { type: String, optionalTypes: [Number], required: true, value: 1, observer() {} },
        loose: { type: null },
        items: { type: Array },
        defaulted: { type: Number, value: "not-a-number" },
      },
    })],
  });\n`;
  const request = analyzeRequest({
    script: PAGE,
    extraFiles: [{ path: "behavior-card.ts", text: component }],
    fragments: FRAGMENTS,
    components: [
      usage("behavior-ok", [mustache("card-title", "pageTitle"), mustache("score", "total"), mustache("items", "list"), mustache("defaulted", "total"), mustache("unknown", "total")], "behavior-card.ts"),
      usage("behavior-bad", [mustache("card-title", "total"), mustache("score", "flag"), attribute("score", "literal"), attribute("defaulted", "literal")], "behavior-card.ts"),
      usage("behavior-loose", [mustache("loose", "total")], "behavior-card.ts"),
      // Missing required attributes are not diagnosed by the existing
      // strategy; this pins that decision instead of inventing a diagnostic.
      usage("behavior-missing-required", [], "behavior-card.ts"),
    ],
  });
  const outcome = componentOutcome(request);
  const results = new Map(outcome.results.map((entry) => [entry.tag, entry.diagnostics]));
  assert.deepEqual(results.get("behavior-ok"), []);
  const bad = results.get("behavior-bad") ?? [];
  assert.equal(bad.length, 3);
  assert.match(bad[0]?.message ?? "", /component property "card-title" expects string, got number/);
  assert.match(bad[1]?.message ?? "", /component property "score" expects string \| number, got boolean/);
  assert.match(bad[2]?.message ?? "", /component property "defaulted" expects number, got string literal/);
  assert.equal(results.has("behavior-loose"), false);
  assert.equal(results.has("behavior-missing-required"), false);
  assert.equal(outcome.skipped, 1);
  assert.deepEqual(outcome.skippedDetails, ["behavior-loose.loose"]);
});

function componentRequest(options: {
  script: string;
  fragments: Array<[string, string]>;
  extraFiles?: TestFile[];
  jsCheck?: "inherit" | "pragma" | "off";
  checkJs?: boolean;
  scriptPath?: string;
}) {
  return analyzeRequest({
    script: options.script,
    scriptPath: options.scriptPath ?? "index.ts",
    kind: "component",
    ...(options.extraFiles ? { extraFiles: options.extraFiles } : {}),
    ...(options.jsCheck ? { jsCheck: options.jsCheck } : {}),
    ...(options.checkJs ? { checkJs: options.checkJs } : {}),
    fragments: options.fragments.map(([fragmentId, exprText]) =>
      fragment(fragmentId, exprText, { start: 0, end: Buffer.byteLength(exprText, "utf8") }, { root: "component" }),
    ),
  });
}

function hoverOf(result: ReturnType<typeof analyze>, fragmentId: string) {
  if (!result.ok) throw new Error(result.reason);
  return result.response.fragments.find((entry) => entry.fragmentId === fragmentId);
}

test("inline Behavior data and properties join the component instance type", () => {
  const request = componentRequest({
    script: `Component({
      behaviors: [Behavior({
        data: {
          /** 共享计数 */
          sharedCount: 3,
          nested: { label: "x" },
        },
        properties: { cardTitle: { type: String } },
      })],
      data: { own: "own" },
      properties: { ownProp: { type: Number } },
    });`,
    fragments: [
      ["count", "sharedCount"],
      ["nested", "nested.label"],
      ["cardTitle", "cardTitle"],
      ["ownProp", "ownProp"],
    ],
  });
  const result = analyze(request, config);
  const count = hoverOf(result, "count");
  assert.equal(count?.hover?.typeOnly, "number", JSON.stringify(count));
  assert.equal(count?.hover?.documentation, "共享计数");
  assert.deepEqual(count?.diagnostics, []);
  assert.equal(count?.definition?.kind, "generated");
  assert.deepEqual(count?.definition?.range, {
    start: Buffer.byteLength(
      request.files[0]!.text.slice(0, request.files[0]!.text.indexOf("sharedCount")),
      "utf8",
    ),
    end:
      Buffer.byteLength(
        request.files[0]!.text.slice(0, request.files[0]!.text.indexOf("sharedCount")),
        "utf8",
      ) + "sharedCount".length,
  });
  const nested = hoverOf(result, "nested");
  assert.equal(nested?.hover?.typeOnly, "string");
  assert.deepEqual(nested?.diagnostics, []);
  assert.equal(hoverOf(result, "cardTitle")?.hover?.typeOnly, "string");
  assert.equal(hoverOf(result, "ownProp")?.hover?.typeOnly, "number");
  assert.deepEqual(hoverOf(result, "ownProp")?.diagnostics, []);
});

test("component's own data wins over a behavior field with the same name", () => {
  const request = componentRequest({
    script: `Component({
      behaviors: [Behavior({ data: { title: 1 } })],
      data: { title: "own" },
    });`,
    fragments: [["title", "title"]],
  });
  const title = hoverOf(analyze(request, config), "title");
  assert.equal(title?.hover?.typeOnly, "string");
  assert.deepEqual(title?.diagnostics, []);
});

test("imported behavior default export contributes data and properties", () => {
  const behavior = `const shared = Behavior({
    data: { importedCount: 7 },
    properties: { importedTitle: { type: String, optionalTypes: [Number] } },
  });
  export default shared;`;
  const request = componentRequest({
    script: `import shared from "./shared";
    Component({ behaviors: [shared], data: {} });`,
    extraFiles: [{ path: "shared.ts", text: behavior }],
    fragments: [
      ["count", "importedCount"],
      ["title", "importedTitle"],
    ],
  });
  const result = analyze(request, config);
  assert.equal(hoverOf(result, "count")?.hover?.typeOnly, "number");
  assert.equal(hoverOf(result, "title")?.hover?.typeOnly, "string | number");
  assert.deepEqual(hoverOf(result, "count")?.diagnostics, []);
  assert.deepEqual(hoverOf(result, "title")?.diagnostics, []);
  const definition = hoverOf(result, "count")?.definition;
  assert.equal(definition?.kind, "generated");
  assert.equal(definition?.fileId, "shared.ts");
  const propertyDefinition = hoverOf(result, "title")?.definition;
  assert.equal(propertyDefinition?.kind, "generated");
  assert.equal(propertyDefinition?.fileId, "shared.ts");
  const behaviorText = behavior;
  assert.equal(
    behaviorText.slice(
      propertyDefinition?.range?.start ?? 0,
      propertyDefinition?.range?.end ?? 0,
    ),
    "importedTitle",
  );
});

test("nested behaviors apply before their owner and later behaviors win", () => {
  const request = componentRequest({
    script: `Component({
      behaviors: [
        Behavior({ data: { later: "later", nested: { value: 1 } } }),
        Behavior({
          behaviors: [Behavior({ data: { nested: { value: "inner" }, onlyNested: 1 } })],
          data: { later: 2 },
        }),
      ],
      data: {},
    });`,
    fragments: [
      ["later", "later"],
      ["nested", "nested.value"],
      ["only", "onlyNested"],
    ],
  });
  const result = analyze(request, config);
  assert.equal(hoverOf(result, "later")?.hover?.typeOnly, "number");
  assert.equal(hoverOf(result, "nested")?.hover?.typeOnly, "string");
  assert.equal(hoverOf(result, "only")?.hover?.typeOnly, "number");
});

test("behavior cycles and factory calls degrade with a reason, not a crash", () => {
  const a = `import b from "./b"; export default Behavior({ behaviors: [b], data: { fromA: 1 } });`;
  const b = `import a from "./a"; export default Behavior({ behaviors: [a], data: { fromB: 2 } });`;
  const request = componentRequest({
    script: `import a from "./a";
    Component({ behaviors: [a, createBehavior()], data: {} });`,
    extraFiles: [
      { path: "a.ts", text: a },
      { path: "b.ts", text: b },
    ],
    fragments: [["a", "fromA"]],
  });
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const fragment = hoverOf(result, "a");
  assert.equal(fragment?.hover?.typeOnly, "number");
  const reason = result.states.hover.reason ?? "";
  assert.match(reason, /behavior/i, reason);
  assert.match(reason, /dynamic|could not|cycle|not part/i, reason);
});

test("spread and dynamic data members are reported and stay any", () => {
  const request = componentRequest({
    script: `const extra = { value: 1 };
    Component({
      behaviors: [Behavior({ data: { ...extra, dynamic: makeValue() } })],
      data: {},
    });`,
    fragments: [["dynamic", "dynamic"]],
  });
  const result = analyze(request, config);
  if (!result.ok) throw new Error(result.reason);
  const dynamic = hoverOf(result, "dynamic");
  assert.equal(dynamic?.hover?.typeOnly, "any");
  assert.match(result.states.hover.reason ?? "", /behavior/i);
});

test("behavior type inference is conservative for literals", () => {
  const parsed = ts.createSourceFile("x.ts", "const value = 1;", ts.ScriptTarget.Latest, true);
  const initializer = (parsed.statements[0] as ts.VariableStatement).declarationList
    .declarations[0]!.initializer!;
  assert.deepEqual(typeOfInitializer(initializer, parsed), { text: "number", exact: true });
});

test("named behavior imports and re-export aliases resolve statically", () => {
  const behavior = `const implementation = Behavior({ data: { namedCount: 4 } });
  export { implementation as shared };`;
  const request = componentRequest({
    script: `import { shared } from "./shared";
    Component({ behaviors: [shared], data: {} });`,
    extraFiles: [{ path: "shared.ts", text: behavior }],
    fragments: [["count", "namedCount"]],
  });
  const result = analyze(request, config);
  assert.equal(hoverOf(result, "count")?.hover?.typeOnly, "number");
  assert.equal(
    result.ok && result.states.hover.status,
    "supported",
    result.ok ? result.states.hover.reason : "analysis unavailable",
  );
});

test("a behavior data member initialized from a local const keeps its checked type", () => {
  const request = componentRequest({
    script: `const initial = { label: "x" };
    Component({ behaviors: [Behavior({ data: { fromConst: initial } })], data: {} });`,
    fragments: [["field", "fromConst"]],
  });
  const field = hoverOf(analyze(request, config), "field");
  assert.match(field?.hover?.typeOnly ?? "", /label/, JSON.stringify(field));
  assert.deepEqual(field?.diagnostics, []);
});

test("JavaScript behavior data uses JSDoc casts under checkJs", () => {
  const request = componentRequest({
    scriptPath: "index.js",
    checkJs: true,
    script: `Component({
      behaviors: [Behavior({ data: {
        /** js count */
        jsCount: 2,
      } })],
      data: {},
    });`,
    fragments: [["count", "jsCount"]],
  });
  const result = analyze(request, config);
  const count = hoverOf(result, "count");
  assert.equal(count?.hover?.typeOnly, "number");
  assert.equal(count?.hover?.documentation, "js count");
  assert.deepEqual(count?.diagnostics, []);
});

test("unresolved behavior does not fabricate 2339 in either layout", () => {
  for (const layoutId of ["in-place-shadow", "symbol-bridge"] as const) {
    const request = componentRequest({
      script: `Component({ behaviors: [factory()], data: {} });`,
      fragments: [["unknown", "x"]],
    });
    const result = analyze(request, { ...config, layoutId });
    assert.equal(result.ok, true);
    if (!result.ok) return;
    assert.equal(result.build.behaviorIncomplete, true, layoutId);
    assert.equal(result.build.buildStatus, "partial", layoutId);
    assert.equal(result.states.diagnostics.status, "partial", layoutId);
    const fragment = result.response.fragments.find((entry) => entry.fragmentId === "unknown");
    assert.deepEqual(
      (fragment?.diagnostics ?? []).filter((diagnostic) => diagnostic.code === 2339),
      [],
      `${layoutId}: ${JSON.stringify(fragment)}`,
    );
  }
});

test("CommonJS require behavior resolves statically", () => {
  const behavior = `const shared = Behavior({ data: { importedCount: 7 } });
  module.exports = shared;`;
  const request = componentRequest({
    script: `const shared = require("./shared");
    Component({ behaviors: [shared], data: {} });`,
    extraFiles: [{ path: "shared.js", text: behavior }],
    fragments: [["count", "importedCount"]],
  });
  const result = analyze(request, config);
  const count = hoverOf(result, "count");
  assert.equal(count?.hover?.typeOnly, "number");
  assert.deepEqual(count?.diagnostics, []);
  assert.equal(result.ok && result.build.behaviorIncomplete, false);
});

test("non-literal behavior data/properties mark the view Partial", () => {
  const request = componentRequest({
    script: `const shared = { known: 1 };
    const sharedProps = { title: { type: String } };
    Component({ behaviors: [Behavior({ data: shared, properties: sharedProps })], data: {} });`,
    fragments: [["unknown", "missingField"]],
  });
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.build.behaviorIncomplete, true);
  assert.equal(result.build.buildStatus, "partial");
  assert.match(result.build.reason ?? "", /non-object-literal/);
  assert.deepEqual(
    (hoverOf(result, "unknown")?.diagnostics ?? []).filter(
      (diagnostic) => diagnostic.code === 2339,
    ),
    [],
  );
});

test("behavior data keys that cannot be probe locals never break the probe", () => {
  const request = componentRequest({
    script: `Component({
      behaviors: [Behavior({ data: { default: 1, eval: 2, class: 3, __wxml_root: 4, ok: 5 } })],
      data: {},
    });`,
    fragments: [
      ["unknown", "other"],
      ["ok", "ok"],
    ],
  });
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.deepEqual(result.probeProblems, []);
  assert.equal(result.build.behaviorIncomplete, true);
  assert.match(result.build.reason ?? "", /cannot be used as a template root/);
  assert.deepEqual(
    (hoverOf(result, "unknown")?.diagnostics ?? []).filter(
      (diagnostic) => diagnostic.code === 2339,
    ),
    [],
  );
  assert.equal(hoverOf(result, "ok")?.hover?.typeOnly, "number");
});

test("a Page and Component in one file use the selected registration's behaviors", () => {
  const request = analyzeRequest({
    script: `Page({ data: { pageOnly: "p" } });
    Component({ behaviors: [Behavior({ data: { leaked: 1 } })], data: {} });`,
    fragments: [
      fragment("page", "pageOnly", { start: 0, end: 8 }),
      fragment("leaked", "leaked", { start: 0, end: 6 }),
    ],
  });
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.registration.kind, "page");
  assert.equal(result.build.behaviorIncomplete, false);
  assert.equal(
    result.response.fragments.find((entry) => entry.fragmentId === "page")?.hover?.typeOnly,
    "string",
  );
  const leaked = result.response.fragments.find((entry) => entry.fragmentId === "leaked");
  assert.notEqual(leaked?.hover?.typeOnly, "number", JSON.stringify(leaked));
});

test("an unresolved external behavior keeps the component's own property checks", () => {
  const component = `import { externalBehavior } from "some-lib";
  Component({ behaviors: [externalBehavior], properties: { title: { type: String } } });`;
  const request = analyzeRequest({
    script: PAGE,
    extraFiles: [{ path: "external.ts", text: component }],
    fragments: FRAGMENTS,
    components: [
      usage("bad", [mustache("title", "total")], "external.ts"),
    ],
  });
  const outcome = componentOutcome(request);
  const diagnostic = outcome.results.find((entry) => entry.tag === "bad")?.diagnostics[0];
  assert.match(
    diagnostic?.message ?? "",
    /component property "title" expects string, got number/,
  );
  assert.ok(outcome.skipped >= 1);
  assert.ok(outcome.skippedDetails.includes("bad.behavior"));
});
