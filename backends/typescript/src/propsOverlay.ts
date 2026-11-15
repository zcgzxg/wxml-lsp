/** Rendering of the declaration-derived `properties` overlay (TypeScript and JSDoc). */

import type { DeclaredProperty } from "./ast.ts";

export const PROPS_TYPE_NAME = "__WxmlProps_0";
export const PROPS_VALUE_NAME = "__wxml_props_0";
export const PROPS_RAW_NAME = "__wxml_props_raw_0";

export interface RenderedMember {
  name: string;
  /** Offset of the member name inside the rendered text. */
  offset: number;
  length: number;
}

export interface RenderedProps {
  text: string;
  members: RenderedMember[];
}

class Builder {
  private readonly parts: string[] = [];
  private length = 0;

  add(text: string): void {
    this.parts.push(text);
    this.length += text.length;
  }

  get offset(): number {
    return this.length;
  }

  toString(): string {
    return this.parts.join("");
  }
}

/**
 * Render a TypeScript type literal for declared component properties.
 *
 * The overlay exists because `miniprogram-api-typings@5.3.1` maps
 * `FullProperty<T, V>` to `ValueType<T>` and ignores `optionalTypes`, so the
 * instance `data` type under-reports unions declared as
 * `{ type: String, optionalTypes: [Number] }`.
 */
export function renderPropsType(properties: DeclaredProperty[], indent: string): RenderedProps {
  const builder = new Builder();
  const members: RenderedMember[] = [];
  builder.add(`${indent}type ${PROPS_TYPE_NAME} = {\n`);
  for (const property of properties) {
    if (property.doc) builder.add(`${indent}  /** ${sanitizeDoc(property.doc)} */\n`);
    builder.add(`${indent}  `);
    const nameText = quoteName(property.name);
    // A quoted name is `"a-b"`: the definition target is the name, not the quotes.
    const quoted = nameText.length !== property.name.length;
    members.push({
      name: property.name,
      offset: builder.offset + (quoted ? 1 : 0),
      length: property.name.length,
    });
    builder.add(`${nameText}: ${property.typeText};\n`);
  }
  builder.add(`${indent}};`);
  return { text: builder.toString(), members };
}

/**
 * Render the overlay value.
 *
 * TypeScript uses a type assertion; JavaScript uses a JSDoc cast from `unknown`
 * (casting `null` directly is a 2352 error under `checkJs`).
 */
export function renderPropsValue(
  properties: DeclaredProperty[],
  indent: string,
  syntax: "ts" | "js",
): RenderedProps {
  if (syntax === "ts") {
    return { text: `${indent}const ${PROPS_VALUE_NAME} = null as unknown as ${PROPS_TYPE_NAME};`, members: [] };
  }
  const builder = new Builder();
  const members: RenderedMember[] = [];
  builder.add(`${indent}/**\n${indent} * @typedef {Object} ${PROPS_TYPE_NAME}\n`);
  for (const property of properties) {
    builder.add(`${indent} * @property {${property.typeText}} `);
    const nameText = quoteName(property.name);
    const quoted = nameText.length !== property.name.length;
    members.push({
      name: property.name,
      offset: builder.offset + (quoted ? 1 : 0),
      length: property.name.length,
    });
    const doc = property.doc ? ` ${sanitizeDoc(property.doc)}` : "";
    builder.add(`${nameText}${doc}\n`);
  }
  builder.add(`${indent} */\n`);
  builder.add(`${indent}/** @type {unknown} */\n${indent}const ${PROPS_RAW_NAME} = null;\n`);
  builder.add(`${indent}const ${PROPS_VALUE_NAME} = /** @type {${PROPS_TYPE_NAME}} */ (${PROPS_RAW_NAME});`);
  return { text: builder.toString(), members };
}

function sanitizeDoc(doc: string): string {
  return doc.replace(/\*\//g, "*\\/").replace(/\s+/g, " ").trim();
}

function quoteName(name: string): string {
  return /^[\p{L}_$][\p{L}\p{N}_$]*$/u.test(name) ? name : JSON.stringify(name);
}
