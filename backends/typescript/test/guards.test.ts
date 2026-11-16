/**
 * Conditional narrowing (`wx:if` / `wx:elif` / `wx:else`, AC-006).
 *
 * The generated probe wraps each mount in real `if`/`else if`/`else` blocks, so
 * the assertions below are a direct test of TypeScript control-flow analysis:
 * a wrong-branch mount must produce a diagnostic, a correct one must not.
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import { analyze } from "../src/analysis.ts";
import { BACKEND_DIR } from "../src/env.ts";
import { renderGuards } from "../src/layouts/guards.ts";
import type { RequestGuardChain } from "../src/protocol.ts";
import { analyzeRequest, byteIndexOf, fragment, fragmentDiagnostics, template } from "./support.ts";

const config = { backendDir: BACKEND_DIR };

const SCRIPT = `type State =
  | { kind: 'loading' }
  | { kind: 'ready'; user: { name: string } }
  | { kind: 'error'; code: number };
Page({
  data: {
    state: { kind: 'loading' } as State,
    maybeUser: null as { name: string } | null,
    users: [{ name: 'Ada', visible: true }],
  },
});
`;

function condition(branch: "if" | "elif" | "else", exprText: string) {
  return { branch, exprText, range: { start: 0, end: Buffer.byteLength(exprText, "utf8") } };
}

function chain(conditions: RequestGuardChain["conditions"], selected: number): RequestGuardChain {
  return { conditions, selected };
}

const STATE_CHAIN = chain(
  [condition("if", "state.kind === 'loading'"), condition("elif", "state.kind === 'ready'")],
  0,
);

/* -------------------------------------------------------------- unit: render */

test("renderGuards opens if/elif and closes one block", () => {
  const rendered = renderGuards(chain([condition("if", "a"), condition("elif", "b")], 1), {
    indent: "  ",
    rewriteCondition: (exprText) => `__wxml_root.${exprText}`,
  });
  assert.equal(rendered.unresolved, false);
  assert.equal(rendered.open, "  if (__wxml_root.a) {\n  } else if (__wxml_root.b) {\n");
  assert.equal(rendered.close, "  }\n");
});

test("renderGuards renders the else branch as a trailing `} else {`", () => {
  const rendered = renderGuards(chain([condition("if", "a"), condition("elif", "b")], 2), {
    indent: "",
    rewriteCondition: (exprText) => exprText,
  });
  assert.equal(rendered.open, "if (a) {\n} else if (b) {\n} else {\n");
  assert.equal(rendered.close, "}\n");
});

test("renderGuards never renders a condition after the selected branch", () => {
  const rendered = renderGuards(chain([condition("if", "a"), condition("elif", "b")], 0), {
    indent: "",
    rewriteCondition: (exprText) => exprText,
  });
  assert.equal(rendered.open, "if (a) {\n");
  assert.ok(!rendered.open.includes("b"));
});

test("an unrewritable condition yields no narrowing block", () => {
  const rendered = renderGuards(chain([condition("if", "a")], 0), {
    indent: "",
    rewriteCondition: () => undefined,
  });
  assert.deepEqual(rendered, { open: "", close: "", unresolved: true });
});

/* ------------------------------------------------- end-to-end: narrowing */

test("if/elif/else narrows a discriminated union per branch", () => {
  const doc = template();
  const nameRange = doc.expr("state.user.name");
  const codeRange = doc.expr("state.code");
  const wrongRange = doc.expr("state.user.name");
  const request = analyzeRequest({
    script: SCRIPT,
    fragments: [
      fragment("ready-name", "state.user.name", nameRange, { guards: [chain(STATE_CHAIN.conditions, 1)] }),
      fragment("error-code", "state.code", codeRange, { guards: [chain(STATE_CHAIN.conditions, 2)] }),
      fragment("loading-wrong", "state.user.name", wrongRange, { guards: [chain(STATE_CHAIN.conditions, 0)] }),
    ],
  });
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;

  const hovers = new Map(result.response.fragments.map((entry) => [entry.fragmentId, entry.hover]));
  assert.equal(hovers.get("ready-name")?.typeOnly, "string", "`wx:elif` branch narrows to the ready variant");
  assert.equal(hovers.get("error-code")?.typeOnly, "number", "`wx:else` narrows to the remaining variant");
  assert.deepEqual(fragmentDiagnostics(result.response, "ready-name"), []);
  assert.deepEqual(fragmentDiagnostics(result.response, "error-code"), []);

  const wrong = fragmentDiagnostics(result.response, "loading-wrong");
  assert.equal(wrong.length, 1, "the loading branch has no `user` property");
  assert.deepEqual(wrong[0]?.range, {
    start: wrongRange.start + byteIndexOf("state.user.name", "user"),
    end: wrongRange.start + byteIndexOf("state.user.name", "user") + "user".length,
  });
});

test("a truthiness guard removes null", () => {
  const doc = template();
  const narrowed = doc.expr("maybeUser.name");
  const unnarrowed = doc.expr("maybeUser.name");
  const request = analyzeRequest({
    script: SCRIPT,
    fragments: [
      fragment("narrowed", "maybeUser.name", narrowed, { guards: [chain([condition("if", "maybeUser")], 0)] }),
      fragment("unnarrowed", "maybeUser.name", unnarrowed),
    ],
  });
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;

  const hovers = new Map(result.response.fragments.map((entry) => [entry.fragmentId, entry.hover]));
  assert.equal(hovers.get("narrowed")?.typeOnly, "string");
  assert.deepEqual(fragmentDiagnostics(result.response, "narrowed"), []);
  assert.ok(
    fragmentDiagnostics(result.response, "unnarrowed").length > 0,
    "without the guard the same expression must report the possibly-null access",
  );
});

test("nested chains narrow cumulatively", () => {
  const doc = template();
  const range = doc.expr("maybeUser.name");
  const request = analyzeRequest({
    script: SCRIPT,
    fragments: [
      fragment("nested", "maybeUser.name", range, {
        guards: [
          chain([condition("if", "maybeUser")], 0),
          chain([condition("if", "maybeUser.name")], 0),
        ],
      }),
    ],
  });
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.deepEqual(result.probeProblems, [], "a nested condition must itself type-check in the narrowed scope");
  assert.deepEqual(result.diagnostics, []);
  assert.equal(result.response.fragments[0]?.hover?.typeOnly, "string");
});

test("guard conditions see loop locals", () => {
  const doc = template();
  const range = doc.expr("item.name");
  const request = analyzeRequest({
    script: SCRIPT,
    fragments: [
      fragment("loop-guard", "item.name", range, {
        bindings: [
          {
            name: "item",
            role: "item",
            sourceExpr: "users",
            sourceRange: { start: 0, end: Buffer.byteLength("users", "utf8") },
          },
        ],
        guards: [chain([condition("if", "item.visible")], 0)],
      }),
    ],
  });
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.deepEqual(result.probeProblems, []);
  assert.deepEqual(result.diagnostics, []);
  assert.equal(result.response.fragments[0]?.hover?.typeOnly, "string");
});

test("an unparseable guard does not fabricate a diagnostic and reports Partial", () => {
  const doc = template();
  const range = doc.expr("state.user.name");
  const request = analyzeRequest({
    script: SCRIPT,
    fragments: [fragment("broken", "state.user.name", range, { guards: [chain([condition("if", "state.kind === ")], 0)] })],
  });
  const result = analyze(request, config);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.build.unresolvedGuards, 1);
  assert.equal(result.states.diagnostics.status, "partial");
  assert.match(result.states.diagnostics.reason ?? "", /could not be resolved and were not checked/);
  assert.deepEqual(result.diagnostics, [], "the unnarrowed error is suppressed, not published");
  assert.deepEqual(result.response.fragments[0]?.diagnostics, []);
});
