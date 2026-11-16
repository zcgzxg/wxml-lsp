/**
 * Shared request builders for the request-driven unit tests.
 *
 * `analyze()` never reads project sources or tsconfig from disk, so a test can
 * keep the whole project in memory and use a project root that does not exist.
 * No file is written anywhere: the tests leave no temporary state behind.
 */

import {
  DEFAULT_LIMITS,
  PROTOCOL_VERSION,
  fileKindOf,
  type AnalyzeRequest,
  type AnalyzeResponse,
  type RequestCompilerOptions,
  type RequestComponentUsage,
  type RequestFile,
  type RequestFragment,
} from "../src/protocol.ts";

/** Deliberately non-existent: a disk read would be a test failure. */
export const TEST_PROJECT_ROOT = "/tmp/opencode/wxml-lsp-wave5-memory-project";

export interface TestFile {
  path: string;
  text: string;
}

function requestFile(path: string, text: string): RequestFile {
  return {
    fileId: path,
    path,
    kind: fileKindOf(path),
    text,
    sourceRevision: 1,
  };
}

export interface RequestOptions {
  script: string;
  /** Project-relative script path (defaults to `index.ts`). */
  scriptPath?: string;
  extraFiles?: TestFile[];
  fragments?: RequestFragment[];
  components?: RequestComponentUsage[];
  kind?: "page" | "component";
  jsCheck?: "inherit" | "pragma" | "off";
  checkJs?: boolean;
  compilerOptions?: Partial<RequestCompilerOptions>;
}

/** Build a self-contained C1 request over in-memory files. */
export function analyzeRequest(options: RequestOptions): AnalyzeRequest {
  const scriptPath = options.scriptPath ?? "index.ts";
  const isJavaScript = /\.(js|jsx)$/.test(scriptPath);
  const compilerOptions: RequestCompilerOptions = {
    strict: true,
    allowJs: isJavaScript,
    checkJs: options.checkJs ?? false,
    target: "ES2022",
    module: "ESNext",
    moduleResolution: "bundler",
    types: ["miniprogram-api-typings"],
    ...options.compilerOptions,
  };
  const kind = options.kind ?? "page";
  return {
    protocolVersion: PROTOCOL_VERSION,
    requestId: "test:1",
    analysisStamp: { snapshotId: 1, projectRevision: 1, viewRevision: 1, backendEpoch: 1 },
    view: { viewId: "test-view", owner: { kind, fileId: scriptPath } },
    project: { projectRoot: TEST_PROJECT_ROOT, compilerOptions, typings: { package: "miniprogram-api-typings", version: "5.3.1" } },
    scriptFileId: scriptPath,
    files: [
      requestFile(scriptPath, options.script),
      ...(options.extraFiles ?? []).map((file) => requestFile(file.path, file.text)),
    ],
    fragments: options.fragments ?? [],
    ...(options.components ? { components: options.components } : {}),
    policy: { jsCheck: options.jsCheck ?? "inherit", propertiesOverlay: "declaration" },
    limits: DEFAULT_LIMITS,
  };
}

/** One request fragment with a byte range that satisfies the protocol checks. */
export function fragment(
  fragmentId: string,
  exprText: string,
  wxmlRange: { start: number; end: number },
  overrides: Partial<RequestFragment> = {},
): RequestFragment {
  return {
    fragmentId,
    exprText,
    wxmlRange,
    fragmentRevision: 1,
    root: "page",
    ...overrides,
  };
}

export interface WxmlTemplate {
  text: string;
  /** Append `{{ expr }}` and return its absolute byte range. */
  expr(exprText: string): { start: number; end: number };
  /** Append raw template text and return its absolute byte range. */
  raw(rawText: string): { start: number; end: number };
}

/** A tiny WXML-like document builder; only byte ranges matter to the backend. */
export function template(): WxmlTemplate {
  const doc = {
    text: "",
    expr(exprText: string): { start: number; end: number } {
      doc.text += "{{ ";
      const start = Buffer.byteLength(doc.text, "utf8");
      doc.text += exprText;
      const end = start + Buffer.byteLength(exprText, "utf8");
      doc.text += " }}\n";
      return { start, end };
    },
    raw(rawText: string): { start: number; end: number } {
      const start = Buffer.byteLength(doc.text, "utf8");
      doc.text += rawText;
      const end = start + Buffer.byteLength(rawText, "utf8");
      return { start, end };
    },
  };
  return doc;
}

/** Byte offset of `needle` inside `text` (UTF-8), or -1. */
export function byteIndexOf(text: string, needle: string): number {
  const index = text.indexOf(needle);
  return index < 0 ? -1 : Buffer.byteLength(text.slice(0, index), "utf8");
}

/** Diagnostics of one response fragment, by fragment id. */
export function fragmentDiagnostics(
  response: AnalyzeResponse,
  fragmentId: string,
): AnalyzeResponse["fragments"][number]["diagnostics"] {
  return response.fragments.find((entry) => entry.fragmentId === fragmentId)?.diagnostics ?? [];
}
