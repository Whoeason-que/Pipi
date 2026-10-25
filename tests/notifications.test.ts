import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { decodeApplicationServerKey, notificationTarget, notificationTargetFromUrl } from "../src/notifications.ts";

test("notification navigation accepts identities and rejects paths or arbitrary URLs", () => {
  assert.deepEqual(notificationTarget({ agentName: "中文 Agent", sessionId: "1758000000000-abc123" }), { agentName: "中文 Agent", sessionId: "1758000000000-abc123" });
  for (const sessionId of ["../outside", "https://evil.test/", "", "a.b"]) {
    assert.equal(notificationTarget({ agentName: "a", sessionId }), null);
  }
  const url = new URL("https://pipi.test/?notificationAgent=a%20b&notificationSession=s-1");
  assert.deepEqual(notificationTargetFromUrl(url), { agentName: "a b", sessionId: "s-1" });
  assert.equal(notificationTargetFromUrl(new URL("https://pipi.test/")), null);
});

test("VAPID public keys are decoded as URL safe base64", () => {
  const bytes = Uint8Array.of(4, 255, 254, 253, 0, 1);
  const encoded = Buffer.from(bytes).toString("base64url");
  assert.deepEqual(decodeApplicationServerKey(encoded), bytes);
});

function worker(existing = false) {
  const handlers = new Map<string, (event: unknown) => void>();
  const shown: Array<{ title: string; options: { data: unknown } }> = [];
  const opened: string[] = [];
  const messages: unknown[] = [];
  let focused = 0;
  const client = { url: "https://pipi.test/", postMessage: (data: unknown) => messages.push(data), focus: async () => { focused++; } };
  const self = {
    location: { origin: "https://pipi.test" },
    addEventListener: (type: string, handler: (event: unknown) => void) => handlers.set(type, handler),
    registration: { showNotification: async (title: string, options: { data: unknown }) => { shown.push({ title, options }); } },
    clients: { matchAll: async () => existing ? [client] : [], openWindow: async (url: string) => { opened.push(url); } },
  };
  vm.runInNewContext(readFileSync(new URL("../public/sw.js", import.meta.url), "utf8"), { self, URL });
  const dispatch = async (type: string, data: Record<string, unknown>) => {
    let pending: Promise<unknown> | undefined;
    handlers.get(type)!({ ...data, waitUntil: (promise: Promise<unknown>) => { pending = promise; } });
    await pending;
  };
  return { handlers, shown, opened, messages, focused: () => focused, dispatch };
}

test("push displays a visible notification even for malformed data and does not cache API responses", async () => {
  const sw = worker();
  for (const value of [null, "text", {}]) await sw.dispatch("push", { data: { json: () => value } });
  await sw.dispatch("push", { data: { json: () => { throw new Error("invalid json"); } } });
  assert.equal(sw.shown.length, 4);
  assert.ok(sw.shown.every(item => item.title === "Pipi"));
  assert.equal(sw.handlers.has("fetch"), false);
});

test("notification click cold starts at the local session URL and never trusts a payload URL", async () => {
  const sw = worker();
  await sw.dispatch("notificationclick", { notification: { data: { agentName: "a b", sessionId: "s-1", url: "https://evil.test/" }, close() {} } });
  const target = new URL(sw.opened[0]);
  assert.equal(target.origin, "https://pipi.test");
  assert.deepEqual(notificationTargetFromUrl(target), { agentName: "a b", sessionId: "s-1" });
});

test("notification click focuses an existing client and sends the session identity", async () => {
  const sw = worker(true);
  await sw.dispatch("notificationclick", { notification: { data: { agentName: "a", sessionId: "s" }, close() {} } });
  assert.equal(sw.focused(), 1);
  assert.equal(sw.opened.length, 0);
  assert.equal((sw.messages[0] as { type: string }).type, "pipi-notification-open");
});
