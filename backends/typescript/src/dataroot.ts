/**
 * Data-root classification through the type checker.
 *
 * A syntactic scan only catches same-file interfaces. The C1 contract has to
 * cover imported aliases too: `import type { User } from './models';
 * Page({ data: User })` must report `unknown`, never fabricate fields or leak a
 * bare 2693 diagnostic as "checked".
 */

import path from "node:path";
import ts from "typescript";
import type { DataRootKind } from "./ast.ts";
import type { VirtualProject } from "./compiler.ts";

export function classifyDataRoot(
  project: VirtualProject,
  scriptPath: string,
  dataInitializerOffset: number | undefined,
  syntacticTypeReference: boolean,
): DataRootKind {
  if (dataInitializerOffset === undefined) return "absent";
  const program = project.program();
  const sourceFile = program.getSourceFile(path.resolve(scriptPath));
  if (!sourceFile) return "unsupported";
  const node = findNodeAt(sourceFile, dataInitializerOffset);
  if (!node) return "unsupported";
  if (ts.isObjectLiteralExpression(node)) return "object";
  if (!ts.isIdentifier(node)) return "expression";
  if (syntacticTypeReference) return "type-reference";

  // A type used as a value is a 2693/2749 diagnostic at the identifier; this also
  // covers imported aliases whose symbol is not reachable without parent nodes.
  const start = node.getStart(sourceFile);
  const typeUsedAsValue = project
    .languageService()
    .getSemanticDiagnostics(path.resolve(scriptPath))
    .some(
      (diagnostic) =>
        (diagnostic.code === 2693 || diagnostic.code === 2749) &&
        diagnostic.start !== undefined &&
        diagnostic.start >= start &&
        diagnostic.start < start + node.getWidth(sourceFile),
    );
  return typeUsedAsValue ? "type-reference" : "expression";
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
