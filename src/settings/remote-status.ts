import type { RemoteAccessStatus } from "../types.ts";

export function remoteAccessPresentation(status: RemoteAccessStatus) {
  const active = status.localRunning || status.tunnelRunning;
  const state = status.localRunning && status.tunnelConnected ? "running"
    : status.tunnelRunning ? "connecting"
    : status.localRunning ? "localOnly" : "stopped";
  return { active, state };
}
