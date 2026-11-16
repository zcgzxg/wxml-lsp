/**
 * Candidate A: in-place shadow.
 *
 * The original script stays the type environment. A probe member is inserted
 * *inside* the registration's options object (or inside its `methods` object for
 * components) so fragments are checked against `this.data` while the options
 * literal remains the direct argument of `Page`/`Component`: contextual typing,
 * generic inference and `this` typing stay intact.
 */

import ts from "typescript";
import { memberMap, type Registration } from "../ast.ts";
import { planFragments, PROPS_PREFIX, ROOT_PREFIX } from "../fragments.ts";
import { indentLines, localHelpers, renderFragmentScope } from "./locals.ts";
import { buildSegments } from "./segments.ts";
import { rewriteExpression } from "../rewrite.ts";
import { PROPS_TYPE_NAME, PROPS_VALUE_NAME, renderPropsType, renderPropsValue, type RenderedMember } from "../propsOverlay.ts";
import { assemble, type Piece } from "../text.ts";
import type {
  CapabilityStatus,
  FragmentRecord,
  GeneratedDeclaration,
  LayoutBuild,
} from "../types.ts";
import type { LayoutContext } from "./context.ts";

export const IN_PLACE_PROBE = "__wxml_check_0";
export const ROOT_NAME = "__wxml_root";

const METHOD_INDENT = "      ";
const BODY_INDENT = "        ";

class Pieces {
  private readonly items: Piece[] = [];
  length = 0;

  text(text: string): void {
    this.append([{ kind: "text", text }]);
  }

  /** Append pieces and keep the running offset in sync (locals included). */
  append(pieces: Piece[]): void {
    for (const piece of pieces) {
      this.items.push(piece);
      this.length += piece.text.length;
    }
  }

  pieces(): Piece[] {
    return [...this.items];
  }
}

export function buildInPlaceShadow(context: LayoutContext): LayoutBuild {
  const registration = context.registration;
  if (!registration) {
    const detail = context.scan.unsupportedCalls.length > 0 ? `; saw ${context.scan.unsupportedCalls.join(", ")}` : "";
    return emptyBuild(context, "unsupported", `no ${context.rootKind} registration with an object literal argument${detail}`);
  }

  const notes: string[] = [];
  let buildStatus: CapabilityStatus = "supported";
  let reason: string | undefined;
  if (context.registrationCount > 1) {
    buildStatus = "partial";
    reason = `${context.registrationCount} ${context.rootKind} registrations; the probe mounts the first one`;
    notes.push("multi-registration: candidate selection is not frozen, results are reported as partial");
  }

  if (context.dataRootKind === "type-reference") {
    return emptyBuild(
      context,
      "unsupported",
      "data root is a type-only reference (interface/type alias, possibly imported); no runtime data object exists",
      notes,
    );
  }
  if (context.dataRootKind === "absent") {
    buildStatus = "partial";
    reason ??= "registration has no `data` member; the template root is empty";
  }
  if (context.dataRootKind === "unsupported") {
    return emptyBuild(
      context,
      "unsupported",
      "`data` member exists but is not a property assignment (shorthand/method); the template root cannot be bound",
      notes,
    );
  }
  if (!context.propertyScan.complete) {
    buildStatus = "partial";
    reason ??= `component properties are incomplete: ${context.propertyScan.problems.join("; ")}`;
    notes.push("optionalTypes coverage may be under-reported because `properties` is not a plain literal");
  }

  const overlayProperties = context.propertyScan.properties;
  const overlayNames = new Set(overlayProperties.map((property) => property.name));
  const drafts = planFragments(context.fragments, {
    propertyOverlay: overlayNames,
    rootPrefix: ROOT_PREFIX,
  });

  const syntax = context.isJavaScript ? "js" : "ts";
  const body = new Pieces();
  const marks: Array<{ member: RenderedMember; renderedStart: number }> = [];
  body.text(`\n    ${IN_PLACE_PROBE}() {\n`);
  body.text(`${BODY_INDENT}const ${ROOT_NAME} = this.data;\n`);
  const rootExpressionOffset = body.length - "this.data;\n".length;
  if (overlayProperties.length > 0) {
    if (!context.isJavaScript) {
      const rendered = renderPropsType(overlayProperties, BODY_INDENT);
      for (const member of rendered.members) marks.push({ member, renderedStart: body.length + member.offset });
      body.text(`${rendered.text}\n`);
    }
    const value = renderPropsValue(overlayProperties, BODY_INDENT, syntax);
    for (const member of value.members) marks.push({ member, renderedStart: body.length + member.offset });
    body.text(`${value.text}\n`);
  }
  // The helper types are only needed when a `wx:for` source has to be typed; they
  // are declared once per probe, after the root, because they depend on nothing.
  const typedBindings = drafts.some((draft) => draft.bindings.length > 0);
  const helpers = localHelpers(syntax);
  if (typedBindings && helpers.length > 0) body.text(`${indentLines(helpers, BODY_INDENT)}\n`);

  let unresolvedBindings = 0;
  let unresolvedGuards = 0;
  for (const draft of drafts) {
    const scope = renderFragmentScope(draft, {
      indent: BODY_INDENT,
      syntax,
      rewriteSource: (sourceExpr, outerNames) => rewriteInScope(overlayNames, sourceExpr, outerNames),
      rewriteCondition: (exprText) =>
        rewriteInScope(overlayNames, exprText, new Set(draft.bindings.length > 0 ? draft.bindings.map((b) => b.name) : draft.locals)),
    });
    unresolvedBindings += scope.unresolvedBindings;
    unresolvedGuards += scope.unresolvedGuards;
    body.text(scope.open);
    body.text(scope.mountIndent);
    const mountId = `${draft.fragmentId}#0`;
    body.append([
      {
        kind: "fragment",
        fragmentId: draft.fragmentId,
        mountId,
        text: draft.mountedText,
        target: draft.target,
        targetOffset: draft.targetOffset,
        hoverTargets: draft.hoverTargets,
      },
    ]);
    body.text(`;\n`);
    body.text(scope.close);
  }
  body.text(`${METHOD_INDENT}},\n  `);

  const placement = probePlacement(registration);
  if ("problem" in placement) {
    return emptyBuild(context, "partial", placement.problem, notes);
  }
  const methodPieces = body.pieces();
  const probePieces: Piece[] =
    placement.wrapInMethods === true
      ? [
          { kind: "text", text: `\n    methods: {\n  ` },
          ...methodPieces,
          { kind: "text", text: `    },\n  ` },
        ]
      : methodPieces;
  const methodWrapperShift = placement.wrapInMethods === true ? `\n    methods: {\n  `.length : 0;

  const pragmaPieces: Array<{
    id: string;
    kind: "pragma" | "edit";
    at: number;
    pieces: Piece[];
    replaceLength?: number;
  }> = [];
  const jsCheckApplied = effectiveJsCheck(context, notes);
  // A BOM before a hashbang makes TypeScript mis-parse the shebang, so any
  // pragma (injected or from the source) would fall outside the leading block.
  const mustDropBom =
    context.pragma.bomLength > 0 &&
    (jsCheckApplied === "pragma" || context.pragma.check || context.pragma.noCheck);
  if (mustDropBom) {
    pragmaPieces.push({ id: "bom", kind: "edit", at: 0, pieces: [], replaceLength: 1 });
    notes.push("BOM dropped in the derived view so pragmas are recognised after the shebang");
  }
  if (jsCheckApplied === "pragma") {
    pragmaPieces.push({
      id: "pragma",
      kind: "pragma",
      at: context.pragma.injectionOffset,
      pieces: [{ kind: "text", text: "// @ts-check\n" }],
    });
    notes.push("shadow-only `// @ts-check` enables JS checking without changing project options");
  }
  if (jsCheckApplied === "off" && context.pragma.check) {
    // TypeScript honours the last pragma in the leading comment block, so the
    // neutralising pragma must go after the source pragmas.
    pragmaPieces.push({
      id: "nocheck",
      kind: "pragma",
      at: context.pragma.neutralizeOffset,
      pieces: [{ kind: "text", text: "// @ts-nocheck\n" }],
    });
    notes.push("source `@ts-check` neutralised with a shadow-only `@ts-nocheck` (policy off)");
  }

  const assembled = assemble(
    context.scriptText,
    [
      ...pragmaPieces.map((entry) => ({
        id: entry.id,
        kind: entry.kind,
        at: entry.at,
        pieces: entry.pieces,
        replaceLength: entry.replaceLength,
      })),
      { id: "probe", kind: "probe" as const, at: placement.at, pieces: probePieces },
    ],
    context.scriptPath,
  );

  const probeInsertion = assembled.insertions.find((insertion) => insertion.insertionId === "probe");
  if (!probeInsertion) throw new Error("layout A: probe insertion missing");

  const fragments: FragmentRecord[] = drafts.map((draft) => ({
    fragmentId: draft.fragmentId,
    expr: draft.expr,
    root: draft.root,
    target: draft.target,
    locals: draft.locals,
    bindings: draft.bindings,
    guards: draft.guards,
    localRoot: draft.localRoot,
    mounts: assembled.mounts
      .filter((mount) => mount.fragmentId === draft.fragmentId)
      .map((mount) => ({
        mountId: mount.mountId,
        fragmentId: mount.fragmentId,
        insertionId: mount.insertionId,
        file: context.scriptPath,
        text: mount.text,
        startUtf16: mount.startUtf16,
        targetStartUtf16: mount.targetStartUtf16,
        targetLengthUtf16: mount.targetLengthUtf16,
        target: draft.target,
        hoverTargets: mount.hoverTargets,
        segments: buildSegments(mount.startUtf16, draft),
      })),
  }));

  const generatedDeclarations: GeneratedDeclaration[] = marks.map((mark) => ({
    file: context.scriptPath,
    start: probeInsertion.startUtf16 + methodWrapperShift + mark.renderedStart,
    length: mark.member.length,
    source: byteRangeOfNode(context.scriptText, context.scriptPath, propertyNameNode(context, mark.member.name)),
  }));

  // Offsets are relative to the whole insertion (the wrapper included).
  const helperOffsets = [
    { name: IN_PLACE_PROBE, offset: probeInsertion.startUtf16 + bodyOffsetOf(probePieces, IN_PLACE_PROBE) },
    { name: ROOT_NAME, offset: probeInsertion.startUtf16 + bodyOffsetOf(probePieces, ROOT_NAME) },
    ...(overlayProperties.length > 0
      ? [{ name: PROPS_VALUE_NAME, offset: probeInsertion.startUtf16 + bodyOffsetOf(probePieces, PROPS_VALUE_NAME) }]
      : []),
  ];

  if (unresolvedBindings > 0 || unresolvedGuards > 0) {
    buildStatus = "partial";
    const unresolvedReason = "some template bindings or guards could not be resolved and were not checked";
    reason = reason ? `${reason}; ${unresolvedReason}` : unresolvedReason;
    notes.push("unresolved `wx:for` bindings or `wx:if` guards are mounted as `any`/unnarrowed and not checked");
  }

  return {
    layout: "in-place-shadow",
    buildStatus,
    ...(reason ? { reason } : {}),
    unresolvedBindings,
    unresolvedGuards,
    files: [{ path: context.scriptPath, text: assembled.text }],
    queryFile: context.scriptPath,
    insertions: assembled.insertions,
    fragments,
    generatedDeclarations,
    jsCheckApplied,
    notes,
    edits: assembled.edits,
    probeAnchors: {
      file: context.scriptPath,
      rootOffset: probeInsertion.startUtf16 + methodWrapperShift + rootExpressionOffset + "this.".length,
      helperOffsets,
    },
    moduleAfter: moduleKindOf(context.scriptPath, assembled.text),
  };
}

/**
 * Effective checking policy: `off > @ts-nocheck > source @ts-check > pragma`.
 *
 * A source `@ts-check` only takes effect for `inherit`/`pragma`; an explicit
 * `off` wins and is enforced by neutralising the source pragma (the layout adds
 * a later `@ts-nocheck`).
 */
export function effectiveJsCheck(
  context: Pick<LayoutContext, "isJavaScript" | "jsCheckPolicy" | "checkJs" | "pragma" | "wantPragma">,
  notes: string[],
): LayoutBuild["jsCheckApplied"] {
  if (context.jsCheckPolicy === "off") return "off";
  if (context.pragma.noCheck) {
    notes.push("`@ts-nocheck` in the source disables template diagnostics");
    return "off";
  }
  if (!context.isJavaScript) return "inherit";
  if (context.checkJs) return "inherit";
  if (context.pragma.check) return "inherit";
  if (context.wantPragma) return "pragma";
  return "off";
}

function propertyNameNode(context: LayoutContext, name: string): ts.Node | undefined {
  const declaration = context.propertyScan.properties.find((property) => property.name === name);
  return declaration?.nameNode;
}

function byteRangeOfNode(
  text: string,
  file: string,
  node: ts.Node | undefined,
): GeneratedDeclaration["source"] {
  if (!node) return undefined;
  const start = Buffer.byteLength(text.slice(0, node.getStart()), "utf8");
  const end = Buffer.byteLength(text.slice(0, node.getEnd()), "utf8");
  return { file, start, end };
}

/** Rewrite a template sub-expression against `this.data` (and the props overlay). */
function rewriteInScope(
  overlay: ReadonlySet<string>,
  exprText: string,
  locals: ReadonlySet<string>,
): string | undefined {
  try {
    return rewriteExpression(exprText, undefined, {
      rootPrefix: ROOT_PREFIX,
      propsPrefix: PROPS_PREFIX,
      propertyOverlay: overlay,
      locals,
    }).text;
  } catch {
    return undefined;
  }
}

/** Offset of the first piece that starts with `needle` (no text search). */
function bodyOffsetOf(pieces: Piece[], needle: string): number {
  let offset = 0;
  for (const piece of pieces) {
    if (piece.kind === "text" && piece.text.includes(needle)) return offset + piece.text.indexOf(needle);
    offset += piece.text.length;
  }
  return 0;
}

interface ProbePlacement {
  at: number;
  wrapInMethods?: boolean;
}

function probePlacement(registration: Registration): ProbePlacement | { problem: string } {
  if (registration.kind === "page") return { at: registration.options.getStart() + 1 };
  const methods = memberMap(registration.options).get("methods");
  if (methods) {
    if (!ts.isPropertyAssignment(methods) || !ts.isObjectLiteralExpression(methods.initializer)) {
      return { problem: "component `methods` is not an object literal; the in-place probe cannot be mounted" };
    }
    return { at: methods.initializer.getStart() + 1 };
  }
  return { at: registration.options.getStart() + 1, wrapInMethods: true };
}

function emptyBuild(
  context: LayoutContext,
  status: CapabilityStatus,
  reason: string,
  notes: string[] = [],
): LayoutBuild {
  return {
    layout: "in-place-shadow",
    buildStatus: status,
    reason,
    files: [{ path: context.scriptPath, text: context.scriptText }],
    queryFile: context.scriptPath,
    insertions: [],
    fragments: [],
    generatedDeclarations: [],
    jsCheckApplied: context.jsCheckPolicy === "off" ? "off" : "inherit",
    notes,
    edits: [],
    moduleAfter: moduleKindOf(context.scriptPath, context.scriptText),
  };
}

/** Module classification of a text snapshot, using TypeScript's own indicator. */
export function moduleKindOf(fileName: string, text: string): "script" | "module" {
  const sourceFile = ts.createSourceFile(fileName, text, ts.ScriptTarget.Latest, true);
  const candidate = sourceFile as ts.SourceFile & { externalModuleIndicator?: ts.Node };
  return candidate.externalModuleIndicator === undefined ? "script" : "module";
}

export { PROPS_TYPE_NAME };
