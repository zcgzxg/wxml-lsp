/** Mount segment construction shared by both candidates. */

import type { FragmentDraft, Segment } from "../types.ts";

/** Shift the draft's mounted-text segments to their absolute file offsets. */
export function buildSegments(startUtf16: number, draft: FragmentDraft): Segment[] {
  return draft.segments.map((segment) => ({
    ...segment,
    startUtf16: startUtf16 + segment.startUtf16,
  }));
}
