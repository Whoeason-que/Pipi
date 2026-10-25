import assert from "node:assert/strict";
import test from "node:test";
import { errorMessage } from "../src/errors.ts";

test("errorMessage normalizes Error, string and host-shaped messages", () => {
  assert.equal(errorMessage(new Error("boom")), "boom");
  assert.equal(errorMessage(" plain "), " plain ");
  assert.equal(errorMessage({ message: "host failure" }), "host failure");
  assert.equal(errorMessage({ message: "  " }), "未知错误");
  assert.equal(errorMessage({ code: 42 }), "未知错误");
  assert.equal(errorMessage(null, "fallback"), "fallback");
});
