/** Query helpers on top of the language service, normalised for evidence. */

import path from "node:path";
import ts from "typescript";
import type { VirtualProject } from "./compiler.ts";
import type { DocumentationSource, HoverObservation, TypeQuality } from "./types.ts";

export interface RawDiagnostic {
  file: string;
  start: number;
  length: number;
  code: number;
  message: string;
}

export interface RawDefinition {
  file: string;
  start: number;
  length: number;
}

export function hoverAt(project: VirtualProject, file: string, offset: number): HoverObservation | null {
  const service = project.languageService();
  const quickInfo = service.getQuickInfoAtPosition(file, offset);
  if (!quickInfo) return null;
  const typeText = ts.displayPartsToString(quickInfo.displayParts).replace(/\s+/g, " ").trim();
  const typeOnly = typeOnlyOf(typeText);
  const quickInfoDoc = ts.displayPartsToString(quickInfo.documentation).trim();
  const symbolDoc = symbolDocumentationAt(project, file, offset);
  const documentation = quickInfoDoc || symbolDoc;
  const documentationSource: DocumentationSource = quickInfoDoc
    ? "quickinfo"
    : symbolDoc
      ? "symbol"
      : "none";
  return {
    typeText,
    typeOnly,
    documentation,
    documentationSource,
    quality: qualityAt(project, file, offset, typeOnly),
  };
}

export function semanticDiagnostics(project: VirtualProject, file: string): RawDiagnostic[] {
  return project
    .languageService()
    .getSemanticDiagnostics(file)
    .flatMap((diagnostic) => {
      if (diagnostic.file === undefined || diagnostic.start === undefined) return [];
      return [
        {
          file: path.resolve(diagnostic.file.fileName),
          start: diagnostic.start,
          length: diagnostic.length ?? 0,
          code: diagnostic.code,
          message: ts.flattenDiagnosticMessageText(diagnostic.messageText, " | "),
        },
      ];
    });
}

export function definitionsAt(project: VirtualProject, file: string, offset: number): RawDefinition[] {
  const definitions = project.languageService().getDefinitionAtPosition(file, offset) ?? [];
  return definitions.flatMap((definition) => {
    if (!definition.fileName) return [];
    return [
      {
        file: path.resolve(definition.fileName),
        start: definition.textSpan.start,
        length: definition.textSpan.length,
      },
    ];
  });
}

/** Strip the `(kind) name: ` prefix that quick info prepends. */
export function typeOnlyOf(typeText: string): string {
  const match = /^\([^)]*\)\s[^:]*:\s*([\s\S]*)$/.exec(typeText);
  if (match?.[1] !== undefined) return match[1].trim();
  const variable = /^(?:const|let|var)\s[^:]*:\s*([\s\S]*)$/.exec(typeText);
  if (variable?.[1] !== undefined) return variable[1].trim();
  return typeText.trim();
}

/**
 * Type quality of a hover.
 *
 * Quick info degrades to `any` for a property that does not exist, so the
 * contract needs one more distinction: an access on an `any` receiver is `Any`,
 * a missing member is `Missing`, and neither may show a concrete type.
 */
export function qualityAt(project: VirtualProject, file: string, offset: number, typeOnly: string): TypeQuality {
  const program = project.program();
  const sourceFile = program.getSourceFile(file);
  if (!sourceFile) return qualityOf(typeOnly);
  const node = findNodeAt(sourceFile, offset);
  if (!node) return qualityOf(typeOnly);
  const nameNode = ts.isIdentifier(node) ? node : undefined;
  const access =
    nameNode && ts.isPropertyAccessExpression(nameNode.parent) && nameNode.parent.name === nameNode
      ? nameNode.parent
      : undefined;
  if (!access) return qualityOf(typeOnly);
  const checker = project.checker();
  const symbol = checker.getSymbolAtLocation(access.name);
  const receiverText = checker.typeToString(checker.getTypeAtLocation(access.expression));
  const hasDeclaration = (symbol?.declarations?.length ?? 0) > 0;
  if (!hasDeclaration) {
    return receiverText === "any" ? "any" : "missing";
  }
  return qualityOf(typeOnly);
}

export function qualityOf(typeOnly: string): TypeQuality {
  const text = typeOnly.replace(/\s+/g, " ").trim();
  if (text.length === 0) return "missing";
  if (text === "any") return "any";
  if (text === "unknown") return "unknown";
  return "known";
}

function symbolDocumentationAt(project: VirtualProject, file: string, offset: number): string {
  const program = project.program();
  const sourceFile = program.getSourceFile(file);
  if (!sourceFile) return "";
  const node = findNodeAt(sourceFile, offset);
  if (!node) return "";
  const symbol = project.checker().getSymbolAtLocation(node);
  if (!symbol) return "";
  return ts.displayPartsToString(symbol.getDocumentationComment(project.checker())).trim();
}

function findNodeAt(sourceFile: ts.SourceFile, offset: number): ts.Node | undefined {
  let best: ts.Node | undefined;
  const visit = (node: ts.Node): void => {
    if (offset >= node.getStart(sourceFile) && offset < node.getEnd()) {
      best = node;
      node.forEachChild(visit);
    }
  };
  visit(sourceFile);
  return best;
}
