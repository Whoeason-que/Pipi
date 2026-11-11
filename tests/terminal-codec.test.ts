import assert from "node:assert/strict";
import test from "node:test";
import { base64ToBytes, bytesToBase64 } from "../src/terminal/base64.ts";

test("terminal base64 keeps arbitrary PTY bytes intact", () => {
  const bytes = new Uint8Array([0, 1, 2, 27, 91, 65, 127, 128, 200, 255]);
  assert.deepEqual([...base64ToBytes(bytesToBase64(bytes))], [...bytes]);
  assert.equal(bytesToBase64(new Uint8Array()), "");
  assert.equal(base64ToBytes("").length, 0);
});

test("terminal base64 survives chunk boundaries at 32 KiB steps", () => {
  const bytes = new Uint8Array(0x8000 * 2 + 7);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = index % 251;
  }
  const decoded = base64ToBytes(bytesToBase64(bytes));
  assert.equal(decoded.length, bytes.length);
  assert.deepEqual([...decoded.subarray(0, 64)], [...bytes.subarray(0, 64)]);
  assert.deepEqual([...decoded.subarray(-64)], [...bytes.subarray(-64)]);
});
