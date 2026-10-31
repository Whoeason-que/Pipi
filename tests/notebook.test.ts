import assert from "node:assert/strict";
import test from "node:test";
import { parsePlan } from "../src/chat/notebook-plan.ts";

test("parsePlan counts markdown checkboxes", () => {
  assert.deepEqual(parsePlan(""), { done: 0, total: 0 });
  assert.deepEqual(parsePlan("# Notes\n\nno checklist here"), { done: 0, total: 0 });
  assert.deepEqual(
    parsePlan("## Plan\n\n- [ ] one\n- [x] two\n* [X] three\n  - [ ] nested\n"),
    { done: 2, total: 4 },
  );
});

test("parsePlan ignores prose that only looks like a checkbox", () => {
  assert.deepEqual(parsePlan("Use `- [ ]` in the notebook, not in prose"), { done: 0, total: 0 });
  assert.deepEqual(parsePlan("-[]not a checkbox"), { done: 0, total: 0 });
});
