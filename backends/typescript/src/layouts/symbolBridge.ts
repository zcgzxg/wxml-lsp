/**
 * Candidate B: symbol bridge plus a separate check file.
 *
 * The options object is bound to a variable at the registration site (a minimal
 * insertion), the data root is exported from that derived view, and a separate
 * generated `.ts` file mounts the WXML fragments against the exported anchor.
 *
 * Measured consequences (see docs/evidence/type-bridge-c1.md):
 * - the insertion changes contextual `this` typing of the user script;
 * - a registration inside a function cannot be anchored at module top level;
 * - for a *module* script the declaration-derived properties type lives in the
 *   check file's module scope and is invisible to the anchor expression, so the
 *   root degrades to `any` and the user script gains a 2304.
 */

import path from "node:path";
import ts from "typescript";
import { planFragments, PROPS_PREFIX } from "../fragments.ts";
import { indentLines, localHelpers, renderFragmentScope } from "./locals.ts";
import { buildSegments } from "./segments.ts";
import { rewriteExpression } from "../rewrite.ts";
import { PROPS_TYPE_NAME, renderPropsType, renderPropsValue } from "../propsOverlay.ts";
import { assemble, type Piece } from "../text.ts";
import type { CapabilityStatus, FragmentRecord, GeneratedDeclaration, LayoutBuild } from "../types.ts";
import type { LayoutContext } from "./context.ts";

const OPTIONS_VAR = "__wxml_opts_0";
const ROOT_VAR = "__wxml_root_0";
const CHECK_FUNCTION = "__wxml_check_0";
export const CHECK_FILE_NAME = "__wxml_view_0.ts";

export function buildSymbolBridge(context: LayoutContext): LayoutBuild {
  const registration = context.registration;
  if (!registration) {
    return emptyBuild(context, "unsupported", `no ${context.rootKind} registration with an object literal argument`);
  }

  const notes: string[] = [];
  let buildStatus: CapabilityStatus = "supported";
  let reason: string | undefined;
  if (context.registrationCount > 1) {
    buildStatus = "partial";
    reason = `${context.registrationCount} ${context.rootKind} registrations; the bridge mounts the first one`;
  }
  if (context.dataRootKind === "type-reference") {
    return emptyBuild(
      context,
      "unsupported",
      "data root is a type-only reference (interface/type alias, possibly imported); no runtime data object exists",
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
    );
  }
  if (!context.propertyScan.complete) {
    buildStatus = "partial";
    reason ??= `component properties are incomplete: ${context.propertyScan.problems.join("; ")}`;
  }
  if (!registration.topLevel) {
    return emptyBuild(
      context,
      "unsupported",
      "registration runs inside a function: the options object cannot be bound and exported at module top level",
    );
  }

  const moduleBefore = moduleKindText(context.scriptPath, context.scriptText);
  const anchorExported = moduleBefore === "module";
  const statement = statementOf(registration.call);
  const hasProperties = context.propertyScan.properties.length > 0;
  if (hasProperties && anchorExported) {
    buildStatus = "partial";
    reason ??=
      "module script: the declaration-derived properties type lives in the check file module scope and is not visible to the anchor expression";
    notes.push("anchor cannot see __WxmlProps_0 for module scripts; root degrades to any and the script gains 2304");
  }

  const scriptInsertions = [
    { id: "opts-decl", kind: "capture" as const, at: statement.getStart(context.sourceFile), pieces: [{ kind: "text" as const, text: `let ${OPTIONS_VAR};\n` }] },
    { id: "capture", kind: "capture" as const, at: optionsStart(registration.call), pieces: [{ kind: "text" as const, text: `${OPTIONS_VAR} = ` }] },
    {
      id: "anchor",
      kind: "anchor" as const,
      at: context.scriptText.length,
      pieces: [
        {
          kind: "text" as const,
          text:
            hasProperties
              ? `\nconst ${ROOT_VAR} = ${OPTIONS_VAR}.data as typeof ${OPTIONS_VAR}.data & ${PROPS_TYPE_NAME};\n${anchorExported ? `export { ${ROOT_VAR} };\n` : ""}`
              : `\nconst ${ROOT_VAR} = ${OPTIONS_VAR}.data;\n${anchorExported ? `export { ${ROOT_VAR} };\n` : ""}`,
        },
      ],
    },
  ];

  const assembledScript = assemble(context.scriptText, scriptInsertions, context.scriptPath);
  if (anchorExported) notes.push("module script: anchor is exported and imported by the check file");
  else notes.push(`script file: anchor is a global const (no module flip, global name ${ROOT_VAR})`);

  const checkPath = path.join(path.dirname(context.scriptPath), CHECK_FILE_NAME);
  const drafts = planFragments(context.fragments, {
    propertyOverlay: new Set(),
    dataOverlay: new Set(),
    rootPrefix: `${ROOT_VAR}.`,
  });

  const pieces: Piece[] = [];
  let prefixText = "// generated in-memory type-bridge view (never written to disk)\n";
  if (anchorExported) {
    prefixText += `import { ${ROOT_VAR} } from "./${moduleSpecifier(context.scriptPath)}";\n`;
  }
  pieces.push({ kind: "text", text: prefixText });
  const marks: Array<{ name: string; offsetInText: number; length: number }> = [];
  if (hasProperties) {
    const type = renderPropsType(context.propertyScan.properties, "");
    for (const member of type.members) {
      marks.push({ name: member.name, offsetInText: prefixText.length + member.offset, length: member.length });
    }
    const typeBlock = `${type.text}\n`;
    const value = renderPropsValue(context.propertyScan.properties, "", "ts");
    for (const member of value.members) {
      marks.push({
        name: member.name,
        offsetInText: prefixText.length + typeBlock.length + member.offset,
        length: member.length,
      });
    }
    pieces.push({ kind: "text", text: `${typeBlock}${value.text}\n` });
  }
  pieces.push({ kind: "text", text: `\nfunction ${CHECK_FUNCTION}() {\n` });
  // The check file is always TypeScript, so the typed-local helpers can be used
  // even for a JavaScript project (the anchor itself degrades to `any` there).
  const typedBindings = drafts.some((draft) => draft.bindings.length > 0);
  const helpers = localHelpers("ts");
  if (typedBindings && helpers.length > 0) pieces.push({ kind: "text", text: `${indentLines(helpers, "  ")}\n` });

  // Locals without a structured binding stay `any`: the compatibility `locals`
  // list carries names only, so there is no source expression to derive a type
  // from. `renderFragmentScope` documents that as an unresolved binding.
  let unresolvedBindings = 0;
  let unresolvedGuards = 0;
  for (const draft of drafts) {
    const scope = renderFragmentScope(draft, {
      indent: "  ",
      syntax: "ts",
      rewriteSource: (sourceExpr, outerNames) => rewriteInCheck(sourceExpr, outerNames),
      rewriteCondition: (exprText) =>
        rewriteInCheck(exprText, new Set(draft.bindings.length > 0 ? draft.bindings.map((b) => b.name) : draft.locals)),
    });
    unresolvedBindings += scope.unresolvedBindings;
    unresolvedGuards += scope.unresolvedGuards;
    pieces.push({ kind: "text", text: scope.open });
    pieces.push({ kind: "text", text: scope.mountIndent });
    pieces.push({
      kind: "fragment",
      fragmentId: draft.fragmentId,
      mountId: `${draft.fragmentId}#0`,
      text: draft.mountedText,
      target: draft.target,
      targetOffset: draft.targetOffset,
      hoverTargets: draft.hoverTargets,
    });
    pieces.push({ kind: "text", text: `;\n` });
    pieces.push({ kind: "text", text: scope.close });
  }
  pieces.push({ kind: "text", text: `}\n` });

  const assembledCheck = assemble("", [{ id: "check-file", kind: "check-file", at: 0, pieces }], checkPath);

  const fragments: FragmentRecord[] = drafts.map((draft) => ({
    fragmentId: draft.fragmentId,
    expr: draft.expr,
    root: draft.root,
    target: draft.target,
    locals: draft.locals,
    bindings: draft.bindings,
    guards: draft.guards,
    localRoot: draft.localRoot,
    mounts: assembledCheck.mounts
      .filter((mount) => mount.fragmentId === draft.fragmentId)
      .map((mount) => ({
        mountId: mount.mountId,
        fragmentId: mount.fragmentId,
        insertionId: mount.insertionId,
        file: checkPath,
        text: mount.text,
        startUtf16: mount.startUtf16,
        targetStartUtf16: mount.targetStartUtf16,
        targetLengthUtf16: mount.targetLengthUtf16,
        target: draft.target,
        hoverTargets: mount.hoverTargets,
        segments: buildSegments(mount.startUtf16, draft),
      })),
  }));

  const checkInsertion = assembledCheck.insertions.find((insertion) => insertion.insertionId === "check-file");
  if (!checkInsertion) throw new Error("layout B: check-file insertion missing");

  const generatedDeclarations: GeneratedDeclaration[] = marks.map((mark) => {
    const declaration = context.propertyScan.properties.find((property) => property.name === mark.name);
    return {
      file: checkPath,
      start: checkInsertion.startUtf16 + mark.offsetInText,
      length: mark.length,
      ...(declaration
        ? {
            source: {
              file: context.scriptPath,
              start: byteOffset(context.scriptText, declaration.nameNode.getStart()),
              end: byteOffset(context.scriptText, declaration.nameNode.getEnd()),
            },
          }
        : {}),
    };
  });

  const anchorInsertion = assembledScript.insertions.find((insertion) => insertion.insertionId === "anchor");
  if (!anchorInsertion) throw new Error("layout B: anchor insertion missing");
  const anchorOffset = assembledScript.text.indexOf(`const ${ROOT_VAR}`, anchorInsertion.startUtf16);

  const behaviorIncomplete =
    context.behavior.data.length > 0 ||
    context.behavior.properties.length > 0 ||
    context.behavior.dataIncomplete ||
    context.behavior.propertiesIncomplete ||
    context.behavior.problems.length > 0;
  if (behaviorIncomplete) {
    buildStatus = "partial";
    reason ??= "component behaviors are not expanded by the symbol-bridge layout";
    notes.push(
      "behavior data/properties require the in-place-shadow layout; behavior roots are not diagnosed as missing",
    );
  }

  if (unresolvedBindings > 0 || unresolvedGuards > 0) {
    buildStatus = "partial";
    const unresolvedReason = "some template bindings or guards could not be resolved and were not checked";
    reason = reason ? `${reason}; ${unresolvedReason}` : unresolvedReason;
    notes.push("unresolved `wx:for` bindings or `wx:if` guards are mounted as `any`/unnarrowed and not checked");
  }

  return {
    layout: "symbol-bridge",
    buildStatus,
    ...(reason ? { reason } : {}),
    unresolvedBindings,
    unresolvedGuards,
    behaviorIncomplete,
    knownRoots: [
      ...context.dataNames,
      // Behavior properties/data are not mounted by this layout; leave those
      // roots to the incomplete-behavior suppression instead of reporting a
      // missing property that the layout itself could not expose.
      ...context.propertyScan.properties
        .filter((property) => property.source === undefined)
        .map((property) => property.name),
    ],
    files: [
      { path: context.scriptPath, text: assembledScript.text },
      { path: checkPath, text: assembledCheck.text },
    ],
    queryFile: checkPath,
    insertions: [...assembledScript.insertions, ...assembledCheck.insertions],
    fragments,
    generatedDeclarations,
    jsCheckApplied: context.jsCheckPolicy === "off" || context.pragma.noCheck ? "off" : "inherit",
    notes,
    edits: assembledScript.edits,
    anchor: { file: context.scriptPath, offset: anchorOffset },
    probeAnchors: {
      file: context.scriptPath,
      rootOffset: anchorOffset + `const ${ROOT_VAR} = `.length,
      helperOffsets: [
        { name: OPTIONS_VAR, offset: assembledScript.text.indexOf(`let ${OPTIONS_VAR}`) + "let ".length },
        { name: ROOT_VAR, offset: anchorOffset + "const ".length },
        ...(hasProperties
          ? [
              {
                name: PROPS_TYPE_NAME,
                offset: assembledCheck.text.indexOf(`type ${PROPS_TYPE_NAME}`) + "type ".length,
                file: checkPath,
              },
            ]
          : []),
      ].filter((helper) => helper.offset >= "let".length),
    },
    moduleAfter: moduleKindText(context.scriptPath, assembledScript.text),
  };
}

function byteOffset(text: string, utf16Index: number): number {
  return Buffer.byteLength(text.slice(0, utf16Index), "utf8");
}

/** Rewrite a template sub-expression against the exported anchor. */
function rewriteInCheck(exprText: string, locals: ReadonlySet<string>): string | undefined {
  try {
    return rewriteExpression(exprText, undefined, {
      rootPrefix: `${ROOT_VAR}.`,
      propsPrefix: PROPS_PREFIX,
      propertyOverlay: new Set(),
      dataOverlay: new Set(),
      locals,
    }).text;
  } catch {
    return undefined;
  }
}

function optionsStart(call: ts.CallExpression): number {
  const argument = call.arguments[0];
  return argument ? argument.getStart() : call.getEnd() - 1;
}

function statementOf(node: ts.Node): ts.Statement {
  let current: ts.Node = node;
  while (current.parent && !ts.isSourceFile(current.parent) && !ts.isBlock(current.parent)) {
    current = current.parent;
  }
  return current as ts.Statement;
}

function moduleSpecifier(scriptPath: string): string {
  return path.basename(scriptPath).replace(/\.[^.]+$/, "");
}

function moduleKindText(fileName: string, text: string): "script" | "module" {
  const sourceFile = ts.createSourceFile(fileName, text, ts.ScriptTarget.Latest, true);
  const candidate = sourceFile as ts.SourceFile & { externalModuleIndicator?: ts.Node };
  return candidate.externalModuleIndicator === undefined ? "script" : "module";
}

function emptyBuild(context: LayoutContext, status: CapabilityStatus, reason: string): LayoutBuild {
  return {
    layout: "symbol-bridge",
    buildStatus: status,
    reason,
    files: [{ path: context.scriptPath, text: context.scriptText }],
    queryFile: path.join(path.dirname(context.scriptPath), CHECK_FILE_NAME),
    insertions: [],
    fragments: [],
    generatedDeclarations: [],
    jsCheckApplied: context.jsCheckPolicy === "off" || context.pragma.noCheck ? "off" : "inherit",
    notes: [],
    edits: [],
    moduleAfter: moduleKindText(context.scriptPath, context.scriptText),
  };
}

export { OPTIONS_VAR, ROOT_VAR };
