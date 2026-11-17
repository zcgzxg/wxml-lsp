/**
 * Minimal in-memory TypeScript host with an explicit read policy.
 *
 * The bridge never executes user code and never reads project sources from
 * disk: every project file must arrive through the snapshot ("files[]" in the
 * C1 contract). Only the TypeScript standard library and the pinned typings
 * package may be read, and only inside their own directories.
 */

import fs from "node:fs";
import path from "node:path";
import ts from "typescript";

export class EnvironmentError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "EnvironmentError";
  }
}

export interface ReadPolicy {
  /** Absolute directories that may be read from disk. */
  diskRoots: string[];
  /** Absolute files that may be read from disk (lib entry points). */
  diskFiles: string[];
}

export interface VirtualProjectInit {
  rootDir: string;
  compilerOptions: ts.CompilerOptions;
  files: Array<{ path: string; text: string }>;
  readPolicy: ReadPolicy;
}

/**
 * Number of recently used projects kept between requests.
 *
 * TypeScript program construction dominates a request (roughly 0.4-0.8 s for
 * the pinned typings). Reusing the language service and updating only file
 * versions keeps ordinary edits in the 10-50 ms range. The pool is bounded so
 * alternating projects cannot grow memory without limit.
 */
export const MAX_POOLED_PROJECTS = 2;

interface PooledProject {
  key: string;
  project: VirtualProject;
}

const projectPool: PooledProject[] = [];

/**
 * Stable JSON-like serialization for cache keys.
 *
 * `JSON.stringify(value, keyArray)` filters *every* object level by that key
 * list, so nested compiler options such as `paths` became `{}`. This helper
 * sorts object keys recursively, preserves array order (TypeScript resolution
 * fallbacks depend on it), and mirrors JSON's treatment of undefined/function
 * object properties by omitting them.
 */
function stableSerialize(value: unknown): string {
  if (value === null) return "null";
  switch (typeof value) {
    case "string":
    case "number":
    case "boolean":
      return JSON.stringify(value);
    case "undefined":
      return "null";
    case "bigint":
      return JSON.stringify(`${value.toString()}n`);
    case "function":
    case "symbol":
      return "null";
    default:
      break;
  }
  if (Array.isArray(value)) {
    return `[${value.map((entry) => stableSerialize(entry)).join(",")}]`;
  }
  if (typeof value === "object") {
    const object = value as Record<string, unknown>;
    const keys = Object.keys(object)
      .filter((key) => {
        const entry = object[key];
        return entry !== undefined && typeof entry !== "function" && typeof entry !== "symbol";
      })
      .sort();
    return `{${keys.map((key) => `${JSON.stringify(key)}:${stableSerialize(object[key])}`).join(",")}}`;
  }
  return "null";
}

/** Canonical key for a project shape: options, read policy and file paths. */
function projectKey(init: VirtualProjectInit, files: ReadonlyArray<{ path: string }>): string {
  const paths = files.map((file) => path.resolve(file.path)).sort();
  return [
    init.rootDir,
    stableSerialize(init.compilerOptions),
    init.readPolicy.diskRoots.join("\u0000"),
    init.readPolicy.diskFiles.join("\u0000"),
    paths.join("\u0000"),
  ].join("\u0001");
}

/**
 * Returns a project for this exact file shape, reusing a warm language service.
 *
 * A request that changes only file text keeps the cached project and bumps the
 * version of each changed file; TypeScript updates its program incrementally
 * on the next query. A changed file set or option set gets a fresh project.
 */
export function acquireProject(
  init: VirtualProjectInit,
  extraFiles: ReadonlyArray<{ path: string; text: string }> = [],
): VirtualProject {
  const normalized = withPinnedTypings(init);
  const files = [...normalized.files, ...extraFiles];
  const key = projectKey(normalized, files);
  const index = projectPool.findIndex((entry) => entry.key === key);
  if (index >= 0) {
    const [entry] = projectPool.splice(index, 1);
    projectPool.unshift(entry);
    entry.project.refresh(files);
    return entry.project;
  }
  const project = new VirtualProject({ ...normalized, files });
  projectPool.unshift({ key, project });
  if (projectPool.length > MAX_POOLED_PROJECTS) projectPool.length = MAX_POOLED_PROJECTS;
  return project;
}

/**
 * Keeps the pinned mini program typings reachable next to a project tsconfig.
 *
 * A project that sets `typeRoots` replaces TypeScript's default
 * `node_modules/@types` lookup. The project-local roots usually live outside
 * the request snapshot and cannot be read by this backend, so resolving
 * `"miniprogram-api-typings"` against them alone leaves `Component`, `Page`
 * and `wx` undeclared, and every template hover degrades to `any`.
 *
 * Appending the backend's own `node_modules` as a final root restores the
 * pinned package without granting access to any project source: the read
 * policy still rejects files outside the backend typings/lib directories.
 * `types` always includes the pinned package because it is the vocabulary the
 * WXML bridge is built on.
 */
export function withPinnedTypings(init: VirtualProjectInit): VirtualProjectInit {
  const options = init.compilerOptions;
  const types = options.types ?? [];
  const normalizedTypes = types.includes(TYPINGS_PACKAGE)
    ? types
    : [...types, TYPINGS_PACKAGE];
  const typeRoots = options.typeRoots
    ? [...options.typeRoots, path.join(init.rootDir, "node_modules")]
    : undefined;
  return {
    ...init,
    compilerOptions: {
      ...options,
      types: normalizedTypes,
      ...(typeRoots ? { typeRoots } : {}),
    },
  };
}

/** Drops every pooled project. Used by tests and explicit backend resets. */
export function resetProjectPool(): void {
  projectPool.length = 0;
}

/** Number of pooled projects, for bounded-cache tests. */
export function projectPoolSize(): number {
  return projectPool.length;
}

const TYPINGS_PACKAGE = "miniprogram-api-typings";

/** Read policy for this experiment: TS lib + the pinned typings package only. */
export function defaultReadPolicy(backendDir: string): ReadPolicy {
  const typescriptLibDir = path.join(backendDir, "node_modules", "typescript", "lib");
  const typingsDir = path.join(backendDir, "node_modules", TYPINGS_PACKAGE);
  return {
    diskRoots: [typescriptLibDir, typingsDir],
    diskFiles: [path.join(typingsDir, "package.json")],
  };
}

/**
 * Fail fast when the pinned environment is missing.
 *
 * A silently missing lib or typings would turn every query into empty results,
 * which must never look like a clean check.
 */
export function assertEnvironment(backendDir: string, compilerOptions: ts.CompilerOptions): void {
  const lib = ts.getDefaultLibFilePath(compilerOptions);
  if (!fs.existsSync(lib)) {
    throw new EnvironmentError(`TypeScript lib not found: ${path.basename(lib)}`);
  }
  const manifest = path.join(backendDir, "node_modules", TYPINGS_PACKAGE, "package.json");
  if (!fs.existsSync(manifest)) {
    throw new EnvironmentError(`typings package not installed: ${TYPINGS_PACKAGE}`);
  }
  const declared = JSON.parse(fs.readFileSync(manifest, "utf8")) as { version?: string };
  if (!declared.version) {
    throw new EnvironmentError(`typings package has no version: ${TYPINGS_PACKAGE}`);
  }
}

export class VirtualProject {
  readonly rootDir: string;
  readonly compilerOptions: ts.CompilerOptions;
  readonly readPolicy: ReadPolicy;
  private readonly files = new Map<string, string>();
  private readonly versions = new Map<string, number>();
  /** Cached script snapshots keyed by the file version TypeScript last saw. */
  private readonly snapshots = new Map<string, { version: number; snapshot: ts.IScriptSnapshot }>();
  private knownFiles: string[];
  private knownDirectories = new Set<string>();
  private service: ts.LanguageService | undefined;
  private dirty = true;

  constructor(init: VirtualProjectInit) {
    this.rootDir = init.rootDir;
    this.compilerOptions = init.compilerOptions;
    this.readPolicy = init.readPolicy;
    for (const file of init.files) {
      const normalized = path.resolve(file.path);
      this.files.set(normalized, file.text);
      this.versions.set(normalized, 1);
    }
    this.knownFiles = [...this.files.keys()];
    this.rebuildKnownDirectories();
  }

  /** Create or replace a file inside the snapshot. Marks the program dirty. */
  set(filePath: string, text: string): void {
    this.applyFile(filePath, text);
    this.knownFiles = [...this.files.keys()];
    this.rebuildKnownDirectories();
    this.dirty = true;
  }

  /**
   * Replace the snapshot without discarding the warm language service.
   *
   * Only text/version changes are incremental for TypeScript. A changed file
   * set rebuilds the lookup tables; the language service itself stays alive, so
   * the next query reuses parsing and the type checker.
   */
  refresh(files: ReadonlyArray<{ path: string; text: string }>): void {
    const next = new Set<string>();
    for (const file of files) {
      const normalized = path.resolve(file.path);
      next.add(normalized);
      this.applyFile(normalized, file.text);
    }
    let removed = false;
    for (const existing of [...this.files.keys()]) {
      if (next.has(existing)) continue;
      this.files.delete(existing);
      this.versions.delete(existing);
      this.snapshots.delete(existing);
      removed = true;
    }
    if (removed || this.knownFiles.length !== next.size) {
      this.knownFiles = [...this.files.keys()];
      this.rebuildKnownDirectories();
    }
  }

  private applyFile(filePath: string, text: string): void {
    const normalized = path.resolve(filePath);
    if (this.files.get(normalized) === text) return;
    this.files.set(normalized, text);
    const version = (this.versions.get(normalized) ?? 0) + 1;
    this.versions.set(normalized, version);
    this.snapshots.delete(normalized);
  }

  private rebuildKnownDirectories(): void {
    const directories = new Set<string>();
    for (const file of this.files.keys()) {
      let directory = path.dirname(file);
      while (directory !== path.dirname(directory)) {
        if (directories.has(directory)) break;
        directories.add(directory);
        directory = path.dirname(directory);
      }
    }
    this.knownDirectories = directories;
  }

  has(filePath: string): boolean {
    return this.files.has(path.resolve(filePath));
  }

  text(filePath: string): string | undefined {
    return this.files.get(path.resolve(filePath));
  }

  /** Files that the snapshot owns (memory only). */
  fileNames(): string[] {
    return [...this.files.keys()];
  }

  /** Files this project may read from disk, for the evidence record. */
  diskReadable(): string[] {
    return [...this.readPolicy.diskRoots, ...this.readPolicy.diskFiles];
  }

  languageService(): ts.LanguageService {
    if (!this.service || this.dirty) {
      this.service = this.createService();
      this.dirty = false;
    }
    return this.service;
  }

  program(): ts.Program {
    const program = this.languageService().getProgram();
    if (!program) throw new Error("language service returned no program");
    return program;
  }

  checker(): ts.TypeChecker {
    return this.program().getTypeChecker();
  }

  private isAllowedOnDisk(filePath: string): boolean {
    const resolved = path.resolve(filePath);
    if (this.readPolicy.diskFiles.includes(resolved)) return true;
    return this.readPolicy.diskRoots.some(
      (root) => resolved === root || resolved.startsWith(root + path.sep),
    );
  }

  private diskFileExists(filePath: string): boolean {
    return this.isAllowedOnDisk(filePath) && ts.sys.fileExists(filePath);
  }

  private directoryExists(dirPath: string): boolean {
    const resolved = path.resolve(dirPath);
    // Project directories exist as soon as the snapshot contains a file below them.
    if (this.knownDirectories.has(resolved)) return true;
    // Directory existence is not source content: type-reference resolution needs
    // to see node_modules while file reads stay restricted by the policy.
    return ts.sys.directoryExists(resolved);
  }

  private createService(): ts.LanguageService {
    const host: ts.LanguageServiceHost = {
      getScriptFileNames: () => this.knownFiles,
      getScriptVersion: (fileName) => String(this.versions.get(path.resolve(fileName)) ?? 0),
      getScriptSnapshot: (fileName) => {
        const resolved = path.resolve(fileName);
        const inMemory = this.files.get(resolved);
        if (inMemory !== undefined) {
          const version = this.versions.get(resolved) ?? 0;
          const cached = this.snapshots.get(resolved);
          if (cached && cached.version === version) return cached.snapshot;
          const snapshot = ts.ScriptSnapshot.fromString(inMemory);
          this.snapshots.set(resolved, { version, snapshot });
          return snapshot;
        }
        if (!this.isAllowedOnDisk(resolved)) return undefined;
        const onDisk = ts.sys.readFile(resolved);
        return onDisk === undefined ? undefined : ts.ScriptSnapshot.fromString(onDisk);
      },
      getCurrentDirectory: () => this.rootDir,
      getCompilationSettings: () => this.compilerOptions,
      getDefaultLibFileName: (options) => ts.getDefaultLibFilePath(options),
      fileExists: (fileName) => this.files.has(path.resolve(fileName)) || this.diskFileExists(fileName),
      readFile: (fileName) => {
        const resolved = path.resolve(fileName);
        const inMemory = this.files.get(resolved);
        if (inMemory !== undefined) return inMemory;
        return this.isAllowedOnDisk(resolved) ? ts.sys.readFile(resolved) : undefined;
      },
      readDirectory: (directory, extensions, exclude, include, depth) => {
        // Never scan project or dependency directories on disk.
        if (!this.isAllowedOnDisk(directory)) return [];
        return ts.sys.readDirectory(directory, extensions, exclude, include, depth);
      },
      directoryExists: (directory) => this.directoryExists(directory),
      getDirectories: (directory) => (this.isAllowedOnDisk(directory) ? ts.sys.getDirectories(directory) : []),
      useCaseSensitiveFileNames: () => true,
    };
    return ts.createLanguageService(host);
  }
}

/** Parse a file the same way the program will see it. */
export function parseSource(text: string, fileName: string): ts.SourceFile {
  return ts.createSourceFile(fileName, text, ts.ScriptTarget.Latest, true, scriptKindOf(fileName));
}

export function scriptKindOf(fileName: string): ts.ScriptKind {
  const extension = path.extname(fileName).toLowerCase();
  switch (extension) {
    case ".js":
      return ts.ScriptKind.JS;
    case ".jsx":
      return ts.ScriptKind.JSX;
    case ".tsx":
      return ts.ScriptKind.TSX;
    default:
      return ts.ScriptKind.TS;
  }
}

/**
 * Compiler options for unit tests and isolation probes.
 *
 * Production paths (`run.ts`) never call this: they convert the request's
 * effective options with `protocol.toCompilerOptions`.
 */
export function baseCompilerOptions(overrides: ts.CompilerOptions = {}): ts.CompilerOptions {
  return {
    strict: true,
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.ESNext,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
    noEmit: true,
    skipLibCheck: true,
    allowJs: true,
    checkJs: false,
    types: ["miniprogram-api-typings"],
    ...overrides,
  };
}

/** Module classification of a source file, as TypeScript sees it. */
export function moduleKind(sourceFile: ts.SourceFile): "script" | "module" {
  const withIndicator = sourceFile as ts.SourceFile & { externalModuleIndicator?: ts.Node };
  return withIndicator.externalModuleIndicator === undefined ? "script" : "module";
}
