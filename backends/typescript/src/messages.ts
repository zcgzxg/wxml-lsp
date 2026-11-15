/**
 * User-visible message sanitization.
 *
 * TypeScript messages embed bridge helper names and typings internals
 * (`__wxml_root`, `FilterUnknownType<...>`, `WechatMiniprogram.*`). None of that
 * is meaningful to a WXML author, so the published message is a cleaned,
 * truncated variant while the raw text stays internal.
 */

const ROOT_PREFIX_PATTERN = /__wxml_root_?[0-9]*\.|__wxml_props_[0-9]*\./g;
const HELPER_PATTERN = /__wxml_[A-Za-z0-9_]*/g;
const PROPS_TYPE_PATTERN = /__WxmlProps_[0-9]*/g;
const TYPINGS_PATTERN = /WechatMiniprogram\.[A-Za-z0-9_.$]*/g;
const WRAPPERS = [
  "FilterUnknownType",
  "PropertyOptionToData",
  "FullPropertyToData",
  "MixinProperties",
  "MixinMethods",
  "MixinData",
  "ExtractProperties",
  "ExtractMethods",
  "ExtractBehaviorType",
];

const MAX_LENGTH = 200;

export function sanitizeMessage(raw: string): string {
  let text = raw;
  for (const wrapper of WRAPPERS) text = unwrapGeneric(text, wrapper);
  text = text.replace(ROOT_PREFIX_PATTERN, "");
  text = text.replace(HELPER_PATTERN, "helper");
  text = text.replace(PROPS_TYPE_PATTERN, "declared properties");
  text = text.replace(TYPINGS_PATTERN, "miniprogram instance");
  text = collapseDuplicatedIntersections(text);
  text = dedupeRepeatedLiterals(text);
  text = text.replace(/\s*&\s*(?:\.\.\.|\{\s*\})/g, "");
  text = text.replace(/\s*&\s*(?=['"])/g, "");
  text = text.replace(/(?<=['"])\s*&\s*/g, "");
  text = text.replace(/\bhelper\s*\./g, "helper.");
  text = text.replace(/\s+/g, " ").trim();
  text = text.replace(/&\s*&/g, "&").trim();
  if (text.length > MAX_LENGTH) text = `${text.slice(0, MAX_LENGTH - 1).trimEnd()}…`;
  return text;
}

/** Replace `Wrapper<X>` with `X` (single type argument). */
function unwrapGeneric(text: string, name: string): string {
  const marker = `${name}<`;
  let output = "";
  let index = 0;
  while (index < text.length) {
    const start = text.indexOf(marker, index);
    if (start < 0) {
      output += text.slice(index);
      break;
    }
    output += text.slice(index, start);
    let depth = 0;
    let cursor = start + name.length;
    for (; cursor < text.length; cursor += 1) {
      const char = text[cursor];
      if (char === "<") depth += 1;
      if (char === ">") {
        depth -= 1;
        if (depth === 0) {
          cursor += 1;
          break;
        }
      }
    }
    output += text.slice(start + marker.length, cursor - 1);
    index = cursor;
  }
  return output;
}

/**
 * Remove a repeated `{ ... }` literal.
 *
 * Quick info for candidate A produces intersection types with duplicate
 * members; the sanitized message should read like the original declaration.
 */
function dedupeRepeatedLiterals(text: string): string {
  const literals = [...text.matchAll(/\{[^{}]*\}/g)].map((match) => match[0]);
  let output = text;
  for (const literal of literals) {
    const first = output.indexOf(literal);
    if (first < 0) continue;
    const second = output.indexOf(literal, first + literal.length);
    if (second < 0) continue;
    output = `${output.slice(0, second)}${output.slice(second + literal.length)}`;
    output = output.replace(/\s*&\s*&\s*/g, " & ");
  }
  return output;
}

/** `{ a: string } & { a: string }` -> `{ a: string }` (same text, duplicated). */
function collapseDuplicatedIntersections(text: string): string {
  const parts = text.split(/\s+&\s+/);
  if (parts.length < 2) return text;
  const unique: string[] = [];
  for (const part of parts) {
    if (!unique.includes(part)) unique.push(part);
  }
  return unique.join(" & ");
}
