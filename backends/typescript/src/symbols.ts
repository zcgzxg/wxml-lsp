/**
 * Script symbol facts for WXML references, rename and code actions.
 *
 * The query is intentionally separate from `analyze`: it must answer for an
 * arbitrary project script (for example a component that owns a property used
 * by the current page), while still using the same in-memory snapshot and the
 * TypeScript language service. No project file is read from disk and every
 * response token is a UTF-8 byte range in an original snapshot file.
 *
 * Guarantees:
 *
 * - `complete` is false whenever a spread, computed member, behavior or an
 *   out-of-snapshot reference could hide an occurrence;
 * - `setData`/observer paths are recognized only as literal API forms; a
 *   dynamic key makes the answer incomplete instead of guessing;
 * - an insertion point is returned only for a missing event handler on a
 *   literal owner table, and its formatted text is bounded.
 */

import path from "node:path";
import ts from "typescript";
import { findRegistrations, memberMap, propertyName } from "./ast.ts";
import {
  assertEnvironment,
  defaultReadPolicy,
  EnvironmentError,
  parseSource,
  VirtualProject,
} from "./compiler.ts";
import { handlerFacts } from "./events.ts";
import {
  PROTOCOL_VERSION,
  toCompilerOptions,
  type FileKind,
  type SymbolWireOccurrence,
  type SymbolWireReference,
  type SymbolWireToken,
  type SymbolsRequest,
  type SymbolsResponse,
} from "./protocol.ts";

export interface SymbolsConfig {
  /** Backend directory holding `node_modules` with TypeScript and typings. */
  backendDir: string;
}

const MAX_INSERTION_TEXT = 16 * 1024;
const MAX_PATH_SEGMENTS = 16;

interface FileInfo {
  path: string;
  absolute: string;
  kind: FileKind;
  text: string;
  byte: (utf16: number) => number | undefined;
  source?: ts.SourceFile;
}

interface ScanState {
  complete: boolean;
  reasons: string[];
  dynamic: boolean;
}

function reject(state: ScanState, reason: string): void {
  if (!state.reasons.includes(reason)) state.reasons.push(reason);
  state.complete = false;
}

/** UTF-16 -> UTF-8 byte conversion table for one file. */
function byteConverter(text: string): (utf16: number) => number | undefined {
  const boundaries: Array<{ at: number; delta: number }> = [];
  let delta = 0;
  let units = 0;
  for (const char of text) {
    const code = char.codePointAt(0) ?? 0;
    const charUnits = char.length;
    const bytes = code <= 0x7f ? 1 : code <= 0x7ff ? 2 : code <= 0xffff ? 3 : 4;
    if (bytes !== charUnits) {
      delta += bytes - charUnits;
      boundaries.push({ at: units + charUnits, delta });
    }
    units += charUnits;
  }
  if (boundaries.length === 0) {
    return (offset) => (Number.isInteger(offset) && offset >= 0 && offset <= text.length ? offset : undefined);
  }
  return (offset) => {
    if (!Number.isInteger(offset) || offset < 0 || offset > text.length) return undefined;
    if (offset < text.length) {
      const code = text.charCodeAt(offset);
      if (code >= 0xdc00 && code <= 0xdfff) return undefined;
    }
    let low = 0;
    let high = boundaries.length;
    while (low < high) {
      const mid = (low + high) >>> 1;
      if (boundaries[mid]!.at <= offset) low = mid + 1;
      else high = mid;
    }
    return offset + (low === 0 ? 0 : boundaries[low - 1]!.delta);
  };
}

function tokenOf(file: FileInfo, start: number, end: number): SymbolWireToken | undefined {
  if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end < start || end > file.text.length) {
    return undefined;
  }
  const startByte = file.byte(start);
  const endByte = file.byte(end);
  if (startByte === undefined || endByte === undefined) return undefined;
  return { fileId: file.path, start: startByte, end: endByte };
}

/** Returns the byte range of a property name without surrounding quotes. */
function nameToken(
  name: ts.PropertyName | undefined,
  file: FileInfo,
  sourceFile: ts.SourceFile,
): { token: SymbolWireToken; quoted: boolean } | undefined {
  if (!name) return undefined;
  try {
    if (ts.isIdentifier(name)) {
      const token = tokenOf(file, name.getStart(sourceFile), name.getEnd());
      return token ? { token, quoted: false } : undefined;
    }
    if (ts.isStringLiteral(name) || ts.isNoSubstitutionTemplateLiteral(name)) {
      const start = name.getStart(sourceFile);
      const end = name.getEnd();
      if (end - start < 2) return undefined;
      const token = tokenOf(file, start + 1, end - 1);
      return token ? { token, quoted: true } : undefined;
    }
  } catch {
    return undefined;
  }
  return undefined;
}

/** One statically named property assignment with an exact name token. */
interface NamedProperty {
  node: ts.PropertyAssignment;
  name: string;
  token: SymbolWireToken;
  quoted: boolean;
  file: FileInfo;
}

function propertyCandidates(
  object: ts.ObjectLiteralExpression,
  file: FileInfo,
  sourceFile: ts.SourceFile,
  state: ScanState,
): Map<string, NamedProperty[]> {
  const result = new Map<string, NamedProperty[]>();
  for (const property of object.properties) {
    if (ts.isSpreadAssignment(property)) {
      reject(state, "a data root contains a spread member");
      continue;
    }
    if (ts.isComputedPropertyName(property.name)) {
      reject(state, "a data root contains a computed member name");
      continue;
    }
    if (!ts.isPropertyAssignment(property)) {
      reject(state, "a data root contains a shorthand or method member");
      continue;
    }
    const name = propertyName(property);
    if (name === undefined) {
      reject(state, "a data root member has no static name");
      continue;
    }
    const token = nameToken(property.name, file, sourceFile);
    if (!token) {
      reject(state, `data root member ${bounded(name, 80)} has an unmappable name token`);
      continue;
    }
    const entry = result.get(name) ?? [];
    entry.push({ node: property, name, token: token.token, quoted: token.quoted, file });
    result.set(name, entry);
  }
  return result;
}

function bounded(text: string, limit: number): string {
  const value = text.length <= limit ? text : `${text.slice(0, Math.max(0, limit - 1))}…`;
  return value.replace(/[\u0000-\u001f\u007f]/g, " ");
}

interface ScriptIndex {
  project: VirtualProject;
  files: Map<string, FileInfo>;
  subject: FileInfo;
  sourceFile: ts.SourceFile;
  registration?: { kind: "page" | "component"; options: ts.ObjectLiteralExpression };
  state: ScanState;
}

/** Resolve one registration for the requested root kind. */
function resolveRegistration(
  sourceFile: ts.SourceFile,
  root: "page" | "component",
  state: ScanState,
): { kind: "page" | "component"; options: ts.ObjectLiteralExpression } | undefined {
  const scan = findRegistrations(sourceFile);
  let matching = scan.registrations.filter((candidate) => candidate.kind === root);
  if (matching.length === 0 && root === "page") {
    matching = scan.registrations.filter((candidate) => candidate.kind === "component");
  }
  if (matching.length === 0) {
    reject(state, `no literal ${root} registration`);
    return undefined;
  }
  if (matching.length > 1) {
    reject(state, `multiple ${root} registrations`);
  }
  const label = root === "page" ? "Page " : "Component ";
  if (scan.unsupportedCalls.some((call) => call.startsWith(label))) {
    reject(state, `${root} registration is not a literal object`);
  }
  const registration = matching[0]!;
  return { kind: registration.kind, options: registration.options };
}

/** Resolve an expression to an object literal declared in the snapshot. */
function resolveObjectLiteral(
  project: VirtualProject,
  expression: ts.Expression,
  depth: number,
  seen: Set<ts.Node>,
): { object: ts.ObjectLiteralExpression; sourceFile: ts.SourceFile; file: FileInfo } | undefined {
  if (depth > 8) return undefined;
  let node: ts.Expression = expression;
  while (
    ts.isParenthesizedExpression(node) ||
    ts.isAsExpression(node) ||
    ts.isSatisfiesExpression(node) ||
    ts.isTypeAssertionExpression(node)
  ) {
    node = node.expression;
  }
  if (ts.isObjectLiteralExpression(node)) {
    const sourceFile = node.getSourceFile();
    const absolute = path.resolve(sourceFile.fileName);
    const file = fileInfoFor(project, absolute);
    if (!file) return undefined;
    return { object: node, sourceFile, file };
  }
  if (!ts.isIdentifier(node) && !ts.isPropertyAccessExpression(node)) return undefined;
  if (seen.has(node)) return undefined;
  seen.add(node);
  const checker = project.checker();
  let symbol = checker.getSymbolAtLocation(node);
  if (!symbol) return undefined;
  if ((symbol.flags & ts.SymbolFlags.Alias) !== 0) {
    try {
      symbol = checker.getAliasedSymbol(symbol);
    } catch {
      return undefined;
    }
  }
  const declarations = symbol.declarations ?? [];
  let resolved: { object: ts.ObjectLiteralExpression; sourceFile: ts.SourceFile; file: FileInfo } | undefined;
  for (const declaration of declarations) {
    let initializer: ts.Expression | undefined;
    if (ts.isVariableDeclaration(declaration) || ts.isBindingElement(declaration)) {
      initializer = declaration.initializer;
    } else if (ts.isPropertyAssignment(declaration)) {
      initializer = declaration.initializer;
    }
    if (!initializer) continue;
    const candidate = resolveObjectLiteral(project, initializer, depth + 1, seen);
    if (!candidate) continue;
    if (resolved && resolved.object !== candidate.object) return undefined;
    resolved = candidate;
  }
  return resolved;
}

/** File info for an absolute source file inside the request snapshot. */
function fileInfoFor(project: VirtualProject, absolute: string): FileInfo | undefined {
  const resolved = path.resolve(absolute);
  return fileInfoFromProject(project, resolved);
}

const projectFileCache = new WeakMap<VirtualProject, Map<string, FileInfo>>();

function fileInfoFromProject(project: VirtualProject, absolute: string): FileInfo | undefined {
  const files = projectFileCache.get(project);
  return files?.get(path.resolve(absolute));
}

/** Rebuild the absolute file map after the virtual project is constructed. */
function buildFileMap(project: VirtualProject, request: SymbolsRequest): Map<string, FileInfo> {
  const map = new Map<string, FileInfo>();
  for (const file of request.files) {
    const absolute = path.resolve(request.project.projectRoot, file.path);
    const info: FileInfo = {
      path: file.path,
      absolute,
      kind: file.kind,
      text: file.text,
      byte: byteConverter(file.text),
    };
    map.set(absolute, info);
  }
  projectFileCache.set(project, map);
  return map;
}

/** Resolve the data/properties roots of the registration. */
function dataRootObjects(
  project: VirtualProject,
  registration: { kind: "page" | "component"; options: ts.ObjectLiteralExpression },
  state: ScanState,
): {
  data?: { object: ts.ObjectLiteralExpression; sourceFile: ts.SourceFile; file: FileInfo };
  properties?: { object: ts.ObjectLiteralExpression; sourceFile: ts.SourceFile; file: FileInfo };
} {
  const members = memberMap(registration.options);
  const resolveMember = (name: "data" | "properties") => {
    const member = members.get(name);
    if (!member) return undefined;
    if (!ts.isPropertyAssignment(member)) {
      reject(state, `\`${name}\` is not a property assignment`);
      return undefined;
    }
    const resolved = resolveObjectLiteral(project, member.initializer, 0, new Set());
    if (!resolved) {
      reject(state, `\`${name}\` is not a statically resolvable object literal`);
    }
    return resolved;
  };
  const data = resolveMember("data");
  const properties = registration.kind === "component" ? resolveMember("properties") : undefined;
  if (registration.kind === "component") {
    const behaviors = members.get("behaviors");
    if (behaviors) {
      if (!ts.isPropertyAssignment(behaviors)) {
        reject(state, "component behaviors are not statically readable");
      } else if (!ts.isArrayLiteralExpression(behaviors.initializer) || behaviors.initializer.elements.length > 0) {
        reject(state, "component behaviors may declare data or handlers");
      }
    }
  }
  return {
    ...(data ? { data } : {}),
    ...(properties ? { properties } : {}),
  };
}

/** Find the unique property with `name` in one resolved object. */
function uniqueProperty(
  candidates: Map<string, NamedProperty[]>,
  name: string,
  state: ScanState,
): NamedProperty | undefined {
  const entries = candidates.get(name);
  if (!entries || entries.length === 0) return undefined;
  if (entries.length > 1) {
    reject(state, `duplicate data member ${bounded(name, 80)}`);
    return undefined;
  }
  return entries[0];
}

/** Resolve a root-to-leaf path and return the leaf declaration. */
function resolveDataPath(
  project: VirtualProject,
  roots: ReturnType<typeof dataRootObjects>,
  pathSegments: string[],
  state: ScanState,
): {
  declaration?: NamedProperty;
  fields: Map<string, NamedProperty[]>;
  parent?: { object: ts.ObjectLiteralExpression; sourceFile: ts.SourceFile; file: FileInfo };
} {
  if (pathSegments.length === 0 || pathSegments.length > MAX_PATH_SEGMENTS) {
    reject(state, "data path is empty or too deep");
    return { fields: new Map() };
  }
  const rootEntries: Array<NamedProperty[]> = [];
  let current:
    | { object: ts.ObjectLiteralExpression; sourceFile: ts.SourceFile; file: FileInfo }
    | undefined;
  for (const root of [roots.data, roots.properties]) {
    if (!root) continue;
    const candidates = propertyCandidates(root.object, root.file, root.sourceFile, state);
    const entry = candidates.get(pathSegments[0]!);
    if (entry) rootEntries.push(entry);
    if (!current && entry) current = root;
  }
  if (rootEntries.length > 1) {
    reject(state, `data member ${bounded(pathSegments[0]!, 80)} is declared by both data and properties`);
    return { fields: new Map() };
  }
  let declaration: NamedProperty | undefined;
  let fields = new Map<string, NamedProperty[]>();
  let parent: { object: ts.ObjectLiteralExpression; sourceFile: ts.SourceFile; file: FileInfo } | undefined;
  for (let index = 0; index < pathSegments.length; index += 1) {
    if (!current) break;
    const candidates = propertyCandidates(current.object, current.file, current.sourceFile, state);
    fields = candidates;
    const property = uniqueProperty(candidates, pathSegments[index]!, state);
    if (!property) break;
    if (index === pathSegments.length - 1) {
      declaration = property;
      parent = current;
      break;
    }
    const next = resolveObjectLiteral(project, property.node.initializer, 0, new Set());
    if (!next) {
      reject(state, `data member ${bounded(pathSegments.slice(0, index + 1).join("."), 80)} is not an object literal`);
      break;
    }
    current = next;
  }
  return {
    ...(declaration ? { declaration } : {}),
    fields,
    ...(parent ? { parent } : {}),
  };
}

/** Deepest node containing a position. */
function nodeAt(sourceFile: ts.SourceFile, position: number): ts.Node | undefined {
  let best: ts.Node | undefined;
  const visit = (node: ts.Node): void => {
    if (position >= node.getStart(sourceFile) && position < node.getEnd()) {
      best = node;
      node.forEachChild(visit);
    }
  };
  visit(sourceFile);
  return best;
}

/** Rejects reference contexts where a plain token rename changes a binding. */
function unsafeReferenceContext(sourceFile: ts.SourceFile, position: number): string | undefined {
  let node = nodeAt(sourceFile, position);
  while (node) {
    if (ts.isBindingElement(node)) {
      return node.propertyName === undefined
        ? "a destructuring shorthand binding cannot be renamed safely"
        : undefined;
    }
    if (ts.isShorthandPropertyAssignment(node)) {
      return "an object shorthand property cannot be renamed safely";
    }
    if (
      ts.isPropertyAccessExpression(node) ||
      ts.isPropertyAssignment(node) ||
      ts.isMethodDeclaration(node) ||
      ts.isPropertyDeclaration(node) ||
      ts.isVariableDeclaration(node) ||
      ts.isParameter(node)
    ) {
      return undefined;
    }
    node = node.parent;
  }
  return undefined;
}

/** Collect TypeScript references for one declaration name node. */
function collectReferences(
  index: ScriptIndex,
  nameNode: ts.Node,
  state: ScanState,
): SymbolWireReference[] {
  const service = index.project.languageService();
  let groups:
    | ReadonlyArray<{
        definition?: { fileName: string; textSpan: { start: number; length: number } };
        references: readonly ts.ReferenceEntry[];
      }>
    | undefined;
  try {
    const declarationFile = nameNode.getSourceFile();
    const declarationPath = path.resolve(declarationFile.fileName);
    const start = nameNode.getStart(declarationFile);
    groups = service.findReferences(declarationPath, start);
    if (!groups) {
      const fallback = service.getReferencesAtPosition(declarationPath, start) ?? [];
      groups = fallback.length > 0 ? [{ references: fallback }] : [];
    }
  } catch {
    groups = undefined;
  }
  if (!groups) {
    reject(state, "the TypeScript language service did not answer the reference query");
    return [];
  }
  const references: SymbolWireReference[] = [];
  const seen = new Set<string>();
  for (const group of groups) {
    for (const reference of group.references) {
      const absolute = path.resolve(reference.fileName);
      const file = index.files.get(absolute);
      if (!file) {
        reject(state, "a reference points outside the script snapshot");
        continue;
      }
      const token = tokenOf(file, reference.textSpan.start, reference.textSpan.start + reference.textSpan.length);
      if (!token) {
        reject(state, "a reference has an unmappable token range");
        continue;
      }
      const referenceSource = index.project.program().getSourceFile(absolute);
      if (referenceSource) {
        const unsafe = unsafeReferenceContext(referenceSource, reference.textSpan.start);
        if (unsafe) {
          reject(state, unsafe);
          continue;
        }
      }
      const key = `${token.fileId}:${token.start}:${token.end}`;
      if (seen.has(key)) continue;
      seen.add(key);
      const isDeclaration =
        group.definition !== undefined &&
        path.resolve(group.definition.fileName) === absolute &&
        group.definition.textSpan.start === reference.textSpan.start &&
        group.definition.textSpan.length === reference.textSpan.length;
      references.push({
        ...token,
        role: isDeclaration ? "declaration" : reference.isWriteAccess ? "write" : "read",
      });
      if (references.length >= 4096) {
        reject(state, "the reference list was truncated");
        return references;
      }
    }
  }
  return references;
}

function isSetDataCallee(expression: ts.Expression): boolean {
  return ts.isPropertyAccessExpression(expression) && expression.name.text === "setData";
}

function isThisSetData(expression: ts.Expression): boolean {
  return (
    ts.isPropertyAccessExpression(expression) &&
    expression.name.text === "setData" &&
    expression.expression.kind === ts.SyntaxKind.ThisKeyword
  );
}

/** Local aliases of `this` (`const that = this`, including simple chains). */
function thisAliases(sourceFile: ts.SourceFile): Set<string> {
  const aliases = new Set<string>();
  let changed = true;
  while (changed) {
    changed = false;
    const visit = (node: ts.Node): void => {
      if (
        ts.isVariableDeclaration(node) &&
        ts.isIdentifier(node.name) &&
        node.initializer &&
        !aliases.has(node.name.text)
      ) {
        const initializer = node.initializer;
        if (
          initializer.kind === ts.SyntaxKind.ThisKeyword ||
          (ts.isIdentifier(initializer) && aliases.has(initializer.text))
        ) {
          aliases.add(node.name.text);
          changed = true;
        }
      }
      node.forEachChild(visit);
    };
    visit(sourceFile);
  }
  return aliases;
}

/** Whole-value uses of `this.data`/`that.data` hide dynamic field access. */
function scanDataEscapes(sourceFile: ts.SourceFile, state: ScanState): void {
  const aliases = thisAliases(sourceFile);
  const isDataAccess = (node: ts.Node): node is ts.PropertyAccessExpression =>
    ts.isPropertyAccessExpression(node) &&
    node.name.text === "data" &&
    (node.expression.kind === ts.SyntaxKind.ThisKeyword ||
      (ts.isIdentifier(node.expression) && aliases.has(node.expression.text)));
  const visit = (node: ts.Node): void => {
    if (
      ts.isElementAccessExpression(node) &&
      ts.isIdentifier(node.expression) &&
      aliases.has(node.expression.text) &&
      node.argumentExpression &&
      ts.isStringLiteral(node.argumentExpression) &&
      node.argumentExpression.text === "data"
    ) {
      reject(state, "an alias accesses `data` through a computed member");
    }
    if (isDataAccess(node)) {
      const parent = node.parent;
      const directMember =
        (ts.isPropertyAccessExpression(parent) || ts.isElementAccessExpression(parent)) &&
        parent.expression === node;
      if (!directMember) {
        const safeDestructure =
          ts.isVariableDeclaration(parent) &&
          parent.initializer === node &&
          ts.isObjectBindingPattern(parent.name) &&
          !parent.name.elements.some((element) => element.dotDotDotToken !== undefined);
        if (!safeDestructure) {
          reject(
            state,
            "this.data is used as a whole value; dynamic field access is not analyzable",
          );
        }
      } else if (ts.isElementAccessExpression(parent)) {
        reject(state, "this.data uses a computed access that cannot be proven static");
      }
    }
    node.forEachChild(visit);
  };
  visit(sourceFile);
}

function propertyNameSite(
  name: ts.PropertyName,
  sourceFile: ts.SourceFile,
): { start: number; end: number; name: string; quoted: boolean } | undefined {
  try {
    if (ts.isIdentifier(name)) {
      return { start: name.getStart(sourceFile), end: name.getEnd(), name: name.text, quoted: false };
    }
    if (ts.isStringLiteral(name) || ts.isNoSubstitutionTemplateLiteral(name)) {
      return {
        start: name.getStart(sourceFile) + 1,
        end: name.getEnd() - 1,
        name: name.text,
        quoted: true,
      };
    }
  } catch {
    return undefined;
  }
  return undefined;
}

interface LiteralSetDataPath {
  site: { start: number; end: number; name: string; quoted: boolean };
  path: string[];
  prefixLength: number;
  nested: boolean;
}

/** Collects literal setData paths, rejecting dynamic object members. */
function collectSetDataPaths(
  object: ts.ObjectLiteralExpression,
  prefix: string[],
  sourceFile: ts.SourceFile,
  state: ScanState,
  out: LiteralSetDataPath[],
): void {
  for (const property of object.properties) {
    if (ts.isSpreadAssignment(property)) {
      reject(state, "a setData object contains a spread member");
      continue;
    }
    if (ts.isComputedPropertyName(property.name)) {
      reject(state, "a setData object contains a computed key");
      continue;
    }
    if (!ts.isPropertyAssignment(property)) {
      reject(state, "a setData object contains a shorthand or method member");
      continue;
    }
    const site = propertyNameSite(property.name, sourceFile);
    if (!site) {
      reject(state, "a setData key cannot be mapped to an exact token");
      continue;
    }
    if (
      site.name.length === 0 ||
      site.name.includes("[") ||
      site.name.includes("]") ||
      site.name.startsWith(".") ||
      site.name.endsWith(".") ||
      site.name.includes("..")
    ) {
      reject(state, "a setData key uses an unsupported bracket or dotted path");
      continue;
    }
    const segments = site.name.split(".");
    if (segments.some((segment) => segment.length === 0)) {
      reject(state, "a setData key contains an empty segment");
      continue;
    }
    const path = [...prefix, ...segments];
    const nested = ts.isObjectLiteralExpression(property.initializer);
    out.push({ site, path, prefixLength: prefix.length, nested });
    if (nested) {
      collectSetDataPaths(property.initializer, path, sourceFile, state, out);
    }
  }
}

/** Returns whether one path is a prefix of the other (or equal). */
function pathsIntersect(left: string[], right: string[]): boolean {
  const depth = Math.min(left.length, right.length);
  for (let index = 0; index < depth; index += 1) {
    if (left[index] !== right[index]) return false;
  }
  return true;
}

/** Exact token for the target segment inside one literal setData path. */
function setDataOccurrence(
  entry: LiteralSetDataPath,
  target: string[],
  file: FileInfo,
): { token: SymbolWireToken; path: string[]; segment: number } | undefined {
  if (entry.path.length < target.length) return undefined;
  for (let index = 0; index < target.length; index += 1) {
    if (entry.path[index] !== target[index]) return undefined;
  }
  const segment = target.length - 1;
  if (segment < entry.prefixLength) return undefined;
  const segments = entry.site.name.split(".");
  const local = segment - entry.prefixLength;
  if (local < 0 || local >= segments.length) return undefined;
  let offset = 0;
  for (let index = 0; index < local; index += 1) {
    offset += segments[index]!.length + 1;
  }
  const start = entry.site.start + offset;
  const token = tokenOf(file, start, start + segments[local]!.length);
  return token ? { token, path: entry.path, segment } : undefined;
}

/** Scan literal `setData({...})` calls in the subject script. */
function scanSetData(
  sourceFile: ts.SourceFile,
  file: FileInfo,
  target: string[],
  state: ScanState,
  occurrences: SymbolWireOccurrence[],
): void {
  const visit = (node: ts.Node): void => {
    if (ts.isCallExpression(node) && isSetDataCallee(node.expression)) {
      if (!isThisSetData(node.expression)) {
        reject(state, "a setData call on an unknown receiver may reference this data");
        node.forEachChild(visit);
        return;
      }
      const argument = node.arguments[0];
      if (!argument || !ts.isObjectLiteralExpression(argument)) {
        reject(state, "a setData call does not use a literal object argument");
      } else {
        const entries: LiteralSetDataPath[] = [];
        collectSetDataPaths(argument, [], sourceFile, state, entries);
        for (const entry of entries) {
          if (!pathsIntersect(entry.path, target)) continue;
          if (entry.path.length < target.length) {
            if (!entry.nested) {
              reject(state, "a setData key hides a nested target path");
            }
            continue;
          }
          const occurrence = setDataOccurrence(entry, target, file);
          if (occurrence) {
            occurrences.push({
              ...occurrence.token,
              kind: "setDataKey",
              path: occurrence.path,
              segment: occurrence.segment,
            });
          }
        }
      }
    }
    node.forEachChild(visit);
  };
  visit(sourceFile);
}

/** Split an observer path list such as `"user.name, user.age"` into tokens. */
function observerPaths(
  site: { start: number; end: number; name: string; quoted: boolean },
  file: FileInfo,
  target: string[],
  state: ScanState,
  occurrences: SymbolWireOccurrence[],
): void {
  const text = site.name;
  let index = 0;
  while (index < text.length) {
    while (index < text.length && (text[index] === "," || /\s/.test(text[index]!))) index += 1;
    const start = index;
    while (index < text.length && text[index] !== "," && !/\s/.test(text[index]!)) index += 1;
    if (index === start) break;
    const raw = text.slice(start, index);
    const path = raw.split(".");
    if (path.some((segment) => segment.length === 0)) {
      reject(state, "an observer path is empty or malformed");
      continue;
    }
    if (raw.includes("[") || raw.includes("]")) {
      reject(state, "an observer path uses unsupported bracket syntax");
      continue;
    }
    if (path.length < target.length) continue;
    let matches = true;
    for (let depth = 0; depth < target.length; depth += 1) {
      const segment = path[depth]!;
      if (segment === "*" || segment === "**") continue;
      if (segment !== target[depth]) {
        matches = false;
        break;
      }
    }
    if (!matches) continue;
    const segment = target.length - 1;
    const targetSegment = path[segment]!;
    if (targetSegment === "*" || targetSegment === "**") continue;
    let offset = start;
    for (let depth = 0; depth < segment; depth += 1) offset += path[depth]!.length + 1;
    const token = tokenOf(file, site.start + offset, site.start + offset + targetSegment.length);
    if (token) {
      occurrences.push({ ...token, kind: "observerPath", path, segment });
    }
  }
}

/** Other scripts in the snapshot may call page.setData with a literal path. */
function scanOtherScriptSetData(
  index: ScriptIndex,
  target: string[],
  state: ScanState,
): void {
  for (const file of index.files.values()) {
    if (file.path === index.subject.path) continue;
    const sourceFile = index.project.program().getSourceFile(file.absolute);
    if (!sourceFile) continue;
    const ownRegistration = findRegistrations(sourceFile).registrations.length > 0;
    const visit = (node: ts.Node): void => {
      if (
        ts.isCallExpression(node) &&
        ts.isPropertyAccessExpression(node.expression) &&
        node.expression.name.text === "setData"
      ) {
        // `this.setData` in a file with its own Page/Component registration
        // writes that owner's data, not the current view's data.
        if (ownRegistration && isThisSetData(node.expression)) {
          node.forEachChild(visit);
          return;
        }
        const argument = node.arguments[0];
        if (!argument || !ts.isObjectLiteralExpression(argument)) {
          reject(state, "another script calls setData with a dynamic argument");
        } else {
          const entries: LiteralSetDataPath[] = [];
          collectSetDataPaths(argument, [], sourceFile, state, entries);
          for (const entry of entries) {
            if (!pathsIntersect(entry.path, target)) continue;
            reject(state, "another script calls setData with this data path");
            break;
          }
        }
      }
      node.forEachChild(visit);
    };
    visit(sourceFile);
  }
}

/** Scan component `observers` keys and property `observer` option strings. */
function scanObservers(
  project: VirtualProject,
  registration: { kind: "page" | "component"; options: ts.ObjectLiteralExpression },
  target: string[],
  state: ScanState,
  occurrences: SymbolWireOccurrence[],
): void {
  if (registration.kind !== "component") return;
  const members = memberMap(registration.options);
  const observers = members.get("observers");
  if (observers) {
    let resolved: { object: ts.ObjectLiteralExpression; sourceFile: ts.SourceFile; file: FileInfo } | undefined;
    if (ts.isPropertyAssignment(observers)) {
      resolved = resolveObjectLiteral(project, observers.initializer, 0, new Set());
    }
    if (!resolved) {
      reject(state, "component observers are not a statically resolvable object literal");
    } else {
      for (const property of resolved.object.properties) {
        if (ts.isSpreadAssignment(property) || ts.isComputedPropertyName(property.name)) {
          reject(state, "component observers contain a spread or computed key");
          continue;
        }
        const site = propertyNameSite(property.name, resolved.sourceFile);
        if (!site) {
          reject(state, "an observer key cannot be mapped to an exact token");
          continue;
        }
        observerPaths(site, resolved.file, target, state, occurrences);
      }
    }
  }
  const properties = members.get("properties");
  if (!properties) return;
  if (!ts.isPropertyAssignment(properties)) {
    reject(state, "component properties are not a property assignment");
    return;
  }
  const resolvedProperties = resolveObjectLiteral(project, properties.initializer, 0, new Set());
  if (!resolvedProperties) {
    reject(state, "component properties are not a statically resolvable object literal");
    return;
  }
  for (const property of resolvedProperties.object.properties) {
    if (!ts.isPropertyAssignment(property) || !ts.isObjectLiteralExpression(property.initializer)) {
      continue;
    }
    const options = memberMap(property.initializer);
    const observer = options.get("observer");
    if (!observer) continue;
    if (!ts.isPropertyAssignment(observer)) {
      reject(state, "a property observer is not a literal string");
      continue;
    }
    const value = observer.initializer;
    if (!ts.isStringLiteral(value) && !ts.isNoSubstitutionTemplateLiteral(value)) {
      reject(state, "a property observer is not a literal string");
      continue;
    }
    observerPaths(
      {
        start: value.getStart(resolvedProperties.sourceFile) + 1,
        end: value.getEnd() - 1,
        name: value.text,
        quoted: true,
      },
      resolvedProperties.file,
      target,
      state,
      occurrences,
    );
  }
}

/* ------------------------------------------------------------------ insertions */

function lineIndent(text: string, position: number): string {
  const start = text.lastIndexOf("\n", Math.max(0, position - 1)) + 1;
  let end = start;
  while (end < text.length && (text[end] === " " || text[end] === "\t")) end += 1;
  return text.slice(start, end);
}

function newlineOf(text: string): string {
  return text.includes("\r\n") ? "\r\n" : "\n";
}

function isValidIdentifier(name: string): boolean {
  return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name);
}

function insertionForHandler(
  facts: ReturnType<typeof handlerFacts>,
  sourceFile: ts.SourceFile,
  file: FileInfo,
  name: string,
  kind: "page" | "component",
  state: ScanState,
): { fileId: string; offset: number; text: string } | undefined {
  if (!isValidIdentifier(name)) {
    reject(state, "the handler name is not a valid identifier");
    return undefined;
  }
  const container = facts.container;
  if (!container) {
    reject(state, "the registration has no literal object to extend");
    return undefined;
  }
  const text = file.text;
  const newline = newlineOf(text);
  const close = container.getEnd() - 1;
  if (close < 0 || close >= text.length || text[close] !== "}") {
    reject(state, "the registration object has no literal closing brace");
    return undefined;
  }
  const open = container.getStart(sourceFile);
  const containerIndent = lineIndent(text, open);
  const properties = container.properties;
  let indent: string;
  if (properties.length > 0) {
    const first = properties[0]!;
    const line = file.text.lastIndexOf("\n", Math.max(0, first.getStart(sourceFile) - 1)) + 1;
    indent = file.text.slice(line, first.getStart(sourceFile));
    if (!/^[ \t]*$/.test(indent)) indent = `${containerIndent}  `;
  } else {
    indent = `${containerIndent}  `;
  }
  const handlerMember = `${name}() {${newline}${indent}  ${newline}${indent}}`;
  let member = handlerMember;
  if (kind === "component" && !facts.methods) {
    // Insert `methods: { ... }` as a normal member. A member inserted before
    // existing properties/data must carry a trailing comma.
    const inner = `${indent}  `;
    member = `methods: {${newline}${inner}${name}() {${newline}${inner}  ${newline}${inner}},${newline}${indent}}`;
  }
  let prefix = "";
  if (properties.length > 0) {
    const last = properties[properties.length - 1]!;
    const gap = text.slice(last.getEnd(), close);
    if (/\/[/*]/.test(gap)) {
      reject(state, "comments before the closing brace make the insertion anchor ambiguous");
      return undefined;
    }
    const trimmed = gap.trim();
    const needsComma = trimmed.length === 0;
    if (!needsComma && trimmed !== ",") {
      reject(state, "the closing brace is not a clean insertion anchor");
      return undefined;
    }
    prefix = needsComma ? "," : "";
  }
  const trailingComma = properties.length > 0 ? "," : "";
  const insertText = `${prefix}${newline}${indent}${member}${trailingComma}${newline}${containerIndent}`;
  if (insertText.length > MAX_INSERTION_TEXT) {
    reject(state, "the handler insertion text is too large");
    return undefined;
  }
  const offset = file.byte(close);
  if (offset === undefined) {
    reject(state, "the handler insertion offset cannot be mapped");
    return undefined;
  }
  return { fileId: file.path, offset, text: insertText };
}

/** Static member names of one component option object. */
function objectMemberNames(
  object: ts.ObjectLiteralExpression,
  state: ScanState,
): string[] {
  const names: string[] = [];
  for (const property of object.properties) {
    if (ts.isSpreadAssignment(property) || ts.isComputedPropertyName(property.name)) {
      reject(state, "a component owner object contains a spread or computed member");
      continue;
    }
    const name = propertyName(property);
    if (name === undefined) {
      reject(state, "a component owner member has no static name");
      continue;
    }
    names.push(name);
  }
  return names;
}

/**
 * Names already declared by the owner registration.
 *
 * For components `data`, `properties` and `methods` share one runtime
 * namespace. The symbol query therefore returns the union so a rename cannot
 * introduce the same key into two option objects.
 */
function ownerReservedNames(
  sourceFile: ts.SourceFile,
  registration: { kind: "page" | "component"; options: ts.ObjectLiteralExpression },
  state: ScanState,
): string[] {
  const names = new Set(
    handlerFacts(sourceFile, registration.kind, "__wxml_missing__").taken,
  );
  if (registration.kind === "component") {
    const members = memberMap(registration.options);
    for (const option of ["data", "properties", "methods"] as const) {
      const member = members.get(option);
      if (!member) continue;
      if (ts.isPropertyAssignment(member) && ts.isObjectLiteralExpression(member.initializer)) {
        for (const name of objectMemberNames(member.initializer, state)) names.add(name);
      } else {
        reject(state, `component \`${option}\` members are not statically readable`);
      }
    }
  }
  return [...names];
}

/* -------------------------------------------------------------------- queries */

/**
 * Answer one script symbol query.
 *
 * Throws `RequestError` only for malformed requests. Every valid request gets
 * a `SymbolsResponse`; degraded coverage is expressed with `state`/`complete`,
 * never by fabricating tokens.
 */
export function symbols(request: SymbolsRequest, config: SymbolsConfig): SymbolsResponse {
  const state: ScanState = { complete: true, reasons: [], dynamic: false };
  const protocolVersion = PROTOCOL_VERSION;
  const empty = (): SymbolsResponse => ({
    protocolVersion,
    requestId: request.requestId,
    state: { status: "unsupported", reason: state.reasons[0] ?? "script symbols are unavailable" },
    complete: false,
    references: [],
    occurrences: [],
    fields: [],
    reserved: [],
    ...(state.reasons.length > 1 ? { reason: state.reasons.join("; ") } : {}),
  });

  try {
    assertEnvironment(config.backendDir, toCompilerOptions(request.project.compilerOptions));
  } catch (error) {
    const reason = error instanceof EnvironmentError ? error.message : String(error);
    return {
      ...empty(),
      state: { status: "unavailable", reason: `type environment is incomplete: ${bounded(reason, 200)}` },
    };
  }

  const compilerOptions = toCompilerOptions(request.project.compilerOptions);
  if (compilerOptions.typeRoots) {
    compilerOptions.typeRoots.push(path.join(config.backendDir, "node_modules"));
  }
  const readPolicy = defaultReadPolicy(config.backendDir);
  for (const typeRoot of compilerOptions.typeRoots ?? []) {
    readPolicy.diskRoots.push(path.resolve(typeRoot));
  }
  const project = new VirtualProject({
    rootDir: config.backendDir,
    compilerOptions,
    files: request.files.map((file) => ({
      path: path.resolve(request.project.projectRoot, file.path),
      text: file.text,
    })),
    readPolicy,
  });
  const files = buildFileMap(project, request);
  const subjectAbsolute = path.resolve(request.project.projectRoot, request.subjectFileId);
  const subject = files.get(subjectAbsolute);
  if (!subject) return empty();
  let sourceFile = project.program().getSourceFile(subjectAbsolute);
  if (!sourceFile) {
    try {
      sourceFile = parseSource(subject.text, subjectAbsolute);
    } catch {
      return { ...empty(), state: { status: "partial", reason: "the subject script could not be parsed" } };
    }
  }
  subject.source = sourceFile;
  const registration = resolveRegistration(sourceFile, request.root, state);
  const index: ScriptIndex = {
    project,
    files,
    subject,
    sourceFile,
    ...(registration ? { registration } : {}),
    state,
  };

  const query = request.query;
  const result: SymbolsResponse = {
    protocolVersion,
    requestId: request.requestId,
    state: { status: "supported" },
    complete: true,
    references: [],
    occurrences: [],
    fields: [],
    reserved: [],
  };

  if (request.sourceComplete === false) {
    return {
      ...empty(),
      state: {
        status: "partial",
        reason: "the project source snapshot was truncated",
      },
      reason: "the project source snapshot was truncated",
    };
  }

  if (query.kind === "eventHandler") {
    const actualKind = registration?.kind ?? request.root;
    const facts = handlerFacts(sourceFile, actualKind, query.name);
    if (facts.state.status !== "supported") {
      reject(state, facts.state.reason ?? "the handler owner table is incomplete");
    }
    result.fields = facts.taken.map((name) => ({ name, quoted: false }));
    result.reserved = [...facts.taken];
    if (facts.nameNode) {
      const token = nameToken(facts.nameNode as ts.PropertyName, subject, sourceFile);
      if (token) result.declaration = token.token;
      result.references = collectReferences(index, facts.nameNode, state);
    } else if (facts.opaque) {
      reject(state, "the handler exists but is not a literal function");
    } else if (state.complete) {
      const insertion = registration
        ? insertionForHandler(facts, sourceFile, subject, query.name, actualKind, state)
        : undefined;
      if (insertion) result.insertion = insertion;
    }
  } else {
    if (!registration) {
      return {
        ...empty(),
        fields: [],
        state: { status: "unsupported", reason: state.reasons[0] ?? "no registration for the data root" },
      };
    }
    const roots = dataRootObjects(project, registration, state);
    if (query.kind === "dataRoots") {
      const names = new Map<string, NamedProperty>();
      for (const root of [roots.data, roots.properties]) {
        if (!root) continue;
        const candidates = propertyCandidates(root.object, root.file, root.sourceFile, state);
        for (const [name, entries] of candidates) {
          if (entries.length > 1 || names.has(name)) {
            reject(state, `duplicate data member ${bounded(name, 80)}`);
            continue;
          }
          names.set(name, entries[0]!);
        }
      }
      result.fields = [...names.values()].map((entry) => ({
        name: entry.name,
        declaration: entry.token,
        quoted: entry.quoted,
      }));
      result.reserved = ownerReservedNames(sourceFile, registration, state);
    } else {
      const resolved = resolveDataPath(project, roots, query.path, state);
      if (resolved.declaration) {
        result.declaration = resolved.declaration.token;
        result.references = collectReferences(index, resolved.declaration.node.name, state);
      }
      result.fields = [...resolved.fields.values()]
        .filter((entries) => entries.length === 1)
        .map((entries) => {
          const entry = entries[0]!;
          return { name: entry.name, declaration: entry.token, quoted: entry.quoted };
        });
      result.reserved = ownerReservedNames(sourceFile, registration, state);
      scanDataEscapes(sourceFile, state);
      scanOtherScriptSetData(index, query.path, state);
      scanSetData(sourceFile, subject, query.path, state, result.occurrences);
      scanObservers(project, registration, query.path, state, result.occurrences);
      if (resolved.declaration && resolved.declaration.file.path !== subject.path) {
        reject(state, "shared data declarations are not supported for complete rename");
      }
    }
  }

  result.complete = state.complete;
  if (!state.complete) {
    result.state = { status: "partial", reason: state.reasons[0] ?? "script symbol coverage is partial" };
    result.reason = state.reasons.join("; ");
  }
  return result;
}
