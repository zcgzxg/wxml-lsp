import assert from "node:assert/strict";
import { test } from "node:test";
import { BACKEND_DIR, SAMPLES_DIR } from "../src/env.ts";
import { inspectLayout, runScenario, type RunConfig } from "../src/run.ts";
import { scenarioById } from "../src/scenarios.ts";
import { assemble, byteToUtf16, mapSpanToSource, utf16ToByte, type MountLike } from "../src/text.ts";

const config: RunConfig = { backendDir: BACKEND_DIR, samplesDir: SAMPLES_DIR };

test("UTF-16 and byte offsets agree around astral characters", () => {
  const text = "😀中文\nabc";
  assert.equal(utf16ToByte(text, 0), 0);
  assert.equal(utf16ToByte(text, 2), 4);
  assert.equal(utf16ToByte(text, 3), 7);
  assert.equal(utf16ToByte(text, 1), undefined, "mid-surrogate is not a byte boundary");
  assert.equal(byteToUtf16(text, 4), 2);
  assert.equal(byteToUtf16(text, 3), undefined, "mid-codepoint is not an index");
});

test("mapped span trims a synthetic prefix and keeps the source part", () => {
  const mount: MountLike = {
    startUtf16: 100,
    text: "__wxml_root.maybeUser",
    segments: [
      { startUtf16: 100, lengthUtf16: 12 },
      { startUtf16: 112, lengthUtf16: 9, source: { start: 10, end: 19 } },
    ],
  };
  const mapped = mapSpanToSource(mount, 100, 21);
  assert.deepEqual(mapped.range, { start: 10, end: 19 });
  assert.equal(mapped.trimmedSyntheticPrefix, true);
});

test("a span entirely inside synthetic text is not attributable", () => {
  const mount: MountLike = {
    startUtf16: 0,
    text: "__wxml_root.title",
    segments: [
      { startUtf16: 0, lengthUtf16: 12 },
      { startUtf16: 12, lengthUtf16: 5, source: { start: 3, end: 8 } },
    ],
  };
  assert.equal(mapSpanToSource(mount, 0, 12).unattributable, true);
  assert.equal(mapSpanToSource(mount, 8, 2).unattributable, true, "span ends before the source part");
  assert.deepEqual(mapSpanToSource(mount, 14, 2).range, { start: 5, end: 7 }, "offset inside the source part");
});

test("assemble maps offsets after an insertion with several synthetic prefixes", () => {
  const shadow = assemble(
    "abc",
    [
      {
        id: "probe",
        kind: "probe",
        at: 0,
        pieces: [
          { kind: "text", text: "x " },
          {
            kind: "fragment",
            fragmentId: "f",
            mountId: "f#0",
            text: "__wxml_root.a + __wxml_root.b",
            target: "b",
            targetOffset: 28,
          },
          { kind: "text", text: " y" },
        ],
      },
    ],
    "index.ts",
  );
  const insertionLength = shadow.text.length - "abc".length;
  assert.equal(shadow.toOriginalOffset(insertionLength - 1), undefined);
  assert.equal(shadow.toOriginalOffset(insertionLength), 0);
  assert.equal(shadow.toOriginalOffset(insertionLength + 1), 1);
});

test("a mapped span never exceeds the recorded source range", () => {
  const mount: MountLike = {
    startUtf16: 0,
    text: "__wxml_root.title",
    segments: [
      { startUtf16: 0, lengthUtf16: 12 },
      { startUtf16: 12, lengthUtf16: 5, source: { start: 2, end: 4 } },
    ],
  };
  assert.equal(mapSpanToSource(mount, 12, 5).unattributable, true, "5 text bytes cannot fit 2 source bytes");
  assert.deepEqual(mapSpanToSource(mount, 12, 2).range, { start: 2, end: 4 });
});

test("out-of-bounds spans and non-character boundaries are unattributable", () => {
  const mount: MountLike = {
    startUtf16: 100,
    text: "__wxml_root.😀",
    segments: [
      { startUtf16: 100, lengthUtf16: 12 },
      { startUtf16: 112, lengthUtf16: 2, source: { start: 10, end: 14 } },
    ],
    targetStartUtf16: 112,
    targetLengthUtf16: 2,
  };
  assert.equal(mapSpanToSource(mount, 100, 15).unattributable, true, "end beyond the mount");
  assert.equal(mapSpanToSource(mount, 99, 2).unattributable, true, "start before the mount");
  assert.equal(mapSpanToSource(mount, 112, 3).unattributable, true, "targetLength + 1");
  assert.equal(mapSpanToSource(mount, 113, 1).unattributable, true, "low-surrogate start");
  assert.equal(mapSpanToSource(mount, 112, 1).unattributable, true, "high-surrogate end");
  assert.deepEqual(mapSpanToSource(mount, 112, 2).range, { start: 10, end: 14 });
});

test("fractional and negative span bounds are unattributable", () => {
  const mount: MountLike = {
    startUtf16: 100,
    text: "__wxml_root.title",
    segments: [
      { startUtf16: 100, lengthUtf16: 12 },
      { startUtf16: 112, lengthUtf16: 5, source: { start: 0, end: 5 } },
    ],
    targetStartUtf16: 112,
    targetLengthUtf16: 5,
  };
  assert.equal(mapSpanToSource(mount, 112.5, 1).unattributable, true, "fractional start");
  assert.equal(mapSpanToSource(mount, 112, 0.5).unattributable, true, "fractional length");
  assert.equal(mapSpanToSource(mount, 112, -1).unattributable, true, "negative length");
});

test("cross-gap target is used only when fully inside the span", () => {
  const mount: MountLike = {
    startUtf16: 0,
    text: "__wxml_root.a + __wxml_root.b",
    segments: [
      { startUtf16: 0, lengthUtf16: 12 },
      { startUtf16: 12, lengthUtf16: 1, source: { start: 0, end: 1 } },
      { startUtf16: 13, lengthUtf16: 3, source: { start: 1, end: 4 } },
      { startUtf16: 16, lengthUtf16: 12 },
      { startUtf16: 28, lengthUtf16: 1, source: { start: 4, end: 5 } },
    ],
    targetStartUtf16: 28,
    targetLengthUtf16: 1,
  };
  const mapped = mapSpanToSource(mount, 10, 8);
  assert.equal(mapped.trimmedSyntheticPrefix, true);
  assert.deepEqual(mapped.range, { start: 1, end: 4 }, "largest overlap, not the target outside the span");
});

test("every mount carries an explicit synthetic/source split", () => {
  for (const id of ["page-basic", "component-properties", "unicode-crlf", "component-js"]) {
    const spec = scenarioById(id);
    for (const layout of ["in-place-shadow", "symbol-bridge"] as const) {
      const build = inspectLayout(spec, layout, config);
      const insertionIds = new Set(build.insertions.map((insertion) => insertion.insertionId));
      for (const fragment of build.fragments) {
        assert.ok(fragment.mounts.length >= 1, `${id}/${layout}: ${fragment.expr} has no mount`);
        for (const mount of fragment.mounts) {
          assert.ok(mount.segments.length >= 1, `${id}/${layout}: mount must have segments`);
          assert.ok(insertionIds.has(mount.insertionId), "mount must reference a recorded insertion");
          let rebuilt = "";
          let sourceBytes = 0;
          for (const segment of mount.segments) {
            const from = segment.startUtf16 - mount.startUtf16;
            rebuilt += mount.text.slice(from, from + segment.lengthUtf16);
            if (segment.source) {
              assert.ok(segment.source.end >= segment.source.start);
              sourceBytes += segment.source.end - segment.source.start;
            }
          }
          assert.equal(rebuilt, mount.text, `${id}/${layout}: segments must cover the mount`);
          assert.equal(
            sourceBytes,
            Buffer.byteLength(fragment.expr, "utf8"),
            `${id}/${layout}: source segments must cover the expression exactly`,
          );
          assert.ok(
            mount.segments.some((segment) => segment.source !== undefined),
            "every mount contains original WXML text",
          );
        }
      }
    }
  }
  const spec = scenarioById("unicode-crlf");
  const run = runScenario(spec, "in-place-shadow", config);
  const diagnostic = run.diagnostics[0];
  assert.ok(diagnostic?.range);
  const bytes = Buffer.from(spec.wxml, "utf8").subarray(diagnostic.range.start, diagnostic.range.end);
  assert.equal(bytes.toString("utf8"), "nmae");
});

test("nullable receiver diagnostics land on the receiver, not the member", () => {
  const spec = scenarioById("nullable");
  for (const layout of ["in-place-shadow", "symbol-bridge"] as const) {
    const run = runScenario(spec, layout, config);
    assert.equal(run.diagnosticsExact, true, `${layout} diagnostics`);
    const wxmlBytes = Buffer.from(spec.wxml, "utf8");
    const mapped = run.diagnostics
      .filter((diagnostic) => diagnostic.range)
      .map((diagnostic) => wxmlBytes.subarray(diagnostic.range!.start, diagnostic.range!.end).toString("utf8"))
      .sort();
    assert.deepEqual(mapped, ["external", "maybeUser"]);
    assert.ok(
      run.diagnostics.every((diagnostic) => diagnostic.trimmedSyntheticPrefix),
      "receiver diagnostics start inside the synthetic root prefix",
    );
    // The published message must not contain helper names.
    assert.ok(run.diagnostics.every((diagnostic) => !diagnostic.message.includes("__wxml")));
  }
});

test("one expression mounted twice keeps both mappings and publishes one diagnostic", () => {
  const spec = scenarioById("page-basic");
  for (const layout of ["in-place-shadow", "symbol-bridge"] as const) {
    const single = runScenario(spec, layout, config);
    const doubled = runScenario(spec, layout, config, { mountRepeat: 2 });
    assert.equal(doubled.diagnosticsExact, true, `${layout}: duplicated mounts must not duplicate results`);
    assert.equal(doubled.diagnostics.length, single.diagnostics.length, `${layout}: diagnostics are deduplicated by code and span`);
    assert.deepEqual(doubled.diagnostics[0]?.range, single.diagnostics[0]?.range);
    assert.equal(
      doubled.hovers.find((entry) => entry.query === "user.name")?.observation?.typeOnly,
      "string",
    );
  }
});

test("BOM is dropped whenever a pragma must be honoured", () => {
  for (const id of ["js-bom-check-off", "js-bom-check-inherit"]) {
    const spec = scenarioById(id);
    const build = inspectLayout(spec, "in-place-shadow", config);
    const run = runScenario(spec, "in-place-shadow", config);
    const derived = build.files[0]!.text;
    assert.ok(!derived.startsWith("\uFEFF"), `${id}: BOM must be dropped`);
    assert.ok(derived.startsWith("#!"), `${id}: hashbang stays first`);
    const bom = run.insertions.find((insertion) => insertion.insertionId === "bom");
    assert.ok(bom, `${id}: BOM replacement must be recorded`);
    assert.equal(bom.kind, "edit");
    assert.equal(bom.replaceLengthUtf16, 1);
    assert.ok(build.notes.some((note) => note.includes("BOM dropped")), `${id}: note must explain the drop`);
  }
});

test("published evidence carries insertions and mounts for every run", () => {
  for (const id of ["page-basic", "component-properties", "js-shebang-bom", "nullable"]) {
    const spec = scenarioById(id);
    for (const layout of ["in-place-shadow", "symbol-bridge"] as const) {
      const run = runScenario(spec, layout, config);
      assert.ok(run.insertions.length > 0, `${id}/${layout}: insertions must be part of the response`);
      assert.ok(run.mounts.length > 0, `${id}/${layout}: mounts must be part of the response`);
      const insertionIds = new Set(run.insertions.map((insertion) => insertion.insertionId));
      for (const mount of run.mounts) {
        assert.ok(insertionIds.has(mount.insertionId), `${id}/${layout}: mount references a recorded insertion`);
        assert.equal(mount.segments.length, 2, `${id}/${layout}: synthetic + source`);
        assert.equal(mount.segments[0]?.source, undefined);
        assert.ok(mount.segments[1]?.source);
        for (const insertion of run.insertions) {
          assert.ok(insertion.replaceLengthUtf16 >= 0);
          assert.ok(insertion.startUtf16 >= 0);
        }
      }
    }
  }
});

test("mount target length is published and matches the mounted identifier", () => {
  for (const id of ["page-basic", "component-properties", "unicode-crlf"]) {
    const spec = scenarioById(id);
    for (const layout of ["in-place-shadow", "symbol-bridge"] as const) {
      const build = inspectLayout(spec, layout, config);
      const run = runScenario(spec, layout, config);
      for (const fragment of build.fragments) {
        for (const mount of fragment.mounts) {
          const evidence = run.mounts.find((entry) => entry.mountId === mount.mountId);
          assert.ok(evidence, `${id}/${layout}: mount must be published`);
          assert.equal(evidence.targetLengthUtf16, mount.target.length, `${id}/${layout}: ${mount.mountId}`);
          assert.equal(evidence.segments[0]!.startUtf16, mount.segments[0]!.startUtf16);
        }
      }
    }
  }
});

test("BOM removal is a one-character replacement, not an insertion", () => {
  const spec = scenarioById("js-shebang-bom");
  const build = inspectLayout(spec, "in-place-shadow", config);
  const run = runScenario(spec, "in-place-shadow", config);
  const derived = build.files[0]!.text;
  assert.ok(!derived.startsWith("\uFEFF"), "the derived view drops the BOM");
  assert.ok(derived.startsWith("#!"), "the hashbang stays first");

  const bom = run.insertions.find((insertion) => insertion.insertionId === "bom");
  assert.ok(bom, "BOM removal must be a recorded insertion");
  assert.equal(bom.kind, "edit");
  assert.equal(bom.startUtf16, 0);
  assert.equal(bom.lengthUtf16, 0);
  assert.equal(bom.replaceLengthUtf16, 1);

  const pragma = run.insertions.find((insertion) => insertion.insertionId === "pragma");
  assert.ok(pragma);
  assert.equal(pragma.replaceLengthUtf16, 0);
  assert.equal(derived.slice(pragma.startUtf16, pragma.startUtf16 + pragma.lengthUtf16), "// @ts-check\n");

  // The published mount offsets point into the derived text, after the -1 shift.
  for (const mount of run.mounts) {
    assert.equal(derived.slice(mount.startUtf16, mount.startUtf16 + mount.segments.reduce((n, s) => n + s.lengthUtf16, 0)).length, mount.segments.reduce((n, s) => n + s.lengthUtf16, 0));
  }
  assert.equal(run.diagnosticsExact, true);
});

test("suppressed diagnostics stay out of the published set", () => {
  const spec = scenarioById("js-jsdoc-pragma");
  const run = runScenario(spec, "in-place-shadow", config);
  assert.ok(run.suppressedDiagnostics > 0);
  assert.deepEqual(
    run.diagnostics.map((diagnostic) => diagnostic.code),
    [2339],
    "only the mapped template diagnostic is published",
  );
  assert.deepEqual(
    run.suppressedDetails.map((entry) => entry.code),
    [2322],
    "the unrelated script error is reported as suppressed, not published",
  );
});
