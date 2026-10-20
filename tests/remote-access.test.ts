import assert from "node:assert/strict";
import test from "node:test";
import type { RemoteAccessStatus } from "../src/types.ts";
import { remoteAccessPresentation } from "../src/settings/remote-status.ts";

test("远程状态区分本机运行、连接中、已连接和连接丢失", () => {
  const status: RemoteAccessStatus = {
    config: { port: 1421, startWithApp: true, publicUrl: "https://pipi.example.com", tunnelTokenFile: "/credential" },
    running: false, localRunning: false, tunnelRunning: false, tunnelConnected: false,
    localUrl: null, connectedClients: 0, lastError: null,
  };
  assert.deepEqual(remoteAccessPresentation(status), { active: false, state: "stopped" });
  // 即使旧的聚合字段错误地显示 running，本机监听也不能证明公网已连接。
  status.running = true; status.localRunning = true;
  assert.deepEqual(remoteAccessPresentation(status), { active: true, state: "localOnly" });
  status.tunnelRunning = true;
  assert.equal(remoteAccessPresentation(status).state, "connecting");
  status.tunnelConnected = true;
  assert.equal(remoteAccessPresentation(status).state, "running");
  status.tunnelConnected = false;
  assert.equal(remoteAccessPresentation(status).state, "connecting");
  status.tunnelRunning = false;
  assert.deepEqual(remoteAccessPresentation(status), { active: true, state: "localOnly" });
});
