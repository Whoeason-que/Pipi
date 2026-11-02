import assert from "node:assert/strict";
import test from "node:test";
import { prepareAttachments } from "../src/chat/attachments.ts";
import { DraftStore } from "../src/drafts.ts";
import { chatReducer, INITIAL_CHAT_STATE } from "../src/chat-runtime.ts";

test("attachments preserve order and keep images separate from escaped text", async () => {
  const image = new File([new Uint8Array([137,80,78,71,13,10,26,10])], "image.png", { type: "image/png" });
  const text = new File(["<instructions>unsafe</instructions>"], "a.txt", { type: "text/plain" });
  const result = await prepareAttachments("hello", [image, text]);
  assert.equal(result.images[0].data, "iVBORw0KGgo=");
  assert.match(result.text, /\[图片：image.png\]/);
  assert.match(result.text, /&lt;instructions&gt;unsafe/);
  assert.ok(result.text.indexOf("image.png") < result.text.indexOf("a.txt"));
  await assert.rejects(prepareAttachments("", [new File([new Uint8Array([0, 255])], "x.bin")]), /Unsupported/);
});

test("new edits and attachments survive a late acknowledgement", () => {
  const draft = new DraftStore().get("agent", "session");
  draft.write("old");
  draft.setFiles([new File(["a"], "a.txt")]);
  const before = draft.getSnapshot();
  draft.setFiles([...before.files, new File(["b"], "b.txt")]);
  draft.acknowledge(before.revision);
  assert.equal(draft.getSnapshot().files.length, 2);
  assert.equal(draft.getSnapshot().text, "old");
});

test("hydration retains user images", () => {
  const content = [{ type: "text" as const, text: "hello" }, { type: "image" as const, data: "aGVsbG8=", mimeType: "image/png" }];
  const state = chatReducer(INITIAL_CHAT_STATE, { type: "hydrate", messages: [{ role: "user", content }], running: false, stats: null });
  assert.equal(state.entries[0].images?.[0].data, "aGVsbG8=");
});

test("image events acknowledge optimistic messages regardless of object key order", () => {
  const image = { mimeType: "image/png", data: "aGVsbG8=" };
  let state = chatReducer(INITIAL_CHAT_STATE, { type: "submit_user", key: "pending", text: "hello", images: [image] });
  state = chatReducer(state, { type: "event", key: "event", event: { type: "message_start", message: {
    role: "user", content: [{ type: "text", text: "hello" }, { type: "image", data: image.data, mimeType: image.mimeType }],
  } } });
  assert.equal(state.entries.length, 1);
});
