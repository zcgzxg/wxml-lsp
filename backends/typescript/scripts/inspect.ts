/**
 * Inspect one scenario/candidate pair: derived files, insertions, mounts,
 * generated declarations, mapped diagnostics and hover results.
 *
 *   node scripts/inspect.ts page-basic in-place-shadow
 *   node scripts/inspect.ts component-properties symbol-bridge
 */

import path from "node:path";
import { defaultReadPolicy, VirtualProject } from "../src/compiler.ts";
import { BACKEND_DIR, SAMPLES_DIR } from "../src/env.ts";
import { toCompilerOptions } from "../src/protocol.ts";
import { hoverAt, semanticDiagnostics } from "../src/queries.ts";
import { inspectLayout, inspectRequest } from "../src/run.ts";
import { scenarioById } from "../src/scenarios.ts";
import type { LayoutId } from "../src/types.ts";

const scenarioId = process.argv[2] ?? "page-basic";
const layout = (process.argv[3] ?? "in-place-shadow") as LayoutId;
const spec = scenarioById(scenarioId);
const config = { backendDir: BACKEND_DIR, samplesDir: SAMPLES_DIR };

const request = inspectRequest(spec, layout, config);
const build = inspectLayout(spec, layout, config);
console.log(`scenario: ${spec.id} (${spec.registration}), candidate: ${layout}`);
console.log(`build: ${build.buildStatus}${build.reason ? ` — ${build.reason}` : ""}`);
console.log(`js check: ${build.jsCheckApplied}`);
for (const note of build.notes) console.log(`note: ${note}`);
for (const file of build.files) {
  console.log(`\n=== ${path.relative(BACKEND_DIR, file.path)} ===`);
  console.log(file.text);
}
console.log("\ninsertions:");
for (const insertion of build.insertions) {
  console.log(
    `  ${insertion.insertionId} (${insertion.kind}) ${path.basename(insertion.file)} ` +
      `[${insertion.startUtf16}, ${insertion.startUtf16 + insertion.lengthUtf16}) replace=${insertion.replaceLengthUtf16}`,
  );
}
console.log("\ngenerated declarations:");
for (const declaration of build.generatedDeclarations) {
  console.log(
    `  ${path.basename(declaration.file)} [${declaration.start}, ${declaration.start + declaration.length}) -> ${
      declaration.source ? `${declaration.source.start}..${declaration.source.end}` : "unmapped"
    }`,
  );
}
console.log("\nmounts:");
for (const fragment of build.fragments) {
  for (const mount of fragment.mounts) {
    console.log(`  ${mount.mountId} → ${mount.text} @${mount.startUtf16} (target ${mount.targetStartUtf16})`);
    for (const segment of mount.segments) {
      console.log(
        `    [${segment.startUtf16}, ${segment.startUtf16 + segment.lengthUtf16}) ${
          segment.source ? `WXML bytes ${segment.source.start}..${segment.source.end}` : "synthetic"
        }`,
      );
    }
  }
}

if (build.fragments.length > 0) {
  const project = new VirtualProject({
    rootDir: BACKEND_DIR,
    compilerOptions: toCompilerOptions(request.project.compilerOptions),
    files: [
      ...request.files.map((file) => ({
        path: path.resolve(request.project.projectRoot, file.path),
        text: file.text,
      })),
      ...build.files,
    ],
    readPolicy: defaultReadPolicy(BACKEND_DIR),
  });
  const filesWithMounts = [...new Set(build.fragments.flatMap((fragment) => fragment.mounts.map((mount) => mount.file)))];
  for (const file of filesWithMounts) {
    console.log(`\nraw diagnostics: ${path.relative(BACKEND_DIR, file)}`);
    for (const diagnostic of semanticDiagnostics(project, file)) {
      console.log(`  ${diagnostic.code} @${diagnostic.start}+${diagnostic.length} ${diagnostic.message.slice(0, 140)}`);
    }
  }
  console.log("\nhover:");
  for (const fragment of build.fragments) {
    const mount = fragment.mounts[0];
    if (!mount) continue;
    const observation = hoverAt(project, mount.file, mount.targetStartUtf16);
    console.log(
      `  ${fragment.fragmentId} (${fragment.expr}) → ${
        observation ? `${observation.typeOnly} (${observation.quality}, ${observation.documentationSource})` : "none"
      }`,
    );
  }
}
