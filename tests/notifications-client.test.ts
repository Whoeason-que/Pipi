import assert from "node:assert/strict";
import test from "node:test";
import { disableWebPush, enableWebPush } from "../src/notifications-client.ts";

async function environment(run: (state: {
  calls: string[];
  permission: string;
  subscribed: boolean;
  rejectCommand: string;
  registered: boolean;
}) => Promise<void>) {
  const state = { calls: [] as string[], permission: "granted", subscribed: false, rejectCommand: "", registered: true };
  const subscription = {
    endpoint: "https://web.push.apple.com/test",
    options: {},
    toJSON: () => ({ keys: { p256dh: "public", auth: "auth" } }),
    unsubscribe: async () => { state.calls.push("unsubscribe"); state.subscribed = false; return true; },
  };
  const registration = { pushManager: {
    getSubscription: async () => state.subscribed ? subscription : null,
    subscribe: async () => { state.calls.push("subscribe"); state.subscribed = true; return subscription; },
  } };
  const Notification = { requestPermission: () => { state.calls.push("permission"); return Promise.resolve(state.permission); } };
  const overrides: Record<string, unknown> = {
    window: { isSecureContext: true, PushManager: {}, Notification, __PIPI_DEV_PLATFORM__: {
      invoke: async (name: string) => {
        state.calls.push(name);
        if (name === state.rejectCommand) throw new Error("offline");
        if (name === "web_push_public_key") return "BAECAw";
        if (name === "push_subscription_status") return state.registered;
      },
    } },
    Notification,
    navigator: { serviceWorker: {
      register: async () => { state.calls.push("worker"); return registration; },
      ready: Promise.resolve(registration),
      getRegistration: async () => registration,
    } },
    location: { origin: "https://pipi.test" },
  };
  const previous = new Map(Object.keys(overrides).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  try {
    for (const [key, value] of Object.entries(overrides)) Object.defineProperty(globalThis, key, { configurable: true, writable: true, value });
    await run(state);
  } finally {
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
}

test("permission is requested in the click stack and denial creates no subscription or server key", async () => {
  await environment(async state => {
    state.permission = "denied";
    const result = enableWebPush();
    assert.deepEqual(state.calls, ["permission"]);
    await assert.rejects(result, /permission was not granted/);
    assert.equal(state.subscribed, false);
    assert.deepEqual(state.calls, ["permission"]);
  });
});

test("failed first registration rolls back only the newly created browser subscription", async () => {
  await environment(async state => {
    state.rejectCommand = "register_push_subscription";
    await assert.rejects(enableWebPush(), /offline/);
    assert.equal(state.subscribed, false);
    assert.ok(state.calls.includes("unsubscribe"));
  });
  await environment(async state => {
    state.subscribed = true;
    state.rejectCommand = "register_push_subscription";
    await assert.rejects(enableWebPush(), /offline/);
    assert.equal(state.subscribed, true);
    assert.ok(!state.calls.includes("unsubscribe"));
  });
});

test("failed device removal preserves its browser subscription for retry", async () => {
  await environment(async state => {
    state.subscribed = true;
    state.rejectCommand = "remove_push_subscription";
    await assert.rejects(disableWebPush(), /offline/);
    assert.equal(state.subscribed, true);
    assert.ok(!state.calls.includes("unsubscribe"));
  });
});

test("reconnect replaces an expired subscription before registering the new endpoint", async () => {
  await environment(async state => {
    state.subscribed = true;
    state.registered = false;
    await enableWebPush();
    assert.equal(state.subscribed, true);
    assert.ok(state.calls.indexOf("unsubscribe") < state.calls.indexOf("subscribe"));
    assert.ok(state.calls.indexOf("subscribe") < state.calls.indexOf("register_push_subscription"));
  });
});
