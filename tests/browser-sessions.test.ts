import assert from "node:assert/strict";
import test from "node:test";
import { browserSession, forgetBrowserSession, rekeyBrowserSession, removeBrowserSession, setBrowserSession, type BrowserSession } from "../src/chat/browser-sessions.ts";

test("browser identity follows a fork and remains isolated from the original session", () => {
  const first = {} as BrowserSession, other = {} as BrowserSession;
  setBrowserSession("a", "old", first);
  setBrowserSession("b", "old", other);
  rekeyBrowserSession("a", "old", "new");
  assert.equal(browserSession("a", "old"), undefined);
  assert.equal(browserSession("a", "new"), first);
  assert.equal(browserSession("b", "old"), other);
  removeBrowserSession("a", "new", other);
  assert.equal(browserSession("a", "new"), first);
  removeBrowserSession("a", "new", first);
  removeBrowserSession("b", "old", other);
});

test("forgetting a session closes its retained browser", async () => {
  let closed = false;
  const record = { generation: 0, ready: Promise.resolve(), pending: Promise.resolve(),
    view: { close: async () => { closed = true; } } } as unknown as BrowserSession;
  setBrowserSession("owner", "deleted", record);
  forgetBrowserSession("owner", "deleted");
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(browserSession("owner", "deleted"), undefined);
  assert.equal(record.generation, 1);
  assert.equal(closed, true);
});
