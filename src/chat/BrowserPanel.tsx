import { useEffect, useRef, useState, type FormEvent } from "react";
import { Webview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalPosition, LogicalSize } from "@tauri-apps/api/dpi";
import { useI18n } from "../i18n";
import { invoke } from "../platform";
import { browserSession, removeBrowserSession, setBrowserSession, type BrowserSession } from "./browser-sessions";

/** The inspector attaches to a session-owned view; unmount only hides it. */
export default function BrowserPanel({ active, agentName, sessionId, ensureSession }: {
  active: boolean; agentName: string; sessionId: string | null; ensureSession?: () => Promise<string>;
}) {
  const { t } = useI18n();
  const viewport = useRef<HTMLDivElement>(null);
  const addressInput = useRef<HTMLInputElement>(null);
  const activeRef = useRef(active);
  const syncRef = useRef<() => void>(() => {});
  const opening = useRef(false);
  activeRef.current = active;
  const [address, setAddress] = useState(() => sessionId ? browserSession(agentName, sessionId)?.address ?? "" : "");
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState("");
  const [navigation, setNavigation] = useState({ canGoBack: false, canGoForward: false, loading: false });

  const open = async (event: FormEvent) => {
    event.preventDefault();
    if (opening.current) return;
    opening.current = true;
    try {
      const value = address.trim();
      const scheme = /^[a-z][a-z\d+.-]*:\/\//i.test(value);
      const local = /^(localhost|127\.0\.0\.1|\[::1\])(?=[:/]|$)/i.test(value);
      const url = new URL(scheme ? value : `${local ? "http" : "https"}://${value}`);
      if (!value || !["http:", "https:"].includes(url.protocol) || url.username || url.password) throw new Error(t("inspector.browserInvalid"));
      const id = sessionId ?? await ensureSession?.();
      if (!id || !viewport.current) throw new Error("Session identity is not ready yet.");
      try {
        const state = await invoke("browser_view", { agentName, sessionId: id, action: "navigate", url: url.href });
        const view = await Webview.getByLabel(state.label);
        if (!view) throw new Error("Browser view closed during navigation.");
        let record = browserSession(agentName, id);
        if (!record) {
          record = { view, ready: Promise.resolve(), pending: Promise.resolve(), address: state.url, generation: 0 };
          setBrowserSession(agentName, id, record);
          setRevision(n => n + 1);
        }
        record.address = state.url || url.href;
        setAddress(record.address);
        setError("");
        return;
      } catch (reason) {
        if (!String(reason).includes("Host browser creation is currently available on Linux only")) throw reason;
      }
      const previous = browserSession(agentName, id);
      if (previous) throw new Error("Browser controls are currently available on Linux only.");
      const hex = (input: string) => Array.from(new TextEncoder().encode(input), b => b.toString(16).padStart(2, "0")).join("");
      const rect = viewport.current.getBoundingClientRect();
      const view = new Webview(getCurrentWindow(), `browser-${hex(agentName)}--${hex(id)}--${crypto.randomUUID()}`, {
        url: url.href, x: rect.left, y: rect.top, width: Math.max(1, rect.width), height: Math.max(1, rect.height), focus: false,
      });
      const ready = new Promise<void>((resolve, reject) => {
        void view.once("tauri://created", () => resolve());
        void view.once<string>("tauri://error", event => reject(new Error(String(event.payload))));
      });
      const record: BrowserSession = { view, ready, address: url.href, generation: 0, pending: Promise.resolve() };
      setBrowserSession(agentName, id, record);
      setAddress(url.href);
      setError("");
      setRevision(n => n + 1);
      try { await ready; }
      catch (reason) { removeBrowserSession(agentName, id, record); setRevision(n => n + 1); throw reason; }
    } catch (reason) { setError(String(reason)); }
    finally { opening.current = false; }
  };

  useEffect(() => {
    const element = viewport.current;
    const record = sessionId && browserSession(agentName, sessionId);
    if (!sessionId || !element || !record) return;
    setAddress(record.address);
    let disposed = false, scheduled = 0;
    const generation = ++record.generation;
    let syncQueued = false, needsSync = false;
    let lastLayout: string | null = null;
    const modalOpen = () => Boolean(document.querySelector('[aria-modal="true"], [data-state="open"][role="dialog"]'));
    const enqueue = (operation: () => Promise<void>) => {
      record.pending = record.pending.then(async () => { await record.ready; await operation(); })
        .catch(reason => { if (!disposed) setError(String(reason)); });
    };
    const sync = () => {
      needsSync = true;
      cancelAnimationFrame(scheduled);
      scheduled = requestAnimationFrame(() => {
        if (syncQueued) return;
        syncQueued = true;
        enqueue(async () => {
          try {
            while (needsSync && !disposed && generation === record.generation) {
              needsSync = false;
              const bounds = element.getBoundingClientRect();
              const visible = activeRef.current && !modalOpen() && bounds.width >= 1 && bounds.height >= 1;
              const layout = visible ? `${Math.round(bounds.left)},${Math.round(bounds.top)},${Math.round(bounds.width)},${Math.round(bounds.height)}` : "hidden";
              if (layout === lastLayout) continue;
              if (!visible) {
                await record.view.hide();
                lastLayout = layout;
                continue;
              }
              try {
                await invoke("browser_view", { agentName, sessionId, action: "layout", bounds: {
                  x: Math.round(bounds.left), y: Math.round(bounds.top),
                  width: Math.max(1, Math.round(bounds.width)), height: Math.max(1, Math.round(bounds.height)),
                } });
              } catch (reason) {
                if (!String(reason).includes("Native browser layout is not required")) throw reason;
                await record.view.setPosition(new LogicalPosition(bounds.left, bounds.top));
                await record.view.setSize(new LogicalSize(bounds.width, bounds.height));
              }
              if (!disposed && generation === record.generation && activeRef.current && !modalOpen()) {
                await record.view.show();
                lastLayout = layout;
              }
            }
          } finally { syncQueued = false; }
        });
      });
    };
    syncRef.current = sync;
    const resize = new ResizeObserver(sync);
    resize.observe(element);
    // A DOM change can move the viewport without resizing it. Keep checking
    // its position, but coalesce checks and avoid repeating unchanged layouts.
    const mutations = new MutationObserver(sync);
    mutations.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["aria-modal", "data-state"] });
    window.addEventListener("resize", sync);
    window.addEventListener("scroll", sync, true);
    sync();
    return () => {
      disposed = true;
      cancelAnimationFrame(scheduled);
      resize.disconnect(); mutations.disconnect();
      window.removeEventListener("resize", sync); window.removeEventListener("scroll", sync, true);
      syncRef.current = () => {};
      enqueue(async () => { if (generation === record.generation) await record.view.hide(); });
    };
  }, [agentName, sessionId, revision]);

  useEffect(() => { syncRef.current(); }, [active]);
  const hasBrowser = !!sessionId && !!browserSession(agentName, sessionId);
  useEffect(() => {
    if (!sessionId || hasBrowser || !active) return;
    let disposed = false;
    let pending = false;
    const attach = async () => {
      if (pending) return;
      pending = true;
      try {
        const state = await invoke("browser_view", { agentName, sessionId, action: "state" });
        const view = await Webview.getByLabel(state.label);
        if (disposed || !view || browserSession(agentName, sessionId)) return;
        setBrowserSession(agentName, sessionId, { view, ready: Promise.resolve(), pending: Promise.resolve(), address: state.url, generation: 0 });
        setRevision(n => n + 1);
      } catch { /* A session without a browser is normal. */ }
      finally { pending = false; }
    };
    void attach();
    // The agent may create a background view after the panel has mounted.
    const timer = window.setInterval(() => void attach(), 1000);
    return () => { disposed = true; clearInterval(timer); };
  }, [active, agentName, sessionId, hasBrowser]);
  useEffect(() => {
    if (!sessionId || !hasBrowser || !active) return;
    let disposed = false;
    let pending = false;
    const update = async () => {
      if (pending) return;
      pending = true;
      try {
        const state = await invoke("browser_view", { agentName, sessionId, action: "state" });
        if (disposed) return;
        const record = browserSession(agentName, sessionId);
        if (record && state.url) record.address = state.url;
        if (state.url && document.activeElement !== addressInput.current) setAddress(state.url);
        setNavigation(previous => previous.canGoBack === state.canGoBack && previous.canGoForward === state.canGoForward && previous.loading === state.loading
          ? previous : { canGoBack: state.canGoBack, canGoForward: state.canGoForward, loading: state.loading });
      } catch { /* The view can be closing or the host can lack native controls. */ }
      finally { pending = false; }
    };
    void update();
    const timer = window.setInterval(() => void update(), 1000);
    return () => { disposed = true; clearInterval(timer); };
  }, [active, agentName, sessionId, hasBrowser, revision]);
  const control = async (action: string) => {
    if (!sessionId) return;
    try { await invoke("browser_view", { agentName, sessionId, action }); }
    catch (reason) { setError(String(reason)); }
  };
  const close = async () => {
    if (!sessionId) return;
    const record = browserSession(agentName, sessionId);
    if (!record) return;
    record.generation++;
    try {
      await record.ready; await record.pending; await record.view.close();
      removeBrowserSession(agentName, sessionId, record);
      setRevision(n => n + 1);
    } catch (reason) { setError(String(reason)); }
  };
  return <div className="browser-panel">
    <form className="browser-toolbar" aria-busy={navigation.loading} onSubmit={event => void open(event)}>
      <button type="button" aria-label={t("inspector.browserBack")} disabled={!hasBrowser || !navigation.canGoBack} onClick={() => void control("back")}>‹</button>
      <button type="button" aria-label={t("inspector.browserForward")} disabled={!hasBrowser || !navigation.canGoForward} onClick={() => void control("forward")}>›</button>
      <button type="button" aria-label={t("inspector.browserRefresh")} disabled={!hasBrowser} onClick={() => void control("reload")}>↻</button>
      <input ref={addressInput} aria-label={t("inspector.browserAddress")} placeholder={t("inspector.browserPlaceholder")}
        value={address} onChange={event => setAddress(event.target.value)} />
      <button type="submit">{t("inspector.browserOpen")}</button>
      <button type="button" disabled={!hasBrowser} onClick={() => void close()}>{t("inspector.browserClose")}</button>
    </form>
    {error && <p className="file-notice" role="alert">{error}</p>}
    <div className="browser-viewport" ref={viewport}>
      {!hasBrowser && <p className="browser-empty">{t("inspector.browserEmpty")}</p>}
    </div>
  </div>;
}
