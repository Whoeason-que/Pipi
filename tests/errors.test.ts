import assert from "node:assert/strict";
import test from "node:test";
import { errorMessage } from "../src/errors.ts";

test("errorMessage normalizes Error, string and host-shaped messages", () => {
  assert.equal(errorMessage(new Error("boom")), "boom");
  assert.equal(errorMessage(" plain "), " plain ");
  assert.equal(errorMessage({ message: "host failure" }), "host failure");
  assert.equal(errorMessage({ message: "  " }), "Unknown error");
  assert.equal(errorMessage({ code: 42 }), "Unknown error");
  assert.equal(errorMessage(null, "fallback"), "fallback");
});
