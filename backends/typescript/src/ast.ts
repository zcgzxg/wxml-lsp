/**
 * Registration discovery and declaration reads.
 *
 * The probe mirrors the rule from ADR-0002: identify `Page`/`Component`
 * registrations through the AST, not by searching for strings, and read
 * `data`/`properties` declarations (including JSDoc) from the source nodes.
 */

import ts from "typescript";

export interface Registration {
  kind: "page" | "component";
  call: ts.CallExpression;
  options: ts.ObjectLiteralExpression;
  /** Registration runs at module top level (not inside a function). */
  topLevel: boolean;
}

const REGISTRARS: Record<string, "page" | "component"> = { Page: "page", Component: "component" };

export interface RegistrationScan {
  registrations: Registration[];
  /** Registration names that appear but cannot be used (reported, never guessed). */
  unsupportedCalls: string[];
}

export function findRegistrations(sourceFile: ts.SourceFile): RegistrationScan {
  const registrations: Registration[] = [];
  const unsupportedCalls: string[] = [];

  const visit = (node: ts.Node): void => {
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression)) {
      const name = node.expression.text;
      const kind = REGISTRARS[name];
      if (kind) {
        if (isLocallyShadowed(node.expression)) {
          unsupportedCalls.push(`${name} (shadowed identifier)`);
        } else {
          const argument = node.arguments[0];
          if (argument && ts.isObjectLiteralExpression(argument)) {
            registrations.push({ kind, call: node, options: argument, topLevel: isTopLevel(node) });
          } else {
            unsupportedCalls.push(`${name} (argument is not an object literal)`);
          }
        }
      }
    }
    node.forEachChild(visit);
  };
  visit(sourceFile);

  return { registrations, unsupportedCalls };
}

function isTopLevel(node: ts.Node): boolean {
  let current: ts.Node | undefined = node.parent;
  while (current) {
    if (ts.isFunctionLike(current) || ts.isClassLike(current) || ts.isBlock(current)) return false;
    current = current.parent;
  }
  return true;
}

function isLocallyShadowed(identifier: ts.Identifier): boolean {
  const name = identifier.text;
  let scope: ts.Node | undefined = identifier.parent;
  while (scope) {
    const statements = ts.isSourceFile(scope)
      ? scope.statements
      : ts.isBlock(scope) || ts.isModuleBlock(scope)
        ? scope.statements
        : undefined;
    if (statements) {
      for (const statement of statements) {
        if (ts.isFunctionDeclaration(statement) && statement.name?.text === name) return true;
        if (ts.isVariableStatement(statement)) {
          for (const declaration of statement.declarationList.declarations) {
            if (ts.isIdentifier(declaration.name) && declaration.name.text === name) return true;
          }
        }
        if (ts.isImportDeclaration(statement) && statement.importClause) {
          const clause = statement.importClause;
          if (clause.name?.text === name) return true;
          const bindings = clause.namedBindings;
          if (bindings && ts.isNamedImports(bindings)) {
            for (const element of bindings.elements) {
              if (element.name.text === name) return true;
            }
          }
          if (bindings && ts.isNamespaceImport(bindings) && bindings.name.text === name) return true;
        }
      }
    }
    scope = scope.parent;
  }
  return false;
}

/** First-level member map of an object literal (last assignment wins). */
export function memberMap(object: ts.ObjectLiteralExpression): Map<string, ts.ObjectLiteralElementLike> {
  const members = new Map<string, ts.ObjectLiteralElementLike>();
  for (const property of object.properties) {
    const name = propertyName(property);
    if (name !== undefined) members.set(name, property);
  }
  return members;
}

export function propertyName(property: ts.ObjectLiteralElementLike): string | undefined {
  if (property.name && (ts.isIdentifier(property.name) || ts.isStringLiteral(property.name))) {
    return property.name.text;
  }
  return undefined;
}

/** JSDoc comment text attached to a node, as plain text. */
export function docTextOf(node: ts.Node): string {
  for (const tag of ts.getJSDocCommentsAndTags(node)) {
    if (ts.isJSDoc(tag)) {
      const comment = tag.comment;
      if (typeof comment === "string") return comment.trim();
      if (comment) return comment.map((part) => part.text).join("").trim();
    }
  }
  return "";
}

export type DataRootKind = "object" | "expression" | "type-reference" | "absent" | "unsupported";

const PROPERTY_TYPE_TEXT: Record<string, string> = {
  String: "string",
  Number: "number",
  Boolean: "boolean",
  Array: "any[]",
  Object: "Record<string, any>",
  Function: "(...args: any[]) => any",
};

export interface DeclaredProperty {
  name: string;
  /** Union text derived from `type` plus `optionalTypes`, for example `string | number`. */
  typeText: string;
  /** True when the declaration lists `optionalTypes`. */
  hasOptionalTypes: boolean;
  /** JSDoc of the property declaration. */
  doc: string;
  /** Property name node in the original script (UTF-16 offsets, original file). */
  nameNode: ts.Node;
  /** The `{ type, value, ... }` initializer, when it is an object literal. */
  initializer?: ts.ObjectLiteralExpression;
  /** Override for declarations that live in another file (component behaviors). */
  source?: { file: string; start: number; end: number };
}

export interface PropertyScan {
  properties: DeclaredProperty[];
  /** False when spread/shorthand members hide declarations. */
  complete: boolean;
  problems: string[];
}

/**
 * Read component `properties` declarations into a type overlay.
 *
 * `miniprogram-api-typings@5.3.1` maps `FullProperty<T, V>` to `ValueType<T>`
 * and ignores `optionalTypes`, so `{ type: String, optionalTypes: [Number] }`
 * hovers as `string`. The overlay restores the declared union.
 *
 * Spread and shorthand members are reported instead of silently narrowed.
 */
export function scanDeclaredProperties(options: ts.ObjectLiteralExpression | undefined): PropertyScan {
  const properties: DeclaredProperty[] = [];
  const problems: string[] = [];
  if (!options) return { properties, complete: true, problems };

  const propertiesMember = memberMap(options).get("properties");
  if (!propertiesMember) return { properties, complete: true, problems };
  if (!ts.isPropertyAssignment(propertiesMember)) {
    return { properties, complete: false, problems: ["component `properties` is not a property assignment"] };
  }
  if (!ts.isObjectLiteralExpression(propertiesMember.initializer)) {
    return { properties, complete: false, problems: ["component `properties` is not an object literal"] };
  }

  for (const property of propertiesMember.initializer.properties) {
    if (ts.isSpreadAssignment(property)) {
      problems.push("`properties` uses a spread assignment; declared properties are incomplete");
      continue;
    }
    if (!ts.isPropertyAssignment(property)) {
      problems.push("`properties` uses a shorthand or method member; declared properties are incomplete");
      continue;
    }
    const name = propertyName(property);
    if (name === undefined) {
      problems.push("`properties` has a computed member name");
      continue;
    }
    if (!ts.isObjectLiteralExpression(property.initializer)) {
      problems.push(`property \`${name}\` is not declared with an object literal`);
      continue;
    }
    const option = memberMap(property.initializer);
    const typeText = unionText(option.get("type"), option.get("optionalTypes"));
    if (!typeText) {
      problems.push(`property \`${name}\` has no readable \`type\` declaration`);
      continue;
    }
    properties.push({
      name,
      typeText,
      hasOptionalTypes: option.has("optionalTypes"),
      doc: docTextOf(property),
      nameNode: property.name ?? property,
      initializer: property.initializer,
    });
  }

  return { properties, complete: problems.length === 0, problems };
}

function unionText(
  typeMember?: ts.ObjectLiteralElementLike,
  optionalMember?: ts.ObjectLiteralElementLike,
): string | undefined {
  const parts: string[] = [];
  const main = constructorText(typeMember);
  if (main) parts.push(main);
  if (optionalMember && ts.isPropertyAssignment(optionalMember) && ts.isArrayLiteralExpression(optionalMember.initializer)) {
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

export interface DeclarationSpan {
  name: string;
  kind: "data" | "properties";
  node: ts.Node;
}

/** Field declarations of `data` and `properties`, for definition/documentation tracing. */
export function declarationSpans(options: ts.ObjectLiteralExpression | undefined): DeclarationSpan[] {
  if (!options) return [];
  const spans: DeclarationSpan[] = [];
  for (const [kind, memberName] of [
    ["data", "data"],
    ["properties", "properties"],
  ] as const) {
    const member = memberMap(options).get(memberName);
    if (!member || !ts.isPropertyAssignment(member)) continue;
    if (!ts.isObjectLiteralExpression(member.initializer)) continue;
    for (const property of member.initializer.properties) {
      const name = propertyName(property);
      if (name === undefined) continue;
      spans.push({ name, kind, node: property.name ?? property });
    }
  }
  return spans;
}
