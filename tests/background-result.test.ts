import assert from "node:assert/strict";
import test from "node:test";
import type { MessageView } from "../src/chat-runtime.ts";
import {
  backgroundResultLabel,
  parseBackgroundResult,
} from "../src/chat/background-result.ts";

const userMessage = (content: string): MessageView =>
  ({ role: "user", content }) as MessageView;

test("parseBackgroundResult recognizes the host tag and splits fields from output", () => {
  const result = parseBackgroundResult(
    userMessage(
      [
        "<background task result>",
        "status: failed",
        "job: job-abc",
        "kind: shell",
        "command: cargo test",
        "exit code: 101",
        "error: test failed",
        "output:",
        "line one",
        "line two",
        "</background task result>",
      ].join("\n"),
    ),
  );
  assert.ok(result);
  assert.equal(result.status, "failed");
  assert.equal(result.jobId, "job-abc");
  assert.equal(result.command, "cargo test");
  assert.equal(result.exitCode, "101");
  assert.equal(result.error, "test failed");
  // 输出本身即使含冒号也不能被当成字段。
  assert.equal(result.output, "line one\nline two");
  assert.equal(backgroundResultLabel(result), "cargo test");
});

test("parseBackgroundResult ignores ordinary user messages and malformed tags", () => {
  assert.equal(parseBackgroundResult(userMessage("hello")), null);
  assert.equal(
    parseBackgroundResult(
      userMessage("<background task result>\nno status here\n</background task result>"),
    ),
    null,
  );
  assert.equal(
    parseBackgroundResult(
      ({ role: "assistant", content: "<background task result>\nstatus: completed\n</background task result>" }) as MessageView,
    ),
    null,
  );
});

test("parseBackgroundResult labels child Agent results by target", () => {
  const result = parseBackgroundResult(
    userMessage(
      [
        "<background task result>",
        "status: completed",
        "job: job-xyz",
        "kind: agent",
        "target agent: reviewer",
        "output:",
        "LGTM",
      ].join("\n") + "\n</background task result>",
    ),
  );
  assert.ok(result);
  assert.equal(backgroundResultLabel(result), "Agent reviewer");
  assert.equal(result.output, "LGTM");
});
