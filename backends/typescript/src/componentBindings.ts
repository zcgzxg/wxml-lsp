/**
 * Custom-component property binding diagnostics (AC-012).
 *
 * The template analysis sends the component usages it found (`components` in the
 * request). Node reads the component's declared `properties` from the same
 * in-memory snapshot, derives the allowed instance type of every property and
 * checks each bound attribute with `checker.isTypeAssignableTo` (or a structural
 * predicate for `Array`/`Object`).
 *
 * Everything that genuinely cannot be resolved — a component that is not part of
 * the snapshot, a non-literal/incomplete `properties` member, an attribute whose
 * fragment was not mounted, a bound expression that is `any`/`unknown`, a
 * `Function` constructor — is counted as `skipped` with a `tag.attribute` pair so
 * the caller can report Partial coverage instead of a clean check. This module
 * never throws on malformed component data and never puts a file path into a
 * diagnostic message.
 */

import path from "node:path";
import ts from "typescript";
import { findRegistrations, memberMap, scanDeclaredProperties } from "./ast.ts";
import { resolveBehaviors } from "./behaviors.ts";
import type { VirtualProject } from "./compiler.ts";
import { sanitizeMessage } from "./messages.ts";
import type {
  AnalyzeRequest,
  RequestComponentAttribute,
  RequestComponentUsage,
} from "./protocol.ts";
import type { LayoutBuild } from "./types.ts";

/** Upper bounds that keep a hostile template from flooding the response. */
const MAX_TAGS = 64;
const MAX_DIAGNOSTICS_PER_TAG = 8;
const MAX_EXPRESSION_TEXT = 80;
/** Example pairs quoted in the Partial reason. */
const MAX_SKIPPED_DETAILS = 3;
/** Hard cap after sanitization (which already trims to 200 chars). */
const MAX_MESSAGE_LENGTH = 512;

export interface ComponentDiagnostic {
  range: { start: number; end: number };
  severity: "error";
  message: string;
}

export interface ComponentResult {
  tag: string;
  diagnostics: ComponentDiagnostic[];
}

export interface ComponentDiagnosticsInput {
  request: AnalyzeRequest;
  build: LayoutBuild;
  project: VirtualProject;
}

export interface ComponentDiagnostics {
  results: ComponentResult[];
  /** Bindings/usages that were not checked (Partial coverage). */
  skipped: number;
  /** Up to three `tag.attribute` (or `tag`) examples of skipped bindings. */
  skippedDetails: string[];
}

/**
 * What an attribute value may be assigned to.
 *
 * `Array`/`Object` declarations are structural (`any[]` / `Record<string, any>`
 * hover text), so they are checked with predicates instead of a synthesized
 * `ts.Type`.
 */
type AllowedTarget =
  | { kind: "assignable"; type: ts.Type }
  | { kind: "array" }
  | { kind: "object" };

/** Primitive constructor name declared in `properties`. */
type PrimitiveKind = "string" | "number" | "boolean" | "array" | "object" | "any";

const PRIMITIVES: Record<string, PrimitiveKind> = {
  String: "string",
  Number: "number",
  Boolean: "boolean",
  Array: "array",
  Object: "object",
  // `Function` (and every unknown constructor) cannot be checked honestly.
  Function: "any",
};

/** One declared property: display text plus every allowed target. */
interface DeclaredProperty {
  text: string;
  /** `null` means "any": the binding is not checked (counted as skipped). */
  targets: AllowedTarget[] | null;
}

/**
 * Collect diagnostics for every custom-component usage in the request.
 *
 * A result entry is returned for each usage that had at least one checkable
 * attribute (possibly with an empty diagnostic list). Usages that could not be
 * resolved at all are only counted in `skipped`.
 */
export function collectComponentDiagnostics(input: ComponentDiagnosticsInput): ComponentDiagnostics {
  const usages = input.request.components ?? [];
  if (usages.length === 0) return { results: [], skipped: 0, skippedDetails: [] };

  const fileIds = new Set(input.request.files.map((file) => file.fileId));
  const checker = input.project.checker();
  const results: ComponentResult[] = [];
  const skippedDetails: string[] = [];
  const tags = new Set<string>();
  let skipped = 0;

  const remember = (detail: string): void => {
    if (skippedDetails.length < MAX_SKIPPED_DETAILS && !skippedDetails.includes(detail)) {
      skippedDetails.push(detail);
    }
  };

  for (const usage of usages) {
    // The limit bounds distinct tags, not usages: a page may repeat one tag.
    if (!tags.has(usage.tag) && tags.size >= MAX_TAGS) {
      skipped += 1;
      remember(usage.tag);
      continue;
    }
    tags.add(usage.tag);
    try {
      const outcome = checkUsage(usage, input, fileIds, checker);
      skipped += outcome.skipped;
      for (const detail of outcome.skippedDetails) remember(detail);
      if (outcome.result) results.push(outcome.result);
    } catch {
      // A malformed component must degrade to Partial coverage, never fail the
      // whole analysis (component data is template input, not trusted code).
      skipped += 1;
      remember(usage.tag);
    }
  }
  return { results, skipped, skippedDetails };
}

interface UsageOutcome {
  result?: ComponentResult;
  skipped: number;
  skippedDetails: string[];
}

function checkUsage(
  usage: RequestComponentUsage,
  input: ComponentDiagnosticsInput,
  fileIds: ReadonlySet<string>,
  checker: ts.TypeChecker,
): UsageOutcome {
  const skippedDetail = (attribute?: string): string =>
    attribute === undefined ? usage.tag : `${usage.tag}.${attribute}`;
  if (!usage.componentFileId || !fileIds.has(usage.componentFileId)) {
    return { skipped: 1, skippedDetails: [usage.tag] };
  }
  const sourceFile = componentSource(input, usage.componentFileId);
  if (!sourceFile) return { skipped: 1, skippedDetails: [usage.tag] };
  const declaration = declaredProperties(sourceFile, checker, input.project);
  if (!declaration) return { skipped: 1, skippedDetails: [usage.tag] };
  const properties = declaration.properties;

  const stringType = checker.getStringType();
  const booleanType = checker.getBooleanType();
  const diagnostics: ComponentDiagnostic[] = [];
  const skippedDetails: string[] = [];
  let skipped = 0;
  let checked = 0;

  const skip = (attributeName: string): void => {
    skipped += 1;
    skippedDetails.push(skippedDetail(attributeName));
  };

  for (const attribute of usage.attributes) {
    const declared =
      properties.get(attribute.name) ??
      properties.get(camelCase(attribute.name)) ??
      properties.get(kebabCase(attribute.name));
    if (!declared) continue; // `class`, `style` and undeclared attributes are not properties
    if (declared.targets === null) {
      skip(attribute.name); // property is `any`/an unreadable constructor
      continue;
    }
    const accepts = (actual: ts.Type): boolean => {
      // An optional data member yields `T | undefined`; `undefined`/`null` mean the
      // attribute is absent at runtime, which WeChat accepts. Every remaining
      // constituent must satisfy one of the declared targets.
      const constituents = definedConstituents(actual);
      if (constituents.length === 0) return true;
      return constituents.every((part) =>
        declared.targets!.some((target) => acceptsType(target, part, checker)),
      );
    };
    if (attribute.valueKind === "mixed") {
      skip(attribute.name); // `title="a{{ b }}"` needs a string conversion; out of scope
      continue;
    }
    if (attribute.valueKind === "literal") {
      checked += 1;
      if (!accepts(stringType)) {
        push(diagnostics, {
          range: attribute.attributeRange,
          severity: "error",
          message: componentMessage(
            `component property "${attribute.name}" expects ${declared.text}, got string literal`,
          ),
        });
      }
      continue;
    }
    if (attribute.valueKind === "boolean") {
      checked += 1;
      if (!accepts(booleanType)) {
        push(diagnostics, {
          range: attribute.attributeRange,
          severity: "error",
          message: componentMessage(
            `component property "${attribute.name}" expects ${declared.text}, got boolean literal`,
          ),
        });
      }
      continue;
    }
    const bound = boundType(input, attribute, checker);
    if (!bound) {
      skip(attribute.name); // no mounted fragment or the expression is any/unknown
      continue;
    }
    checked += 1;
    if (accepts(bound.type)) continue;
    const actual = checker.typeToString(bound.type);
    push(diagnostics, {
      range: attribute.exprRange!,
      severity: "error",
      message: componentMessage(
        `component property "${attribute.name}" expects ${declared.text}, got ${actual}` +
          ` (bound expression "${truncate(bound.exprText)}")`,
      ),
    });
  }

  if (declaration.incomplete) {
    skipped += 1;
    skippedDetails.push(`${usage.tag}.behavior`);
  }
  if (checked === 0 && diagnostics.length === 0) return { skipped, skippedDetails };
  return { result: { tag: usage.tag, diagnostics }, skipped, skippedDetails };
}

/** Structural or assignability check for one allowed target. */
function acceptsType(target: AllowedTarget, actual: ts.Type, checker: ts.TypeChecker): boolean {
  switch (target.kind) {
    case "array":
      return checker.isArrayLikeType(actual);
    case "object":
      // `TypeChecker.isObjectType` is not public in this TypeScript build; the
      // equivalent is the Object type flag (arrays carry it too, as intended).
      return (actual.flags & ts.TypeFlags.Object) !== 0;
    case "assignable":
      return checker.isTypeAssignableTo(actual, target.type);
  }
}

/**
 * Non-nullish constituents of a bound type.
 *
 * `checker.getUnionType` is not public, so a union is split into `type.types` and
 * filtered: `undefined`/`null` constituents mean "the attribute is not set".
 */
function definedConstituents(type: ts.Type): ts.Type[] {
  const parts = type.isUnion() ? type.types : [type];
  return parts.filter((part) => (part.flags & (ts.TypeFlags.Undefined | ts.TypeFlags.Null)) === 0);
}

/** The component script as TypeScript sees it (same snapshot, no disk read). */
function componentSource(input: ComponentDiagnosticsInput, fileId: string): ts.SourceFile | undefined {
  const absolute = path.resolve(input.request.project.projectRoot, fileId);
  return input.project.program().getSourceFile(absolute) ?? undefined;
}

/**
 * Declared `properties` of the component, or `undefined` when the component
 * declaration itself cannot be read (no registration or a spread/invalid own
 * `properties` object). An unresolved behavior only makes the returned
 * `incomplete` flag true: the component's own properties are still checked,
 * while attributes that may belong to the missing behavior coverage are
 * counted as skipped.
 *
 * The property *set* comes from `scanDeclaredProperties`, the same helper the
 * declaration overlay uses, plus statically resolved behavior properties. Each
 * declaration carries its option initializer so the allowed `ts.Type`s can be
 * derived from the real constructor instead of parsing `typeText`.
 */
function declaredProperties(
  sourceFile: ts.SourceFile,
  checker: ts.TypeChecker,
  project: VirtualProject,
): { properties: Map<string, DeclaredProperty>; incomplete: boolean } | undefined {
  const registration = findRegistrations(sourceFile).registrations.find((entry) => entry.kind === "component");
  if (!registration) return undefined;
  const scan = scanDeclaredProperties(registration.options);
  if (!scan.complete) return undefined;
  // Behavior properties are checked from the same snapshot. A dynamic or
  // incomplete behavior graph is not narrowed: the usage is skipped.
  const behavior = resolveBehaviors({
    scriptPath: sourceFile.fileName,
    sourceFile,
    registration,
    getSourceFile: (absolute) => project.program().getSourceFile(path.resolve(absolute)),
    checker,
  });
  const hasOwnProperties = memberMap(registration.options).has("properties");
  if (!hasOwnProperties && behavior.properties.length === 0) return undefined;
  const properties = new Map<string, DeclaredProperty>();
  for (const declared of [...scan.properties, ...behavior.properties]) {
    if (properties.has(declared.name)) continue; // the component's own declaration wins
    properties.set(
      declared.name,
      allowedTypes(declared.initializer, declared.typeText, checker),
    );
  }
  return {
    properties,
    // The component's own declarations are always checked. Only missing
    // behavior-derived declarations are unknown; undeclared attributes are
    // ignored by the checker, so this is a coverage gap, not a fabricated
    // unknown-property diagnostic.
    incomplete: behavior.propertiesIncomplete || behavior.problems.length > 0,
  };
}

/** `card-title` -> `cardTitle` (WeChat attribute/property mapping). */
function camelCase(name: string): string {
  return name.replace(/-([a-z])/g, (_, letter: string) => letter.toUpperCase());
}

/** `cardTitle` -> `card-title`, for a property declared in kebab case. */
function kebabCase(name: string): string {
  return name.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`);
}

/**
 * Allowed instance types of one property option literal.
 *
 * `String`/`Number`/`Boolean` map to the checker's primitive types;
 * `Array`/`Object` become structural predicates; `Function`, `null` and any
 * unknown constructor mean "any", so the binding is skipped rather than checked
 * against a guessed type.
 */
function allowedTypes(
  initializer: ts.Expression | undefined,
  typeText: string,
  checker: ts.TypeChecker,
): DeclaredProperty {
  if (!initializer || !ts.isObjectLiteralExpression(initializer)) return { text: "any", targets: null };
  const option = memberMap(initializer);
  const targets: AllowedTarget[] = [];
  const kinds: PrimitiveKind[] = [];
  let any = false;

  const add = (kind: PrimitiveKind | undefined): void => {
    if (kind === undefined || kind === "any") {
      any = true;
      return;
    }
    if (!kinds.includes(kind)) {
      kinds.push(kind);
      targets.push(allowedTarget(kind, checker));
    }
  };

  const typeMember = option.get("type");
  if (typeMember && ts.isPropertyAssignment(typeMember)) {
    const identifier = typeMember.initializer;
    add(ts.isIdentifier(identifier) ? PRIMITIVES[identifier.text] : undefined);
  } else {
    any = true; // missing/unreadable `type` allows any value
  }

  const optionalMember = option.get("optionalTypes");
  if (optionalMember !== undefined) {
    if (ts.isPropertyAssignment(optionalMember) && ts.isArrayLiteralExpression(optionalMember.initializer)) {
      for (const element of optionalMember.initializer.elements) {
        add(ts.isIdentifier(element) ? PRIMITIVES[element.text] : undefined);
      }
    } else {
      any = true;
    }
  }

  if (any || targets.length === 0) return { text: "any", targets: null };
  return { text: typeText || kinds.join(" | "), targets };
}

function allowedTarget(kind: Exclude<PrimitiveKind, "any">, checker: ts.TypeChecker): AllowedTarget {
  switch (kind) {
    case "string":
      return { kind: "assignable", type: checker.getStringType() };
    case "number":
      return { kind: "assignable", type: checker.getNumberType() };
    case "boolean":
      return { kind: "assignable", type: checker.getBooleanType() };
    case "array":
      return { kind: "array" };
    case "object":
      return { kind: "object" };
  }
}

/**
 * Type of the mounted mustache expression for an attribute binding.
 *
 * The fragment is identified by its `wxmlRange` (the attribute's `exprRange`),
 * so a repeated expression is matched to the exact occurrence that the analysis
 * saw. `undefined` means skip: no fragment, no mount, or an `any`/`unknown`
 * result.
 */
function boundType(
  input: ComponentDiagnosticsInput,
  attribute: RequestComponentAttribute,
  checker: ts.TypeChecker,
): { type: ts.Type; exprText: string } | undefined {
  const range = attribute.exprRange;
  const exprText = attribute.exprText;
  if (!range || exprText === undefined) return undefined;
  const requestFragment = input.request.fragments.find(
    (fragment) => fragment.wxmlRange.start === range.start && fragment.wxmlRange.end === range.end,
  );
  if (!requestFragment) return undefined;
  const record = input.build.fragments.find((fragment) => fragment.fragmentId === requestFragment.fragmentId);
  const mount = record?.mounts[0];
  if (!mount) return undefined;
  const sourceFile = input.project.program().getSourceFile(path.resolve(mount.file));
  if (!sourceFile) return undefined;
  const node =
    expressionAtStart(sourceFile, mount.startUtf16) ?? expressionAtStart(sourceFile, mount.targetStartUtf16);
  if (!node) return undefined;
  const type = checker.getTypeAtLocation(node);
  if ((type.flags & (ts.TypeFlags.Any | ts.TypeFlags.Unknown)) !== 0) return undefined;
  return { type, exprText };
}

/** Outermost expression that starts exactly at `offset`. */
function expressionAtStart(sourceFile: ts.SourceFile, offset: number): ts.Expression | undefined {
  let best: ts.Expression | undefined;
  const visit = (node: ts.Node): void => {
    const start = node.getStart(sourceFile);
    if (offset < start || offset >= node.getEnd()) return;
    if (start === offset && ts.isExpression(node) && (!best || node.getEnd() > best.getEnd())) best = node;
    node.forEachChild(visit);
  };
  visit(sourceFile);
  return best;
}

function push(diagnostics: ComponentDiagnostic[], diagnostic: ComponentDiagnostic): void {
  if (diagnostics.length >= MAX_DIAGNOSTICS_PER_TAG) return;
  diagnostics.push(diagnostic);
}

/**
 * User-visible component message: bridge helper names are sanitized away and the
 * result is bounded. Component declarations are user code, so the message must
 * not expose a path or an internal name.
 */
function componentMessage(raw: string): string {
  const sanitized = sanitizeMessage(raw);
  return sanitized.length <= MAX_MESSAGE_LENGTH ? sanitized : `${sanitized.slice(0, MAX_MESSAGE_LENGTH - 1)}…`;
}

/** Bounded expression text; the message must stay small and path-free. */
function truncate(text: string): string {
  return text.length <= MAX_EXPRESSION_TEXT ? text : `${text.slice(0, MAX_EXPRESSION_TEXT)}…`;
}
