/**
 * Static `Behavior({...})` resolution for components.
 *
 * WeChat components may compose shared state through `Component({ behaviors:
 * [shared, ...] })`. The TypeScript typings do not merge a behavior's `data`
 * or `properties` into the instance type, so the declaration overlay would
 * report every shared field as missing. This module resolves the static subset
 * the runtime uses:
 *
 * - inline `Behavior({...})` calls;
 * - `const shared = Behavior({...})` in the same file;
 * - local relative imports (`import shared from "./shared"`,
 *   `import { shared } from "./shared"`, `const shared = require("./shared")`);
 * - nested `behaviors: [...]` inside a behavior;
 * - local `export default`, named exports and `module.exports`.
 *
 * Precedence follows the WeChat model: the component's own `data`/`properties`
 * win, later behaviors win over earlier ones, and a behavior's own fields win
 * over its nested behaviors. Cycles, factory calls, path aliases, dynamic
 * arrays and files outside the request snapshot are reported as problems; the
 * affected members are skipped or typed `any`, never guessed.
 *
 * Only `data` and `properties` are summarized. Methods are handled by the
 * event resolver, which has its own conservative behavior rule.
 */

import path from "node:path";
import ts from "typescript";
import { docTextOf, memberMap, propertyName, type DeclaredProperty, type Registration } from "./ast.ts";

/** Upper bounds that keep hostile behavior graphs bounded. */
const MAX_BEHAVIORS = 64;
const MAX_DEPTH = 16;
const MAX_MEMBERS = 512;

/** One resolved behavior declaration. */
export interface BehaviorUnit {
  /** Absolute script path. */
  path: string;
  /** Parsed source file (project snapshot, never disk fallback). */
  sourceFile: ts.SourceFile;
  /** The `Behavior({...})` options object. */
  options: ts.ObjectLiteralExpression;
  /** Stable identity used for cycles. */
  identity: string;
  /** Description used in degraded reasons, for example `./shared`. */
  label: string;
}

/** One `data` member contributed by a behavior. */
export interface BehaviorDataMember {
  name: string;
  /** Conservative TypeScript type text. */
  typeText: string;
  /** False when the initializer is not a self-contained literal. */
  exact: boolean;
  doc: string;
  /** Absolute source file. */
  file: string;
  /** Byte range of the member name in `file`. */
  start: number;
  end: number;
  /** Explicit source mapping for generated hover/definition. */
  source: { file: string; start: number; end: number };
  /** True when the component's own `data` declares the same name. */
  ownOverride?: boolean;
}

/** A behavior-contributed component property with its original declaration. */
export interface BehaviorProperty extends DeclaredProperty {
  /** Absolute source file. */
  file: string;
  /** Byte range of the property name in `file`. */
  start: number;
  end: number;
}

export interface BehaviorResolution {
  /** Merged behavior `data`, later behaviors overriding earlier ones. */
  data: BehaviorDataMember[];
  /** Merged behavior `properties`; the component's own declarations must be
   * merged first by the caller. */
  properties: BehaviorProperty[];
  /** `true` when at least one data member is typed `any` or was skipped. */
  dataIncomplete: boolean;
  /** `true` when at least one property declaration was skipped or unreadable. */
  propertiesIncomplete: boolean;
  /** Human-readable degradation reasons (bounded). */
  problems: string[];
}

export interface BehaviorInput {
  /** Absolute owning script path. */
  scriptPath: string;
  /** Parsed owning script. */
  sourceFile: ts.SourceFile;
  /** First component registration of the owning script, when present. */
  registration?: Registration;
  /** Snapshot-only module resolver: absolute path to parsed source. */
  getSourceFile: (absolutePath: string) => ts.SourceFile | undefined;
  /** Checker of the request snapshot, for non-literal behavior initializers. */
  checker?: ts.TypeChecker;
}

/** Resolves the static behavior graph of one component. */
export function resolveBehaviors(input: BehaviorInput): BehaviorResolution {
  const problems: string[] = [];
  const units: BehaviorUnit[] = [];
  const stack = new Set<string>();
  const options = input.registration?.options;
  if (!options) {
    return { data: [], properties: [], dataIncomplete: false, propertiesIncomplete: false, problems };
  }

  const behaviorsMember = memberMap(options).get("behaviors");
  if (behaviorsMember === undefined) {
    return { data: [], properties: [], dataIncomplete: false, propertiesIncomplete: false, problems };
  }
  if (!ts.isPropertyAssignment(behaviorsMember) || !ts.isArrayLiteralExpression(behaviorsMember.initializer)) {
    problems.push("component `behaviors` is not a static array literal");
    return {
      data: [],
      properties: [],
      dataIncomplete: true,
      propertiesIncomplete: true,
      problems,
    };
  }

  for (const element of behaviorsMember.initializer.elements) {
    if (units.length >= MAX_BEHAVIORS) {
      problems.push("behavior budget reached; later behaviors were skipped");
      break;
    }
    const unit = resolveBehaviorExpression(element, input.sourceFile, input, stack, 0, problems);
    if (unit) collectUnit(unit, input, units, stack, 0, problems);
  }

  return mergeUnits(units, options, input, problems);
}

/**
 * Merges resolved units: later units win, so iterate in reverse and keep the
 * first occurrence. The component's own declarations are excluded here and
 * merged by the caller (own always wins).
 */
function mergeUnits(
  units: BehaviorUnit[],
  options: ts.ObjectLiteralExpression,
  input: BehaviorInput,
  problems: string[],
): BehaviorResolution {
  const ownData = memberMap(options).get("data");
  const ownDataMembers = new Map<string, ts.PropertyAssignment>();
  if (ownData && ts.isPropertyAssignment(ownData) && ts.isObjectLiteralExpression(ownData.initializer)) {
    for (const property of ownData.initializer.properties) {
      if (!ts.isPropertyAssignment(property)) continue;
      const name = propertyName(property);
      if (name !== undefined) ownDataMembers.set(name, property);
    }
  }

  const data = new Map<string, BehaviorDataMember>();
  const properties = new Map<string, BehaviorProperty>();
  let dataIncomplete = false;
  let propertiesIncomplete = false;

  for (const unit of [...units].reverse()) {
    const members = memberMap(unit.options);
    const dataMember = members.get("data");
    if (dataMember && ts.isPropertyAssignment(dataMember) && ts.isObjectLiteralExpression(dataMember.initializer)) {
      for (const property of dataMember.initializer.properties) {
        if (ts.isSpreadAssignment(property)) {
          problems.push(`behavior \`${unit.label}\` has a spread in \`data\``);
          dataIncomplete = true;
          continue;
        }
        if (!ts.isPropertyAssignment(property) && !ts.isMethodDeclaration(property)) {
          problems.push(`behavior \`${unit.label}\` has a shorthand/method data member`);
          dataIncomplete = true;
          continue;
        }
        if (!ts.isPropertyAssignment(property)) {
          problems.push(`behavior \`${unit.label}\` data member is a method`);
          dataIncomplete = true;
          continue;
        }
        const name = propertyName(property);
        if (name === undefined) {
          problems.push(`behavior \`${unit.label}\` has a computed data member`);
          dataIncomplete = true;
          continue;
        }
        if (data.has(name)) continue;
        if (data.size >= MAX_MEMBERS) {
          problems.push("behavior data member budget reached");
          dataIncomplete = true;
          break;
        }
        const own = ownDataMembers.get(name);
        const owner = own ?? property;
        const ownerFile = own ? options.getSourceFile() : unit.sourceFile;
        const ownerPath = own ? ownerFile.fileName : unit.path;
        const inferred = typeOfInitializer(owner.initializer, ownerFile, input.checker);
        if (!inferred.exact && own === undefined) {
          dataIncomplete = true;
          problems.push(
            `behavior data member \`${name}\` is typed any: its initializer is not resolvable in the component scope`,
          );
        }
        data.set(name, {
          name,
          typeText: inferred.text,
          exact: inferred.exact,
          doc: docTextOf(owner),
          ...byteSpanOf(ownerFile, owner.name ?? owner),
          file: ownerPath,
          source: {
            file: ownerPath,
            ...byteSpanOf(ownerFile, owner.name ?? owner),
          },
          ...(own ? { ownOverride: true } : {}),
        });
      }
    }

    const propertyMember = members.get("properties");
    if (propertyMember && ts.isPropertyAssignment(propertyMember) && ts.isObjectLiteralExpression(propertyMember.initializer)) {
      for (const property of propertyMember.initializer.properties) {
        if (ts.isSpreadAssignment(property) || !ts.isPropertyAssignment(property)) {
          problems.push(`behavior \`${unit.label}\` has a non-literal \`properties\` member`);
          propertiesIncomplete = true;
          continue;
        }
        const name = propertyName(property);
        if (name === undefined || !ts.isObjectLiteralExpression(property.initializer)) {
          problems.push(`behavior \`${unit.label}\` has a computed/invalid property declaration`);
          propertiesIncomplete = true;
          continue;
        }
        if (properties.has(name)) continue;
        if (properties.size >= MAX_MEMBERS) {
          problems.push("behavior property budget reached");
          propertiesIncomplete = true;
          break;
        }
        const option = memberMap(property.initializer);
        const typeText = unionText(option.get("type"), option.get("optionalTypes"));
        if (!typeText) {
          problems.push(`behavior property \`${name}\` has no readable \`type\``);
          propertiesIncomplete = true;
          continue;
        }
        properties.set(name, {
          name,
          typeText,
          hasOptionalTypes: option.has("optionalTypes"),
          doc: docTextOf(property),
          nameNode: property.name ?? property,
          initializer: property.initializer,
          file: unit.path,
          ...byteSpanOf(unit.sourceFile, property.name ?? property),
          source: {
            file: unit.path,
            ...byteSpanOf(unit.sourceFile, property.name ?? property),
          },
        });
      }
    }
  }

  return {
    data: [...data.values()],
    properties: [...properties.values()],
    dataIncomplete,
    propertiesIncomplete,
    problems: dedupe(problems),
  };
}

/** Depth-first collection: nested behaviors are applied before their owner. */
function collectUnit(
  unit: BehaviorUnit,
  input: BehaviorInput,
  units: BehaviorUnit[],
  stack: Set<string>,
  depth: number,
  problems: string[],
): void {
  if (depth > MAX_DEPTH) {
    problems.push(`behavior nesting is too deep at \`${unit.label}\``);
    return;
  }
  if (stack.has(unit.identity)) {
    problems.push(`behavior cycle at \`${unit.label}\``);
    return;
  }
  if (units.length >= MAX_BEHAVIORS) {
    problems.push("behavior budget reached; later behaviors were skipped");
    return;
  }
  stack.add(unit.identity);
  const nested = memberMap(unit.options).get("behaviors");
  if (nested !== undefined) {
    if (!ts.isPropertyAssignment(nested) || !ts.isArrayLiteralExpression(nested.initializer)) {
      problems.push(`behavior \`${unit.label}\` has a non-static \`behaviors\``);
    } else {
      for (const element of nested.initializer.elements) {
        const child = resolveBehaviorExpression(element, unit.sourceFile, input, stack, depth + 1, problems);
        if (child) collectUnit(child, input, units, stack, depth + 1, problems);
      }
    }
  }
  units.push(unit);
  stack.delete(unit.identity);
}

function resolveBehaviorExpression(
  expression: ts.Expression,
  sourceFile: ts.SourceFile,
  input: BehaviorInput,
  stack: Set<string>,
  depth: number,
  problems: string[],
): BehaviorUnit | undefined {
  if (depth > MAX_DEPTH) {
    problems.push("behavior nesting is too deep");
    return undefined;
  }
  if (ts.isParenthesizedExpression(expression)) {
    return resolveBehaviorExpression(expression.expression, sourceFile, input, stack, depth, problems);
  }
  if (ts.isCallExpression(expression) && ts.isIdentifier(expression.expression) && expression.expression.text === "Behavior") {
    const argument = expression.arguments[0];
    if (!argument || !ts.isObjectLiteralExpression(argument)) {
      problems.push("`Behavior(...)` argument is not an object literal");
      return undefined;
    }
    const identity = `${sourceFile.fileName}:${argument.pos}`;
    return { path: sourceFile.fileName, sourceFile, options: argument, identity, label: labelOf(sourceFile) };
  }
  if (ts.isIdentifier(expression)) {
    return resolveBinding(expression.text, sourceFile, input, stack, depth, problems);
  }
  problems.push("behavior entry is a dynamic expression (factory, conditional or call)");
  return undefined;
}

function resolveBinding(
  name: string,
  sourceFile: ts.SourceFile,
  input: BehaviorInput,
  stack: Set<string>,
  depth: number,
  problems: string[],
): BehaviorUnit | undefined {
  const binding = findModuleBinding(sourceFile, name);
  if (!binding) {
    problems.push(`behavior binding \`${name}\` was not found in ${labelOf(sourceFile)}`);
    return undefined;
  }
  if (binding.kind === "expression") {
    return resolveBehaviorExpression(binding.expression, sourceFile, input, stack, depth + 1, problems);
  }
  return resolveExport(binding.module, binding.exportName, sourceFile, input, stack, depth + 1, problems);
}

type ModuleBinding =
  | { kind: "expression"; expression: ts.Expression }
  | { kind: "import"; module: string; exportName: string | undefined };

/** Finds a top-level variable declaration or import binding. */
function findModuleBinding(sourceFile: ts.SourceFile, name: string): ModuleBinding | undefined {
  for (const statement of sourceFile.statements) {
    if (ts.isVariableStatement(statement)) {
      for (const declaration of statement.declarationList.declarations) {
        if (ts.isIdentifier(declaration.name) && declaration.name.text === name) {
          return declaration.initializer === undefined
            ? undefined
            : { kind: "expression", expression: declaration.initializer };
        }
      }
    }
    if (ts.isImportDeclaration(statement) && statement.importClause) {
      const clause = statement.importClause;
      if (clause.name?.text === name) {
        return { kind: "import", module: statement.moduleSpecifier.getText().replace(/^['"]|['"]$/g, ""), exportName: undefined };
      }
      const bindings = clause.namedBindings;
      if (bindings && ts.isNamedImports(bindings)) {
        for (const element of bindings.elements) {
          if (element.name.text === name) {
            return {
              kind: "import",
              module: statement.moduleSpecifier.getText().replace(/^['"]|['"]$/g, ""),
              exportName: (element.propertyName ?? element.name).text,
            };
          }
        }
      }
    }
  }
  return undefined;
}

function resolveExport(
  specifier: string,
  exportName: string | undefined,
  fromFile: ts.SourceFile,
  input: BehaviorInput,
  stack: Set<string>,
  depth: number,
  problems: string[],
): BehaviorUnit | undefined {
  if (!specifier.startsWith("./") && !specifier.startsWith("../")) {
    problems.push(`behavior import \`${specifier}\` is not a local relative import`);
    return undefined;
  }
  const resolved = resolveRelativeModule(fromFile.fileName, specifier, input.getSourceFile);
  if (!resolved) {
    problems.push(`behavior import \`${specifier}\` is not part of the request snapshot`);
    return undefined;
  }
  const exported = findExport(resolved, exportName);
  if (!exported) {
    problems.push(`behavior export \`${exportName ?? "default"}\` was not found in \`${specifier}\``);
    return undefined;
  }
  return resolveBehaviorExpression(exported, resolved, input, stack, depth + 1, problems);
}

function findExport(sourceFile: ts.SourceFile, exportName: string | undefined): ts.Expression | undefined {
  const wantDefault = exportName === undefined;
  for (const statement of sourceFile.statements) {
    if (ts.isExportAssignment(statement)) {
      if (wantDefault) return statement.expression;
      continue;
    }
    if (ts.isExportDeclaration(statement)) {
      if (!statement.exportClause || !ts.isNamedExports(statement.exportClause)) continue;
      for (const element of statement.exportClause.elements) {
        if (element.name.text !== exportName) continue;
        const localName = element.propertyName?.text ?? element.name.text;
        if (statement.moduleSpecifier) {
          // A re-export from another module cannot be represented without
          // another resolver call; report it as unresolved instead of guessing.
          return undefined;
        }
        const binding = findModuleBinding(sourceFile, localName);
        return binding && binding.kind === "expression" ? binding.expression : undefined;
      }
      continue;
    }
    if (ts.isVariableStatement(statement)) {
      const isExported = hasExportModifier(statement);
      if (!isExported && !wantDefault) continue;
      for (const declaration of statement.declarationList.declarations) {
        if (!ts.isIdentifier(declaration.name)) continue;
        if (wantDefault) {
          if (declaration.name.text === "default" || declaration.name.text === "behavior") {
            return declaration.initializer;
          }
          continue;
        }
        if (declaration.name.text === exportName) return declaration.initializer;
      }
    }
    if (ts.isFunctionDeclaration(statement) && statement.name?.text === exportName) {
      return undefined;
    }
  }
  // CommonJS behavior modules in JavaScript.
  if (wantDefault) {
    for (const statement of sourceFile.statements) {
      if (!ts.isExpressionStatement(statement)) continue;
      const expression = statement.expression;
      if (
        ts.isBinaryExpression(expression) &&
        expression.operatorToken.kind === ts.SyntaxKind.EqualsToken &&
        ts.isPropertyAccessExpression(expression.left) &&
        ts.isIdentifier(expression.left.expression) &&
        expression.left.expression.text === "module" &&
        expression.left.name.text === "exports"
      ) {
        return expression.right;
      }
    }
  }
  return undefined;
}

function hasExportModifier(statement: ts.Statement): boolean {
  const modifiers = ts.canHaveModifiers(statement) ? ts.getModifiers(statement) : undefined;
  return modifiers?.some((modifier) => modifier.kind === ts.SyntaxKind.ExportKeyword) ?? false;
}

/** Resolves a relative module to a snapshot source file (no disk access). */
function resolveRelativeModule(
  fromFile: string,
  specifier: string,
  getSourceFile: (absolutePath: string) => ts.SourceFile | undefined,
): ts.SourceFile | undefined {
  const base = path.resolve(path.dirname(fromFile), specifier);
  const candidates = path.extname(base)
    ? [base]
    : [
        base,
        `${base}.ts`,
        `${base}.tsx`,
        `${base}.js`,
        `${base}.jsx`,
        `${base}.mts`,
        `${base}.cts`,
        path.join(base, "index.ts"),
        path.join(base, "index.tsx"),
        path.join(base, "index.js"),
        path.join(base, "index.jsx"),
      ];
  for (const candidate of candidates) {
    const sourceFile = getSourceFile(candidate);
    if (sourceFile) return sourceFile;
  }
  return undefined;
}

function labelOf(sourceFile: ts.SourceFile): string {
  return path.basename(sourceFile.fileName);
}

/** Byte range of a node in its own source file. */
function byteSpanOf(
  sourceFile: ts.SourceFile,
  node: ts.Node,
): { start: number; end: number } {
  const text = sourceFile.text;
  const start = node.getStart(sourceFile);
  const end = node.getEnd();
  return {
    start: Buffer.byteLength(text.slice(0, start), "utf8"),
    end: Buffer.byteLength(text.slice(0, end), "utf8"),
  };
}

/**
 * Conservative type and literal-ness of a behavior data initializer.
 *
 * `exact` is false when the value depends on module scope (an identifier,
 * call, template literal with substitutions), so the caller reports the field
 * as `any` with a reason instead of inventing a type.
 */
/**
 * True when a checker-rendered type names a type that is not a language
 * builtin. Such a name may live in the behavior module and is not in scope in
 * the owning component, so the caller falls back to `any` instead of emitting
 * an unresolved annotation (or a false diagnostic).
 */
function hasUnresolvedTypeReference(text: string): boolean {
  const parsed = ts.createSourceFile(
    "behavior-type.ts",
    `type __WxmlBehaviorType = ${text};`,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  );
  let unresolved = false;
  const allowed = new Set([
    "any",
    "unknown",
    "string",
    "number",
    "boolean",
    "object",
    "void",
    "undefined",
    "null",
    "never",
    "Record",
    "Array",
    "ReadonlyArray",
    "Partial",
    "Pick",
    "Omit",
    "Promise",
    "Date",
    "Function",
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
  ]);
  const visit = (node: ts.Node): void => {
    if (unresolved) return;
    if (ts.isTypeReferenceNode(node)) {
      const name = node.typeName.getText(parsed);
      if (!allowed.has(name)) unresolved = true;
      return;
    }
    if (ts.isTypeQueryNode(node)) {
      unresolved = true;
      return;
    }
    if (ts.isTypeLiteralNode(node)) {
      // Keys are property names, not type references; values are visited.
      for (const member of node.members) {
        if (ts.isPropertySignature(member) && member.type) visit(member.type);
      }
      return;
    }
    node.forEachChild(visit);
  };
  visit(parsed);
  return unresolved;
}

export function typeOfInitializer(
  expression: ts.Expression,
  sourceFile: ts.SourceFile,
  checker?: ts.TypeChecker,
  _getSourceFile?: (absolutePath: string) => ts.SourceFile | undefined,
): { text: string; exact: boolean } {
  const checked = (): { text: string; exact: boolean } | undefined => {
    if (!checker) return undefined;
    const type = checker.getTypeAtLocation(expression);
    if ((type.flags & (ts.TypeFlags.Any | ts.TypeFlags.Unknown)) !== 0) return undefined;
    const text = checker.typeToString(
      type,
      expression,
      ts.TypeFormatFlags.NoTruncation |
        ts.TypeFormatFlags.UseFullyQualifiedType |
        ts.TypeFormatFlags.UseAliasDefinedOutsideCurrentScope,
    );
    if (
      !text ||
      text.includes("import(") ||
      text.includes("/") ||
      text.includes("\\") ||
      hasUnresolvedTypeReference(text)
    ) {
      return undefined;
    }
    return { text, exact: true };
  };
  if (ts.isParenthesizedExpression(expression)) {
    return typeOfInitializer(expression.expression, sourceFile, checker);
  }
  if (ts.isStringLiteral(expression) || ts.isNoSubstitutionTemplateLiteral(expression)) {
    return { text: "string", exact: true };
  }
  if (ts.isNumericLiteral(expression)) return { text: "number", exact: true };
  if (ts.isPrefixUnaryExpression(expression) && expression.operator === ts.SyntaxKind.MinusToken) {
    return ts.isNumericLiteral(expression.operand)
      ? { text: "number", exact: true }
      : { text: "any", exact: false };
  }
  if (expression.kind === ts.SyntaxKind.TrueKeyword || expression.kind === ts.SyntaxKind.FalseKeyword) {
    return { text: "boolean", exact: true };
  }
  if (expression.kind === ts.SyntaxKind.NullKeyword) return { text: "any", exact: true };
  if (ts.isArrayLiteralExpression(expression)) {
    const types = new Set<string>();
    let exact = true;
    for (const element of expression.elements) {
      if (ts.isSpreadElement(element)) {
        exact = false;
        continue;
      }
      const inferred = typeOfInitializer(element, sourceFile, checker);
      if (!inferred.exact) exact = false;
      types.add(inferred.text);
    }
    if (!exact) return checked() ?? { text: "any[]", exact: false };
    if (types.size === 0) return { text: "any[]", exact: true };
    if (types.size === 1) return { text: `${[...types][0]}[]`, exact: true };
    return { text: `(${[...types].join(" | ")})[]`, exact: true };
  }
  if (ts.isObjectLiteralExpression(expression)) {
    const fields: string[] = [];
    let exact = true;
    for (const property of expression.properties) {
      if (!ts.isPropertyAssignment(property)) {
        exact = false;
        continue;
      }
      const name = propertyName(property);
      if (name === undefined) {
        exact = false;
        continue;
      }
      const inferred = typeOfInitializer(property.initializer, sourceFile, checker);
      if (!inferred.exact) exact = false;
      fields.push(`${JSON.stringify(name)}: ${inferred.text}`);
    }
    return exact ? { text: `{ ${fields.join("; ")} }`, exact: true } : checked() ?? { text: "any", exact: false };
  }
  return checked() ?? { text: "any", exact: false };
}

const PROPERTY_TYPE_TEXT: Record<string, string> = {
  String: "string",
  Number: "number",
  Boolean: "boolean",
  Array: "any[]",
  Object: "Record<string, any>",
  Function: "(...args: any[]) => any",
};

/** Union text from `type` plus `optionalTypes` (same rule as the overlay). */
export function unionText(
  typeMember?: ts.ObjectLiteralElementLike,
  optionalMember?: ts.ObjectLiteralElementLike,
): string | undefined {
  const parts: string[] = [];
  const main = constructorText(typeMember);
  if (main) parts.push(main);
  if (
    optionalMember &&
    ts.isPropertyAssignment(optionalMember) &&
    ts.isArrayLiteralExpression(optionalMember.initializer)
  ) {
    for (const element of optionalMember.initializer.elements) {
      const text = constructorText(element);
      if (text && !parts.includes(text)) parts.push(text);
    }
  }
  return parts.length > 0 ? parts.join(" | ") : undefined;
}

function constructorText(node: ts.Node | undefined): string | undefined {
  if (!node) return undefined;
  if (ts.isPropertyAssignment(node)) return constructorText(node.initializer);
  if (node.kind === ts.SyntaxKind.NullKeyword) return "any";
  if (ts.isIdentifier(node)) return PROPERTY_TYPE_TEXT[node.text];
  return undefined;
}

function dedupe(values: string[]): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const value of values) {
    if (seen.has(value)) continue;
    seen.add(value);
    result.push(value);
  }
  return result;
}
