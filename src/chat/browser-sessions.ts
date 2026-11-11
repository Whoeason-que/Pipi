import type { Webview } from "@tauri-apps/api/webview";
import { sessionKey, sessionKeyPrefix } from "../session-key.ts";

/** Lives beyond a mounted inspector so background sessions retain their page. */
export interface BrowserSession {
  view: Webview;
  ready: Promise<void>;
  address: string;
  generation: number;
  pending: Promise<void>;
}

const browsers = new Map<string, BrowserSession>();

export function browserSession(agent: string, session: string): BrowserSession | undefined {
  return browsers.get(sessionKey(agent, session));
}

export function setBrowserSession(agent: string, session: string, browser: BrowserSession): void {
  browsers.set(sessionKey(agent, session), browser);
}

export function removeBrowserSession(agent: string, session: string, browser: BrowserSession): void {
  const mapKey = sessionKey(agent, session);
  if (browsers.get(mapKey) === browser) browsers.delete(mapKey);
}

export function forgetBrowserSession(agent: string, session: string): void {
  const browser = browserSession(agent, session);
  if (!browser) return;
  removeBrowserSession(agent, session, browser);
  browser.generation++;
  void browser.ready.then(() => browser.pending).then(() => browser.view.close()).catch(() => {});
}

export function forgetAgentBrowsers(agent: string): void {
  const prefix = sessionKeyPrefix(agent);
  for (const encoded of browsers.keys()) {
    if (encoded.startsWith(prefix)) forgetBrowserSession(agent, encoded.slice(prefix.length));
  }
}

export function rekeyBrowserSession(agent: string, previous: string, next: string): void {
  if (previous === next) return;
  const oldKey = sessionKey(agent, previous), newKey = sessionKey(agent, next);
  const browser = browsers.get(oldKey);
  if (!browser || browsers.has(newKey)) return;
  browsers.delete(oldKey);
  browsers.set(newKey, browser);
}
