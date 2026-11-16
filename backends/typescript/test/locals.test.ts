/**
 * Typed `wx:for` locals (AC-005).
 *
 * The end-to-end tests drive `analyze()` with a real script and structured
 * bindings: hover types come from TypeScript's own inference over the generated
 * `__wxml_item`/`__wxml_index` declarations, so a passing test means the
 * generated view is genuinely typed (not just formatted as expected).
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import { analyze } from "../src/analysis.ts";
import { BACKEND_DIR } from "../src/env.ts";
import {
  LOCAL_HELPERS_TS,
  groupLocalLevels,
  localHelpers,
  renderLocals,
  type RewriteSource,
} from "../src/layouts/locals.ts";
import type { AnalyzeResponse, RequestLocalBinding } from "../src/protocol.ts";
import { rewriteExpression } from "../src/rewrite.ts";
import { analyzeRequest, fragment, fragmentDiagnostics, template } from "./support.ts";

const config = { backendDir: BACKEND_DIR };

/** The layout rewriter (same try/catch contract as the layouts). */
const rewriteSource: RewriteSource = (exprText, outerNames) => {
  try {
    return rewriteExpression(exprText, undefined, {
      rootPrefix: "__wxml_root.",
      propsPrefix: "__wxml_props_0.",
      propertyOverlay: new Set(),
      dataOverlay: new Set(),
      locals: outerNames,
    }).text;
  } catch {
    return undefined;
  }
};

function binding(name: string, role: "item" | "index", sourceExpr: string): RequestLocalBinding {
  return { name, role, sourceExpr, sourceRange: { start: 0, end: Buffer.byteLength(sourceExpr, "utf8") } };
}

/* ------------------------------------------------------------- unit: render */

test("renderLocals emits nested __wxml_item/__wxml_index declarations", () => {
  const rendered = renderLocals(
    [binding("item", "item", "users"), binding("index", "index", "users")],
    "    ",
    "ts",
    rewriteSource,
  );
  assert.equal(rendered.unresolved, false);
  assert.equal(
    rendered.text,
    "    {\n      const item = __wxml_item(__wxml_root.users);\n      const index = __wxml_index(__wxml_root.users);\n",
  );
  assert.equal(rendered.close, "    }\n");
});

test("nested levels indent, and closers are innermost first", () => {
  const rendered = renderLocals(
    [
      binding("group", "item", "groups"),
      binding("groupIndex", "index", "groups"),
      binding("member", "item", "group.members"),
    ],
    "  ",
    "ts",
    rewriteSource,
  );
  assert.equal(rendered.unresolved, false);
  assert.equal(
    rendered.text,
    [
      "  {",
      "    const group = __wxml_item(__wxml_root.groups);",
      "    const groupIndex = __wxml_index(__wxml_root.groups);",
      "    {",
      "      const member = __wxml_item(group.members);",
      "",
    ].join("\n"),
  );
  assert.equal(rendered.close, "    }\n  }\n");
});

test("a same-name nested chain captures the outer binding (no TDZ self-reference)", () => {
  // The chain is the full nesting: outer item/index, then an inner item/index
  // that reuse the same names and read the outer `item` in their source.
  const rendered = renderLocals(
    [
      binding("item", "item", "groups"),
      binding("index", "index", "groups"),
      binding("item", "item", "item.members"),
      binding("index", "index", "item.members"),
    ],
    "",
    "ts",
    rewriteSource,
  );
  assert.equal(rendered.unresolved, false);
  assert.equal(
    rendered.text,
    [
      "{",
      "  const item = __wxml_item(__wxml_root.groups);",
      "  const index = __wxml_index(__wxml_root.groups);",
      "  const __wxml_capture_item_0 = item;",
      "  {",
      "    const item = __wxml_item(__wxml_capture_item_0.members);",
      "    const index = __wxml_index(__wxml_capture_item_0.members);",
      "",
    ].join("\n"),
  );
  assert.equal(rendered.close, "  }\n}\n");
  assert.ok(
    !/const item = __wxml_item\(item\.members\)/.test(rendered.text),
    "the inner declaration must not read the name it declares",
  );
});

test("an unparseable source expression falls back to any and reports unresolved", () => {
  const rendered = renderLocals([binding("item", "item", "users[")], "", "ts", rewriteSource);
  assert.equal(rendered.unresolved, true);
  assert.match(rendered.text, /const item = __wxml_item\(undefined as any\);/);
});

test("JavaScript mode binds locals as any without helper types", () => {
  const rendered = renderLocals([binding("item", "item", "users")], "", "js", rewriteSource);
  assert.equal(rendered.unresolved, true, "an `any` binding is a coverage gap");
  assert.equal(rendered.text, "{\n  const item = /** @type {any} */ (undefined);\n");
  assert.equal(localHelpers("js"), "");
  assert.equal(localHelpers("ts"), LOCAL_HELPERS_TS);
});

test("invalid binding names are ignored defensively", () => {
  const rendered = renderLocals([binding("class", "item", "users")], "", "ts", rewriteSource);
  assert.equal(rendered.text, "");
  assert.equal(rendered.close, "");
});

test("levels are grouped by array order, not by source expression", () => {
  const levels = groupLocalLevels([
    binding("item", "item", "users"),
    binding("index", "index", "users"),
    binding("member", "item", "users"),
    binding("memberIndex", "index", "users"),
  ]);
  assert.equal(levels.length, 2);
  assert.equal(levels[0]?.item?.name, "item");
  assert.equal(levels[0]?.index?.name, "index");
  assert.equal(levels[1]?.item?.name, "member");
});

/* ------------------------------------------------------- end-to-end: types */

const TYPED_SCRIPT = `interface User {
  name: string;
  title: string;
}
interface Group {
  label: string;
  members: User[];
}
Page({
  data: {
    users: [{ name: 'Ada', title: 'Lead' }] as User[],
    groups: [{ label: 'g', members: [{ name: 'Bob', title: 'Dev' }] }] as Group[],
    tuple: ['a', 1] as [string, number],
    readonlyUsers: [] as readonly User[],
    groupMap: {} as Record<string, Group>,
    anything: undefined as any,
  },
});
`;

interface LocalCase {
  id: string;
  expr: string;
  bindings: RequestLocalBinding[];
  expectType: string;
  expectQuality?: "known" | "any";
}

const LOCAL_CASES: LocalCase[] = [
  {
    id: "array-item",
    expr: "item.name",
    bindings: [binding("item", "item", "users")],
    expectType: "string",
  },
  {
    id: "array-index",
    expr: "index",
    bindings: [binding("item", "item", "users"), binding("index", "index", "users")],
    expectType: "number",
  },
  {
    id: "tuple-item",
    expr: "item",
    bindings: [binding("item", "item", "tuple")],
    expectType: "string | number",
  },
  {
    id: "readonly-item",
    expr: "item.title",
    bindings: [binding("item", "item", "readonlyUsers")],
    expectType: "string",
  },
  {
    id: "record-item",
    expr: "item.members",
    bindings: [binding("item", "item", "groupMap")],
    expectType: "User[]",
  },
  {
    id: "record-index",
    expr: "index",
    bindings: [binding("index", "index", "groupMap")],
    expectType: "string",
  },
  {
    id: "nested-inner-item",
    expr: "member.name",
    bindings: [
      binding("group", "item", "groups"),
      binding("groupIndex", "index", "groups"),
      binding("member", "item", "group.members"),
      binding("memberIndex", "index", "group.members"),
    ],
    expectType: "string",
  },
  {
    id: "outer-only",
    expr: "item.members",
    bindings: [binding("item", "item", "groups")],
    expectType: "User[]",
  },
  {
    id: "shadowed-inner",
    expr: "item.name",
    bindings: [
      binding("item", "item", "groups"),
      binding("index", "index", "groups"),
      binding("item", "item", "item.members"),
      binding("index", "index", "item.members"),
    ],
    expectType: "string",
  },
  {
    id: "any-item",
    expr: "item",
    bindings: [binding("item", "item", "anything")],
    expectType: "any",
    expectQuality: "any",
  },
];

test("typed locals resolve element and key types end-to-end", () => {
  const doc = template();
  const fragments = LOCAL_CASES.map((localCase) =>
    fragment(localCase.id, localCase.expr, doc.expr(localCase.expr), { bindings: localCase.bindings }),
  );
  const request = analyzeRequest({ script: TYPED_SCRIPT, fragments });
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;

  for (const localCase of LOCAL_CASES) {
    const observation: AnalyzeResponse["fragments"][number]["hover"] = result.response.fragments.find(
      (entry) => entry.fragmentId === localCase.id,
    )?.hover;
    assert.equal(observation?.typeOnly, localCase.expectType, `type of ${localCase.id}`);
    if (localCase.expectQuality) {
      assert.equal(observation?.quality, localCase.expectQuality, `quality of ${localCase.id}`);
    }
    assert.deepEqual(fragmentDiagnostics(result.response, localCase.id), [], `diagnostics of ${localCase.id}`);
  }

  assert.deepEqual(result.probeProblems, [], "typed locals must not break the probe");
  assert.equal(result.states.hover.status, "supported", "typed locals no longer downgrade hover");
  assert.equal(result.states.diagnostics.status, "supported", "typed locals no longer downgrade diagnostics");
  assert.ok(!/template locals are bound as any/.test(result.states.hover.reason ?? ""));
});

test("an inner loop that shadows the outer name does not hit the TDZ", () => {
  const doc = template();
  const expr = "item.name";
  const request = analyzeRequest({
    script: TYPED_SCRIPT,
    fragments: [
      fragment("shadow", expr, doc.expr(expr), {
        bindings: [
          binding("item", "item", "groups"),
          binding("index", "index", "groups"),
          binding("item", "item", "item.members"),
          binding("index", "index", "item.members"),
        ],
      }),
    ],
  });
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.deepEqual(result.probeProblems, []);
  assert.deepEqual(result.diagnostics, [], "a missing capture would surface as a block-scoped-variable error");
  assert.equal(result.response.fragments[0]?.hover?.typeOnly, "string");
});

test("JavaScript projects bind locals as any and report Partial", () => {
  const doc = template();
  const expr = "item.name";
  const request = analyzeRequest({
    script: "Page({ data: { users: [{ name: 'Ada' }] } });\n",
    scriptPath: "index.js",
    checkJs: true,
    fragments: [
      fragment("js-local", expr, doc.expr(expr), { bindings: [binding("item", "item", "users")] }),
    ],
  });
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  const hover = result.response.fragments[0]?.hover;
  assert.equal(hover?.quality, "any", "JavaScript has no helper types; the binding stays any");
  assert.deepEqual(result.diagnostics, []);
  assert.equal(result.states.diagnostics.status, "partial", "an `any` binding is a coverage gap, not a clean check");
  assert.match(result.states.diagnostics.reason ?? "", /could not be resolved and were not checked/);
  assert.equal(result.build.unresolvedBindings, 1);
});

test("an extremely deep `wx:for` source degrades instead of overflowing the stack", () => {
  // A left-associative chain of 40k terms has a 40k-deep AST: the identifier walk
  // must stop at its budget and the binding must fall back to `any`.
  const deep = Array.from({ length: 40_000 }, () => "a").join("+");
  const rendered = renderLocals([binding("item", "item", deep)], "", "ts", rewriteSource);
  assert.equal(rendered.unresolved, true);
  assert.match(rendered.text, /const item = __wxml_item\(undefined as any\);/);
});

test("a `locals`-only request still degrades to Partial with a typed reason", () => {
  const doc = template();
  const expr = "item.name";
  const request = analyzeRequest({
    script: TYPED_SCRIPT,
    fragments: [fragment("legacy", expr, doc.expr(expr), { locals: ["item"] })],
  });
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.states.diagnostics.status, "partial");
  assert.match(result.states.diagnostics.reason ?? "", /could not be resolved and were not checked/);
  assert.deepEqual(fragmentDiagnostics(result.response, "legacy"), []);
});
