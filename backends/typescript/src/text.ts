/**
 * Text helpers: UTF-16 offsets (TypeScript/Node) versus UTF-8 bytes (WXML),
 * assembly of derived files from insertions, and span mapping back to WXML.
 */

import type { InsertionRecord, Segment, SourceByteRange, TextEdit } from "./types.ts";

/** Convert a UTF-16 index into a UTF-8 byte offset; `undefined` mid-surrogate. */
export function utf16ToByte(text: string, utf16Index: number): number | undefined {
  if (utf16Index < 0 || utf16Index > text.length) return undefined;
  if (utf16Index < text.length) {
    const code = text.charCodeAt(utf16Index);
    if (code >= 0xdc00 && code <= 0xdfff) return undefined;
  }
  return Buffer.byteLength(text.slice(0, utf16Index), "utf8");
}

/** Convert a UTF-8 byte offset into a UTF-16 index; `undefined` mid-codepoint. */
export function byteToUtf16(text: string, byteIndex: number): number | undefined {
  if (byteIndex < 0) return undefined;
  const buffer = Buffer.from(text, "utf8");
  if (byteIndex > buffer.length) return undefined;
  if (byteIndex < buffer.length && (buffer[byteIndex]! & 0xc0) === 0x80) return undefined;
  return buffer.subarray(0, byteIndex).toString("utf8").length;
}

/** Byte range for a UTF-16 span inside `text`. */
export function byteRangeOf(text: string, start: number, length: number): SourceByteRange | undefined {
  const startByte = utf16ToByte(text, start);
  const endByte = utf16ToByte(text, start + length);
  if (startByte === undefined || endByte === undefined) return undefined;
  return { start: startByte, end: endByte };
}

export type Piece =
  | { kind: "text"; text: string }
  | {
      kind: "fragment";
      fragmentId: string;
      mountId: string;
      text: string;
      target: string;
      targetOffset: number;
    };

export interface Insertion {
  id: string;
  kind: InsertionRecord["kind"];
  /** Offset in the original text. */
  at: number;
  /** Number of original characters replaced by this insertion (0 for pure insertions). */
  replaceLength?: number;
  pieces: Piece[];
}

export interface AssembledMount {
  mountId: string;
  fragmentId: string;
  insertionId: string;
  text: string;
  /** Offset of `text` inside the assembled file. */
  startUtf16: number;
  /** Offset of the target identifier inside the assembled file. */
  targetStartUtf16: number;
  /** Length of the target identifier in UTF-16 code units. */
  targetLengthUtf16: number;
}

export interface Assembled {
  text: string;
  mounts: AssembledMount[];
  edits: TextEdit[];
  insertions: InsertionRecord[];
  /** Map a derived-file offset back to the original text offset. */
  toOriginalOffset(offset: number): number | undefined;
}

/**
 * Build a derived file by inserting pieces into the original text.
 *
 * Insertions may remove a fixed number of original characters
 * (`replaceLength`), which the contract records as `replaceLengthUtf16`. That is
 * reserved for byte-order-mark removal when a `@ts-check` pragma has to be
 * honoured; everything else is a pure insertion. Keeping the map exact is what
 * lets every derived span be translated back to source bytes.
 */
export function assemble(original: string, insertions: Insertion[], file: string): Assembled {
  const ordered = [...insertions].sort((left, right) => left.at - right.at);
  const chunks: string[] = [];
  const record: InsertionRecord[] = [];
  const edits: TextEdit[] = [];
  const mounts: AssembledMount[] = [];
  let cursor = 0;
  let shadowLength = 0;

  for (const insertion of ordered) {
    if (insertion.at < cursor || insertion.at > original.length) {
      throw new Error(`insertion ${insertion.id} at ${insertion.at} is out of order or out of bounds`);
    }
    const prefix = original.slice(cursor, insertion.at);
    chunks.push(prefix);
    shadowLength += prefix.length;
    cursor = insertion.at;
    const replaceLength = insertion.replaceLength ?? 0;

    const insertedText = insertion.pieces.map((piece) => piece.text).join("");
    const insertionStart = shadowLength;
    record.push({
      insertionId: insertion.id,
      file,
      startUtf16: insertionStart,
      lengthUtf16: insertedText.length,
      replaceLengthUtf16: replaceLength,
      viewRevision: 0,
      backendFileVersion: 0,
      kind: insertion.kind,
    });
    edits.push({ at: insertion.at, length: replaceLength, text: insertedText, shadowStart: insertionStart });
    cursor += replaceLength;

    for (const piece of insertion.pieces) {
      if (piece.kind === "text") {
        chunks.push(piece.text);
        shadowLength += piece.text.length;
        continue;
      }
      mounts.push({
        mountId: piece.mountId,
        fragmentId: piece.fragmentId,
        insertionId: insertion.id,
        text: piece.text,
        startUtf16: shadowLength,
        targetStartUtf16: shadowLength + piece.targetOffset,
        targetLengthUtf16: piece.target.length,
      });
      chunks.push(piece.text);
      shadowLength += piece.text.length;
    }
  }
  chunks.push(original.slice(cursor));

  const toOriginalOffset = (offset: number): number | undefined => {
    let shift = 0;
    for (const edit of edits) {
      if (offset >= edit.shadowStart && offset < edit.shadowStart + edit.text.length) return undefined;
      if (offset >= edit.shadowStart + edit.text.length) shift += edit.text.length - edit.length;
      else break;
    }
    return offset - shift;
  };

  return { text: chunks.join(""), mounts, edits, insertions: record, toOriginalOffset };
}

export interface MappedSpan {
  range?: SourceByteRange;
  /** The span started inside a synthetic prefix and was clamped onto source text. */
  trimmedSyntheticPrefix: boolean;
  /** The span cannot be attributed to original WXML text. */
  unattributable: boolean;
}

export interface MountLike {
  startUtf16: number;
  text: string;
  segments: Segment[];
  /** Target identifier inside the mount (used to pick a safe operand). */
  targetStartUtf16?: number;
  targetLengthUtf16?: number;
}

/**
 * Map a derived-file span onto a WXML byte range.
 *
 * Rules (C1.1):
 * - a span entirely inside synthetic text is not attributable;
 * - a span that starts in a synthetic prefix and ends in source text is clamped
 *   to the source part (needed for `possibly null` diagnostics on
 *   `__wxml_root.maybeUser`);
 * - a span that crosses a synthetic gap is cropped to the mount target when the
 *   target is inside the span, otherwise to the source segment with the largest
 *   overlap. This keeps a real `Operator '<' cannot be applied ...` diagnostic
 *   visible instead of silently dropping it and reporting `Supported` with an
 *   empty set.
 * - out-of-bounds spans, fractional/negative offsets, and boundaries that split
 *   a surrogate pair are unattributable (never clamped).
 */
export function mapSpanToSource(mount: MountLike, start: number, length: number): MappedSpan {
  const end = start + length;
  const sourceSegments = mount.segments.filter((segment) => segment.source !== undefined);
  if (sourceSegments.length === 0) return { trimmedSyntheticPrefix: false, unattributable: true };
  const mountEnd = mount.startUtf16 + mount.text.length;
  if (
    !Number.isInteger(start) ||
    !Number.isInteger(length) ||
    length < 0 ||
    start < mount.startUtf16 ||
    end > mountEnd
  ) {
    return { trimmedSyntheticPrefix: false, unattributable: true };
  }

  let effectiveStart = start;
  let trimmed = false;
  // A span may start inside any synthetic prefix (not only the first one):
  // move it to the end of that prefix, then map the source part.
  const synthetic = mount.segments.find(
    (segment) =>
      segment.source === undefined && effectiveStart >= segment.startUtf16 && effectiveStart < segment.startUtf16 + segment.lengthUtf16,
  );
  if (synthetic) {
    effectiveStart = synthetic.startUtf16 + synthetic.lengthUtf16;
    trimmed = true;
  }
  if (end <= effectiveStart) return { trimmedSyntheticPrefix: trimmed, unattributable: true };

  const mapWithin = (segment: Segment, from: number, to: number): MappedSpan | undefined => {
    if (!segment.source || to <= from) return undefined;
    // Reject boundaries that split a surrogate pair (utf16ToByte returns
    // `undefined` for a low-surrogate index).
    if (
      utf16ToByte(mount.text, from - mount.startUtf16) === undefined ||
      utf16ToByte(mount.text, to - mount.startUtf16) === undefined
    ) {
      return undefined;
    }
    const relativeStart = mount.text.slice(segment.startUtf16 - mount.startUtf16, from - mount.startUtf16);
    const relativeEnd = mount.text.slice(segment.startUtf16 - mount.startUtf16, to - mount.startUtf16);
    const startByte = utf16ToByte(relativeStart, relativeStart.length);
    const endByte = utf16ToByte(relativeEnd, relativeEnd.length);
    if (startByte === undefined || endByte === undefined) return undefined;
    // Defence against a segment whose recorded source range is shorter than
    // the text it produced: never publish an out-of-range span.
    if (endByte < startByte || segment.source.end - segment.source.start < endByte) return undefined;
    return {
      range: { start: segment.source.start + startByte, end: segment.source.start + endByte },
      trimmedSyntheticPrefix: trimmed,
      unattributable: false,
    };
  };

  for (const segment of sourceSegments) {
    const segmentEnd = segment.startUtf16 + segment.lengthUtf16;
    if (effectiveStart >= segment.startUtf16 && end <= segmentEnd) {
      const mapped = mapWithin(segment, effectiveStart, end);
      if (mapped) return mapped;
      return { trimmedSyntheticPrefix: trimmed, unattributable: true };
    }
  }

  // The target is the safest operand to publish a cross-gap diagnostic on,
  // but only when it is fully inside the diagnostic span. Otherwise use the
  // largest-overlap source segment; never clamp onto a token outside the span.
  const targetStart = mount.targetStartUtf16;
  const targetLength = mount.targetLengthUtf16;
  if (targetStart !== undefined && targetLength !== undefined && targetLength > 0) {
    const targetEnd = targetStart + targetLength;
    if (targetStart >= start && targetEnd <= end) {
      const segment = sourceSegments.find(
        (candidate) => targetStart >= candidate.startUtf16 && targetEnd <= candidate.startUtf16 + candidate.lengthUtf16,
      );
      if (segment) {
        const mapped = mapWithin(segment, targetStart, targetEnd);
        if (mapped) return { ...mapped, trimmedSyntheticPrefix: true };
      }
    }
  }

  // Fall back to the source segment with the largest overlap with the span.
  const overlaps = sourceSegments
    .map((segment) => {
      const segmentEnd = segment.startUtf16 + segment.lengthUtf16;
      const from = Math.max(effectiveStart, segment.startUtf16);
      const to = Math.min(end, segmentEnd);
      return { segment, from, to, overlap: to - from };
    })
    .filter((candidate) => candidate.overlap > 0)
    .sort((left, right) => right.overlap - left.overlap);
  for (const candidate of overlaps) {
    const mapped = mapWithin(candidate.segment, candidate.from, candidate.to);
    if (mapped) return { ...mapped, trimmedSyntheticPrefix: true };
  }
  return { trimmedSyntheticPrefix: trimmed, unattributable: true };
}
