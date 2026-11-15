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
  private readonly knownFiles: string[];
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
  }

  /** Create or replace a file inside the snapshot. Marks the program dirty. */
  set(filePath: string, text: string): void {
    const normalized = path.resolve(filePath);
    this.files.set(normalized, text);
    this.versions.set(normalized, (this.versions.get(normalized) ?? 0) + 1);
    if (!this.knownFiles.includes(normalized)) this.knownFiles.push(normalized);
    this.dirty = true;
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
    if (this.knownFiles.some((file) => file.startsWith(resolved + path.sep))) return true;
    // Directory existence is not source content: type-reference resolution needs
    // to see node_modules while file reads stay restricted by the policy.
    return ts.sys.directoryExists(resolved);
  }

  private createService(): ts.LanguageService {
    const host: ts.LanguageServiceHost = {
      getScriptFileNames: () => [...this.files.keys()],
      getScriptVersion: (fileName) => String(this.versions.get(path.resolve(fileName)) ?? 0),
      getScriptSnapshot: (fileName) => {
        const resolved = path.resolve(fileName);
        const inMemory = this.files.get(resolved);
        if (inMemory !== undefined) return ts.ScriptSnapshot.fromString(inMemory);
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
