/**
 * Scenario table for the type-bridge experiment.
 *
 * Every scenario is a real sample project under `samples/`. The WXML text is
 * kept here because the probe only needs expression positions; the real parser
 * belongs to P2 and is out of scope.
 */

import type { ScenarioSpec } from "./types.ts";

const PAGE_BASIC_WXML = `<view class="page">
  <text class="title">{{ title }}</text>
  <text class="name">{{ user.name }}</text>
  <text class="typo">{{ user.nmae }}</text>
</view>
`;

const COMPONENT_WXML = `<view class="card">
  <text class="title">{{ title }}</text>
  <text class="state">{{ state }}</text>
  <text class="subtitle">{{ subtitle }}</text>
</view>
`;

const PAGE_CLOSURE_WXML = `<view class="page">
  <text class="title">{{ config.title }}</text>
  <text class="name">{{ user.name }}</text>
</view>
`;

const JS_JSDOC_WXML = `<view class="page">
  <text class="greeting">{{ greeting }}</text>
  <text class="name">{{ user.name }}</text>
  <text class="typo">{{ user.nmae }}</text>
</view>
`;

export const SCENARIOS: ScenarioSpec[] = [
  {
    id: "page-basic",
    dir: "page-basic",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: PAGE_BASIC_WXML,
    queries: [
      { expr: "title", target: "title", expectType: "string", expectDoc: "页面标题", expectDocSource: "quickinfo" },
      { expr: "user.name", target: "name", expectType: "string", expectDoc: "用户显示名称", expectDocSource: "quickinfo", definition: true, expectDefinition: "script" },
      { expr: "user.nmae", target: "nmae" },
    ],
    expectedDiagnostics: [{ expr: "user.nmae", target: "nmae" }],
    note: "bare Page script, nested data object with field JSDoc",
  },
  {
    id: "page-export-empty",
    dir: "page-export-empty",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: PAGE_BASIC_WXML,
    queries: [
      { expr: "title", target: "title", expectType: "string", expectDoc: "页面标题" },
      { expr: "user.name", target: "name", expectType: "string", expectDoc: "用户显示名称" },
      { expr: "user.nmae", target: "nmae" },
    ],
    expectedDiagnostics: [{ expr: "user.nmae", target: "nmae" }],
    note: "module script (`export {}`)",
  },
  {
    id: "page-cjs",
    dir: "page-cjs",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: PAGE_BASIC_WXML,
    queries: [
      { expr: "title", target: "title", expectType: "string", expectDoc: "页面标题" },
      { expr: "user.name", target: "name", expectType: "string", expectDoc: "用户显示名称" },
      { expr: "user.nmae", target: "nmae" },
    ],
    expectedDiagnostics: [{ expr: "user.nmae", target: "nmae" }],
    note: "CommonJS-shaped script (`module.exports`)",
  },
  {
    id: "page-esm",
    dir: "page-esm",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    extraFiles: ["helper.ts"],
    wxml: PAGE_BASIC_WXML,
    queries: [
      { expr: "title", target: "title", expectType: "string", expectDoc: "页面标题" },
      { expr: "user.name", target: "name", expectType: "string", expectDoc: "用户显示名称" },
      { expr: "user.nmae", target: "nmae" },
    ],
    expectedDiagnostics: [{ expr: "user.nmae", target: "nmae" }],
    note: "ES module page with a relative import",
  },
  {
    id: "page-closure",
    dir: "page-closure",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: PAGE_CLOSURE_WXML,
    queries: [
      { expr: "config.title", target: "title", expectType: "string", expectDoc: "页面标题" },
      { expr: "user.name", target: "name", expectType: "string", expectDoc: "用户显示名称" },
    ],
    expectedDiagnostics: [],
    note: "registration inside a function: local `interface`, local type alias, generic call",
  },
  {
    id: "component-properties",
    dir: "component-properties",
    registration: "component",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: COMPONENT_WXML,
    queries: [
      { expr: "title", target: "title", expectType: "string | number", expectDoc: "标题，支持字符串或数字", expectDocSource: "copied", definition: true, expectDefinition: "generated" },
      { expr: "state", target: "state", expectType: "string", expectDoc: "内部状态" },
      { expr: "subtitle", target: "subtitle" },
    ],
    expectedDiagnostics: [{ expr: "subtitle", target: "subtitle" }],
    note: "`type` + `optionalTypes` union, property JSDoc, undeclared property",
  },
  {
    id: "component-module",
    dir: "component-module",
    registration: "component",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: COMPONENT_WXML,
    queries: [
      { expr: "title", target: "title", expectType: "string | number", expectDoc: "标题，支持字符串或数字" },
      { expr: "state", target: "state", expectType: "string", expectDoc: "内部状态" },
      { expr: "subtitle", target: "subtitle" },
    ],
    expectedDiagnostics: [{ expr: "subtitle", target: "subtitle" }],
    note: "module component with optionalTypes: candidate B cannot see the overlay type from the anchor",
  },
  {
    id: "component-js",
    dir: "component-js",
    registration: "component",
    script: "index.js",
    allowJs: true,
    checkJs: false,
    jsCheck: "pragma",
    wxml: COMPONENT_WXML,
    queries: [
      { expr: "title", target: "title", expectType: "string | number", expectDoc: "标题，支持字符串或数字", expectDocSource: "copied" },
      { expr: "state", target: "state", expectType: "string", expectDoc: "内部状态" },
      { expr: "subtitle", target: "subtitle" },
    ],
    expectedDiagnostics: [{ expr: "subtitle", target: "subtitle" }],
    note: "JavaScript component: properties overlay rendered as JSDoc",
  },
  {
    id: "properties-spread",
    dir: "properties-spread",
    registration: "component",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: COMPONENT_WXML,
    queries: [
      { expr: "title", target: "title", expectType: "string | number" },
      { expr: "state", target: "state", expectType: "string" },
    ],
    expectedDiagnostics: [],
    note: "`properties: { ...base, title }`: declared properties are incomplete, optionalTypes may under-report",
  },
  {
    id: "multi-registration",
    dir: "multi-registration",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: `<view class="page">
  <text class="title">{{ title }}</text>
</view>
`,
    queries: [{ expr: "title", target: "title", expectType: "string", expectDoc: "页面标题" }],
    expectedDiagnostics: [],
    note: "two Page registrations: partial with the candidate count, never a silent first pick",
  },
  {
    id: "js-jsdoc-pragma",
    dir: "js-jsdoc",
    registration: "page",
    script: "index.js",
    allowJs: true,
    checkJs: false,
    jsCheck: "pragma",
    wxml: JS_JSDOC_WXML,
    queries: [
      { expr: "greeting", target: "greeting", expectType: "string", expectDoc: "问候语" },
      { expr: "user.name", target: "name", expectType: "string", expectDoc: "用户显示名称" },
      { expr: "user.nmae", target: "nmae" },
    ],
    expectedDiagnostics: [{ expr: "user.nmae", target: "nmae" }],
    note: "JavaScript + JSDoc with `checkJs: false`; shadow-only `@ts-check` surfaces one unrelated script error",
  },
  {
    id: "js-jsdoc-off",
    dir: "js-jsdoc",
    registration: "page",
    script: "index.js",
    allowJs: true,
    checkJs: false,
    jsCheck: "off",
    wxml: JS_JSDOC_WXML,
    queries: [
      { expr: "greeting", target: "greeting", expectType: "string", expectDoc: "问候语" },
      { expr: "user.name", target: "name", expectType: "string", expectDoc: "用户显示名称" },
    ],
    expectedDiagnostics: [],
    note: "same JavaScript file with template checking off: hover works, no diagnostics",
  },
  {
    id: "js-checkjs",
    dir: "js-jsdoc",
    registration: "page",
    script: "index.js",
    allowJs: true,
    checkJs: true,
    jsCheck: "inherit",
    wxml: JS_JSDOC_WXML,
    queries: [
      { expr: "greeting", target: "greeting", expectType: "string", expectDoc: "问候语" },
      { expr: "user.name", target: "name", expectType: "string", expectDoc: "用户显示名称" },
      { expr: "user.nmae", target: "nmae" },
    ],
    expectedDiagnostics: [{ expr: "user.nmae", target: "nmae" }],
    note: "`checkJs: true` in the project options: no shadow pragma needed",
  },
  {
    id: "ts-nocheck",
    dir: "ts-nocheck",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: `<view class="page">
  <text class="title">{{ title }}</text>
  <text class="typo">{{ title.missing }}</text>
</view>
`,
    queries: [{ expr: "title", target: "title", expectType: "string", expectDoc: "页面标题" }],
    expectedDiagnostics: [],
    note: "`@ts-nocheck` disables template diagnostics; the typo is not published",
  },
  {
    id: "js-nocheck",
    dir: "js-nocheck",
    registration: "page",
    script: "index.js",
    allowJs: true,
    checkJs: false,
    jsCheck: "pragma",
    wxml: `<view class="page">
  <text class="greeting">{{ greeting }}</text>
  <text class="typo">{{ greeting.missing }}</text>
</view>
`,
    queries: [{ expr: "greeting", target: "greeting", expectType: "string", expectDoc: "问候语" }],
    expectedDiagnostics: [],
    note: "`@ts-nocheck` wins over the requested `@ts-check` pragma",
  },
  {
    id: "js-shebang-bom",
    dir: "js-shebang-bom",
    registration: "page",
    script: "index.js",
    allowJs: true,
    checkJs: false,
    jsCheck: "pragma",
    wxml: `<view class="page">
  <text class="title">{{ title }}</text>
  <text class="typo">{{ title.missing }}</text>
</view>
`,
    queries: [{ expr: "title", target: "title", expectType: "string", expectDoc: "页面标题" }],
    expectedDiagnostics: [{ expr: "title.missing", target: "missing" }],
    note: "BOM + shebang: the derived view drops the BOM and injects `// @ts-check` after the shebang",
  },
  {
    id: "interface-root",
    dir: "interface-root",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: `<view class="page">
  <text class="name">{{ user.name }}</text>
</view>
`,
    queries: [],
    expectedDiagnostics: [],
    note: "`Page({ data: User })`: the interface is not a runtime data root",
  },
  {
    id: "interface-root-import",
    dir: "interface-root-import",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    extraFiles: ["models.ts"],
    wxml: `<view class="page">
  <text class="name">{{ user.name }}</text>
</view>
`,
    queries: [],
    expectedDiagnostics: [],
    note: "`import type` alias used as `data`: checker-based classification, no fabricated fields",
  },
  {
    id: "nullable",
    dir: "nullable",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: `<view class="page">
  <text class="maybe">{{ maybeUser.name }}</text>
  <text class="external">{{ external.anything }}</text>
  <text class="item">{{ users[0].name }}</text>
</view>
`,
    queries: [{ expr: "users[0].name", target: "name", expectType: "string", expectDoc: "用户显示名称" }],
    expectedDiagnostics: [
      { expr: "maybeUser.name", target: "maybeUser" },
      { expr: "external.anything", target: "external" },
    ],
    note: "nullable receiver and `unknown` receiver: diagnostics must land on the receiver",
  },
  {
    id: "cross-file-overlay",
    dir: "cross-file-overlay",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    extraFiles: ["models.ts"],
    wxml: `<view class="page">
  <text class="name">{{ user.name }}</text>
</view>
`,
    queries: [
      {
        expr: "user.name",
        target: "name",
        expectType: "string",
        expectDoc: "用户显示名称",
        definition: true,
        expectDefinition: "external",
      },
    ],
    expectedDiagnostics: [],
    note: "cross-file type: definition lands in models.ts, unsaved overlay of models.ts must propagate",
  },
  {
    id: "alias-project",
    dir: "alias-project",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    tsconfig: "tsconfig.json",
    wxml: `<view class="page">
  <text class="name">{{ user.name }}</text>
</view>
`,
    queries: [
      {
        expr: "user.name",
        target: "name",
        expectType: "string",
        expectDoc: "用户显示名称",
        definition: true,
        expectDefinition: "external",
      },
    ],
    expectedDiagnostics: [],
    note: "`tsconfig.json` extends a base file with `paths`; effective options travel in the request",
  },
  {
    id: "component-data-cross-file",
    dir: "component-data-cross-file",
    registration: "component",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    extraFiles: ["models.ts"],
    wxml: `<view class="card">
  <text class="name">{{ user.name }}</text>
  <text class="title">{{ title }}</text>
</view>
`,
    queries: [
      {
        expr: "user.name",
        target: "name",
        expectType: "string",
        expectDoc: "用户显示名称",
        definition: true,
        expectDefinition: "external",
      },
      { expr: "title", target: "title", expectType: "string", expectDoc: "标题" },
    ],
    expectedDiagnostics: [],
    note: "AC-001: component `data` referencing a cross-file `User` plus `properties` in one root",
  },
  {
    id: "crlf-script",
    dir: "crlf-script",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: `<view class="page">\r\n  <text class="name">{{ user.name }}</text>\r\n  <text class="typo">{{ user.nmae }}</text>\r\n</view>\r\n`,
    queries: [
      { expr: "user.name", target: "name", expectType: "string", expectDoc: "用户显示名称" },
      { expr: "user.nmae", target: "nmae" },
    ],
    expectedDiagnostics: [{ expr: "user.nmae", target: "nmae" }],
    note: "CRLF in the *script* (not only in WXML): offsets must not be normalised",
  },
  {
    id: "data-shorthand",
    dir: "data-shorthand",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: `<view class="page">
  <text>{{ title }}</text>
</view>
`,
    queries: [],
    expectedDiagnostics: [],
    note: "`Page({ data })` shorthand: the member exists but is not a property assignment",
  },
  {
    id: "js-source-check-off",
    dir: "js-source-check",
    registration: "page",
    script: "index.js",
    allowJs: true,
    checkJs: false,
    jsCheck: "off",
    wxml: `<view class="page">
  <text class="title">{{ title }}</text>
  <text class="typo">{{ title.missing }}</text>
</view>
`,
    queries: [{ expr: "title", target: "title", expectType: "string", expectDoc: "页面标题" }],
    expectedDiagnostics: [],
    note: "source `@ts-check` + explicit policy off: the derived view neutralises the pragma",
  },
  {
    id: "js-source-check-inherit",
    dir: "js-source-check",
    registration: "page",
    script: "index.js",
    allowJs: true,
    checkJs: false,
    jsCheck: "inherit",
    wxml: `<view class="page">
  <text class="title">{{ title }}</text>
  <text class="typo">{{ title.missing }}</text>
</view>
`,
    queries: [{ expr: "title", target: "title", expectType: "string", expectDoc: "页面标题" }],
    expectedDiagnostics: [{ expr: "title.missing", target: "missing" }],
    note: "source `@ts-check` under `inherit`: checking stays on and the template diagnostic is published",
  },
  {
    id: "methods-nonliteral",
    dir: "methods-nonliteral",
    registration: "component",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: `<view class="card">
  <text class="title">{{ title }}</text>
</view>
`,
    queries: [{ expr: "title", target: "title", expectType: "string", expectDoc: "标题" }],
    expectedDiagnostics: [],
    note: "component `methods` is not an object literal: view Partial, no fragments mounted (B still mounts)",
  },
  {
    id: "unchecked-index",
    dir: "unchecked-index",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    compilerOptions: { noUncheckedIndexedAccess: true },
    wxml: `<view class="page">
  <text class="name">{{ users[0].name }}</text>
</view>
`,
    queries: [],
    expectedDiagnostics: [{ expr: "users[0].name", target: "users[0]" }],
    note: "`noUncheckedIndexedAccess` must travel in the request (2532 on `users[0]`)",
  },
  {
    id: "nullable-no-strict-null",
    dir: "nullable",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    compilerOptions: { strict: true, strictNullChecks: false },
    wxml: `<view class="page">
  <text class="maybe">{{ maybeUser.name }}</text>
</view>
`,
    queries: [{ expr: "maybeUser.name", target: "name", expectType: "string" }],
    expectedDiagnostics: [],
    note: "`strictNullChecks:false` must suppress the nullable diagnostic (no false positive)",
  },
  {
    id: "no-implicit-this-off",
    dir: "no-implicit-this",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    compilerOptions: { strict: false, noImplicitThis: false },
    wxml: `<view class="page">
  <text class="title">{{ title }}</text>
</view>
`,
    queries: [{ expr: "title", target: "title" }],
    expectedDiagnostics: [],
    note: "`noImplicitThis:false` makes `this.data` any: the probe gate must report Unavailable",
  },
  {
    id: "js-bom-check-off",
    dir: "js-bom-shebang-check",
    registration: "page",
    script: "index.js",
    allowJs: true,
    checkJs: true,
    jsCheck: "off",
    wxml: `<view class="page">
  <text class="title">{{ title }}</text>
  <text class="typo">{{ title.missing }}</text>
</view>
`,
    queries: [{ expr: "title", target: "title", expectType: "string", expectDoc: "页面标题" }],
    expectedDiagnostics: [],
    note: "BOM + shebang + source @ts-check + checkJs:true + policy off: the derived view must drop the BOM and publish nothing",
  },
  {
    id: "js-bom-check-inherit",
    dir: "js-bom-shebang-check",
    registration: "page",
    script: "index.js",
    allowJs: true,
    checkJs: false,
    jsCheck: "inherit",
    wxml: `<view class="page">
  <text class="title">{{ title }}</text>
  <text class="typo">{{ title.missing }}</text>
</view>
`,
    queries: [{ expr: "title", target: "title", expectType: "string", expectDoc: "页面标题" }],
    expectedDiagnostics: [{ expr: "title.missing", target: "missing" }],
    note: "BOM + shebang + source @ts-check + inherit: the source pragma stays effective after dropping the BOM",
  },
  {
    id: "unicode-crlf",
    dir: "unicode-crlf",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml:
      '<view class="page">\r\n  <text>{{ 标题 }}</text>\r\n  <text>😀 {{ user.name }}</text>\r\n  <text>{{ user.nmae }}</text>\r\n</view>\r\n',
    queries: [
      { expr: "user.name", target: "name", expectType: "string", expectDoc: "用户显示名称" },
      { expr: "标题", target: "标题", expectType: "string", expectDoc: "标题" },
      { expr: "user.nmae", target: "nmae" },
    ],
    expectedDiagnostics: [{ expr: "user.nmae", target: "nmae" }],
    note: "CRLF, CJK identifier and astral emoji before the expression",
  },
];

export function scenarioById(id: string): ScenarioSpec {
  const scenario = SCENARIOS.find((candidate) => candidate.id === id);
  if (!scenario) throw new Error(`unknown scenario: ${id}`);
  return scenario;
}

/**
 * A generated page with many fields, used to see how a candidate scales with
 * template size. The sample directory only supplies the file path; the script
 * text is passed as an overlay.
 */
export function wideScenario(fieldCount: number): { spec: ScenarioSpec; scriptText: string } {
  const fields = Array.from(
    { length: fieldCount },
    (_, index) => `    /** 字段 ${index} */\n    value${index}: ${index},`,
  ).join("\n");
  const scriptText = `Page({\n  data: {\n${fields}\n  },\n});\n`;
  const wxml = Array.from({ length: fieldCount }, (_, index) => `<text>{{ value${index} }}</text>`).join("\n");
  const spec: ScenarioSpec = {
    id: `wide-${fieldCount}`,
    dir: "page-basic",
    registration: "page",
    script: "index.ts",
    allowJs: false,
    checkJs: false,
    jsCheck: "inherit",
    wxml: `${wxml}\n`,
    queries: Array.from({ length: fieldCount }, (_, index) => ({
      expr: `value${index}`,
      target: `value${index}`,
    })),
    expectedDiagnostics: [],
  };
  return { spec, scriptText };
}
