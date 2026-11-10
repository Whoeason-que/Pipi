import type { Webview } from "@tauri-apps/api/webview";

/** Lives beyond a mounted inspector so background sessions retain their page. */
export interface BrowserSession {
  view: Webview;
  ready: Promise<void>;
  address: string;
  generation: number;
  pending: Promise<void>;
}

const browsers = new Map<string, BrowserSession>();
const key = (agent: string, session: string) => JSON.stringify([agent, session]);

export function browserSession(agent: string, session: string): BrowserSession | undefined {
  return browsers.get(key(agent, session));
}

export function setBrowserSession(agent: string, session: string, browser: BrowserSession): void {
  browsers.set(key(agent, session), browser);
}

export function removeBrowserSession(agent: string, session: string, browser: BrowserSession): void {
  if (browsers.get(key(agent, session)) === browser) browsers.delete(key(agent, session));
}

export function forgetBrowserSession(agent: string, session: string): void {
  const browser = browserSession(agent, session);
  if (!browser) return;
  removeBrowserSession(agent, session, browser);
  browser.generation++;
  void browser.ready.then(() => browser.pending).then(() => browser.view.close()).catch(() => {});
}

export function forgetAgentBrowsers(agent: string): void {
  for (const encoded of browsers.keys()) {
    const [owner, session] = JSON.parse(encoded) as [string, string];
    if (owner === agent) forgetBrowserSession(owner, session);
  }
}

export function rekeyBrowserSession(agent: string, previous: string, next: string): void {
  if (previous === next) return;
  const oldKey = key(agent, previous), newKey = key(agent, next);
  const browser = browsers.get(oldKey);
  if (!browser || browsers.has(newKey)) return;
  browsers.delete(oldKey);
  browsers.set(newKey, browser);
}
