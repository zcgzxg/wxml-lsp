import assert from "node:assert/strict";
import { test } from "node:test";
import { sanitizeMessage } from "../src/messages.ts";

test("helper names are removed from user-visible messages", () => {
  assert.equal(
    sanitizeMessage("'__wxml_root.maybeUser' is possibly 'null'."),
    "'maybeUser' is possibly 'null'.",
  );
  assert.equal(
    sanitizeMessage("Property '__wxml_check_0' does not exist on type 'X'."),
    "Property 'helper' does not exist on type 'X'.",
  );
});

test("typings internals are unwrapped", () => {
  const raw =
    "Property 'subtitle' does not exist on type 'FilterUnknownType<{ state: string; }> & FilterUnknownType<{}> & PropertyOptionToData<FilterUnknownType<{ title: { type: StringConstructor; }; }>>'.";
  const clean = sanitizeMessage(raw);
  assert.ok(!clean.includes("FilterUnknownType"), clean);
  assert.ok(!clean.includes("PropertyOptionToData"), clean);
  assert.ok(clean.includes("{ state: string; }"), clean);
});

test("generated overlay type names are replaced", () => {
  assert.equal(
    sanitizeMessage("Property 'subtitle' does not exist on type '{ state: string; } & __WxmlProps_0'."),
    "Property 'subtitle' does not exist on type '{ state: string; } & declared properties'.",
  );
});

test("duplicated intersection literals collapse", () => {
  assert.equal(
    sanitizeMessage("Property 'nmae' does not exist on type '{ name: string; } & { name: string; }'."),
    "Property 'nmae' does not exist on type '{ name: string; }'.",
  );
});

test("long messages are truncated", () => {
  const members = Array.from({ length: 60 }, (_, index) => `field${index}: string`).join("; ");
  const clean = sanitizeMessage(`Property 'x' does not exist on type '{ ${members} }'.`);
  assert.ok(clean.length <= 200, `${clean.length}`);
  assert.ok(clean.endsWith("…"), clean);
});
