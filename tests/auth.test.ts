import assert from "node:assert/strict";
import test from "node:test";
import {
  getStoredToken,
  setStoredToken,
  getAuthStatus,
  loginWithToken,
  logout,
  onAuthRequired,
  invoke,
} from "../src/platform.ts";

// Setup browser globals for test environment
class MockLocalStorage {
  private store = new Map<string, string>();
  getItem(key: string) {
    return this.store.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.store.set(key, value);
  }
  removeItem(key: string) {
    this.store.delete(key);
  }
  clear() {
    this.store.clear();
  }
}

const mockStorage = new MockLocalStorage();
(globalThis as unknown as { localStorage: typeof mockStorage }).localStorage = mockStorage;
(globalThis as unknown as { window: unknown }).window = {
  location: { search: "", origin: "http://localhost:1421", href: "http://localhost:1421" },
  history: { replaceState: () => {} },
  addEventListener: () => {},
  removeEventListener: () => {},
  setTimeout: setTimeout,
  clearTimeout: clearTimeout,
};

test("token storage sets, gets, and removes token", () => {
  mockStorage.clear();
  assert.equal(getStoredToken(), null);

  setStoredToken("test-token-123");
  assert.equal(getStoredToken(), "test-token-123");

  setStoredToken(null);
  assert.equal(getStoredToken(), null);
});

test("onAuthRequired notifies subscribers", async () => {
  let called = 0;
  const unsubscribe = onAuthRequired(() => {
    called += 1;
  });

  // Trigger through logout
  await logout();
  assert.equal(called, 1);

  unsubscribe();
  await logout();
  assert.equal(called, 1);
});

test("getAuthStatus fetches status from server", async () => {
  const originalFetch = globalThis.fetch;
  try {
    globalThis.fetch = async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.endsWith("/api/auth/status")) {
        return new Response(JSON.stringify({ authRequired: true, authenticated: false }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      return new Response(null, { status: 404 });
    };

    const status = await getAuthStatus();
    assert.deepEqual(status, { authRequired: true, authenticated: false });
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("loginWithToken handles success and failure", async () => {
  mockStorage.clear();
  const originalFetch = globalThis.fetch;
  try {
    globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body || "{}"));
      if (body.token === "valid-secret") {
        return new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      return new Response(JSON.stringify({ ok: false, error: "Token 错误，请核对后重试" }), {
        status: 401,
        headers: { "Content-Type": "application/json" },
      });
    };

    // Test failure
    const failResult = await loginWithToken("wrong-token");
    assert.equal(failResult.ok, false);
    assert.equal(failResult.error, "Token 错误，请核对后重试");
    assert.equal(getStoredToken(), null);

    // Test success
    const successResult = await loginWithToken("valid-secret");
    assert.equal(successResult.ok, true);
    assert.equal(getStoredToken(), "valid-secret");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("webInvoke triggers onAuthRequired on 401 error", async () => {
  let authRequiredTriggered = false;
  const unsubscribe = onAuthRequired(() => {
    authRequiredTriggered = true;
  });

  const originalFetch = globalThis.fetch;
  try {
    globalThis.fetch = async () => {
      return new Response(JSON.stringify({ error: "需要 PIPI_AUTH_TOKEN" }), {
        status: 401,
        headers: { "Content-Type": "application/json" },
      });
    };

    await assert.rejects(
      async () => {
        await invoke("list_agents");
      },
      (err: unknown) => {
        return (
          err instanceof Error &&
          (err as { isAuthError?: boolean }).isAuthError === true
        );
      },
    );

    assert.equal(authRequiredTriggered, true);
  } finally {
    unsubscribe();
    globalThis.fetch = originalFetch;
  }
});
