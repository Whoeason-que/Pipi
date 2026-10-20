import { useEffect, useRef, useState } from "react";
import { useI18n } from "../i18n";
import { invoke } from "../platform";
import type { RemoteAccessConfig, RemoteAccessStatus } from "../types";
import { remoteAccessPresentation } from "./remote-status";

export default function RemoteAccessPanel() {
  const { t } = useI18n();
  const [status, setStatus] = useState<RemoteAccessStatus | null>(null);
  const [config, setConfig] = useState<RemoteAccessConfig | null>(null);
  const [port, setPort] = useState("1421");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [token, setToken] = useState<string | null>(null);
  const [confirmRotate, setConfirmRotate] = useState(false);
  const initialized = useRef(false);

  useEffect(() => {
    if (busy) return;
    let active = true;
    const refresh = async () => {
      try {
        const next = await invoke("remote_access_status");
        if (!active) return;
        setStatus(next);
        if (!initialized.current) {
          initialized.current = true;
          setConfig(next.config);
          setPort(String(next.config.port));
        }
      } catch (reason) { if (active) setError(String(reason)); }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 2500);
    return () => { active = false; window.clearInterval(timer); };
  }, [busy]);

  const act = async (action: () => Promise<void>) => {
    if (busy) return;
    setBusy(true); setError(null); setNotice(null);
    try { await action(); }
    catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  };
  const copy = (value: string) => void act(async () => {
    await navigator.clipboard.writeText(value);
    setNotice(t("common.copied"));
  });

  if (!status || !config) return <section className="settings-block" role="status">
    {error ?? t("common.loading")}
  </section>;

  const dirty = port !== String(status.config.port)
    || config.startWithApp !== status.config.startWithApp || config.publicUrl !== status.config.publicUrl
    || config.tunnelTokenFile !== status.config.tunnelTokenFile;
  const presentation = remoteAccessPresentation(status);

  return <div className="remote-access-panel">
    <section className="settings-block">
      <div className="remote-status-row">
        <div>
          <span className={`badge ${presentation.state === "running" ? "ok" : "neutral"}`}>
            {t(`remoteAccess.${presentation.state}`)}
          </span>
          <p className="hint">{status.localRunning
            ? `${status.connectedClients} ${t("remoteAccess.clients")}` : t("remoteAccess.offHint")}</p>
          <p className="hint">{t("remoteAccess.localService")}: {t(status.localRunning ? "remoteAccess.on" : "remoteAccess.stopped")}
            {" · "}Cloudflare Tunnel: {t(status.tunnelConnected ? "remoteAccess.connected"
              : status.tunnelRunning ? "remoteAccess.connecting" : "remoteAccess.stopped")}</p>
        </div>
        <button className={presentation.active ? "btn" : "btn primary"} disabled={busy || (dirty && !presentation.active)}
          onClick={() => void act(async () => {
            setStatus(await invoke("set_remote_access_enabled", { enabled: !presentation.active }));
          })}>
          {t(presentation.active ? "remoteAccess.stop" : "remoteAccess.start")}
        </button>
      </div>
      {status.localUrl && <div className="remote-address-row">
        <span className="mono">{status.localUrl}</span>
        <button className="link" disabled={busy} onClick={() => copy(status.localUrl!)}>{t("common.copy")}</button>
      </div>}
      <p className="hint">{t("remoteAccess.lifecycle")}</p>
      {dirty && <p className="hint">{t("remoteAccess.saveFirst")}</p>}
    </section>

    <form className="settings-block remote-config" onSubmit={(event) => {
      event.preventDefault();
      void act(async () => {
        const parsed = Number(port);
        if (!/^\d+$/.test(port) || !Number.isInteger(parsed) || parsed < 1 || parsed > 65535)
          throw new Error(t("remoteAccess.invalidPort"));
        const next = await invoke("configure_remote_access", { config: {
          ...config, port: parsed, tunnelTokenFile: config.tunnelTokenFile.trim(), publicUrl: config.publicUrl.trim().replace(/\/$/, ""),
        } });
        setStatus(next); setConfig(next.config); setPort(String(next.config.port));
        setNotice(t("remoteAccess.saved"));
      });
    }}>
      <label className="field"><span className="label">{t("remoteAccess.port")}</span>
        <input type="number" inputMode="numeric" min="1" max="65535" required value={port}
          disabled={busy} onChange={(event) => setPort(event.target.value)} />
      </label>
      <label className="remote-checkbox"><input type="checkbox" checked={config.startWithApp}
        disabled={busy} onChange={(event) => setConfig({ ...config, startWithApp: event.target.checked })} />
        {t("remoteAccess.autoStart")}
      </label>
      <label className="field"><span className="label">{t("remoteAccess.publicUrl")}</span>
        <input type="url" placeholder="https://pipi.example.com" value={config.publicUrl}
          disabled={busy} onChange={(event) => setConfig({ ...config, publicUrl: event.target.value })} />
      </label>
      <label className="field"><span className="label">{t("remoteAccess.tunnelTokenFile")}</span>
        <input type="text" placeholder="/path/to/tunnel.token" value={config.tunnelTokenFile}
          disabled={busy} onChange={(event) => setConfig({ ...config, tunnelTokenFile: event.target.value })} />
      </label>
      <p className="hint">{t("remoteAccess.tunnelHint")}</p>
      <div className="remote-actions">
        <button type="submit" className="btn primary" disabled={busy || !dirty}>{t("common.save")}</button>
        {status.config.publicUrl && <button type="button" className="link" disabled={busy}
          onClick={() => copy(status.config.publicUrl)}>{t("remoteAccess.copyPublicUrl")}</button>}
      </div>
    </form>

    <section className="settings-block">
      <span className="label">{t("remoteAccess.token")}</span>
      <p className="hint">{t("remoteAccess.tokenHint")}</p>
      {token && <label className="field"><span className="sr-only">{t("remoteAccess.token")}</span>
        <input readOnly className="mono remote-token" value={token} onFocus={(event) => event.target.select()} />
      </label>}
      <div className="remote-actions">
        <button className="link" disabled={busy} onClick={() => {
          if (token) setToken(null);
          else void act(async () => setToken(await invoke("remote_access_token", { rotate: false })));
        }}>{t(token ? "remoteAccess.hideToken" : "remoteAccess.showToken")}</button>
        {token && <button className="link" disabled={busy} onClick={() => copy(token)}>{t("common.copy")}</button>}
        <button className="link danger" disabled={busy} onClick={() => setConfirmRotate(true)}>{t("remoteAccess.rotate")}</button>
      </div>
      {confirmRotate && <div className="remote-rotate-confirm" role="alert">
        <p>{t("remoteAccess.rotateConfirm")}</p>
        <div className="remote-actions">
          <button className="btn" disabled={busy} onClick={() => setConfirmRotate(false)}>{t("common.cancel")}</button>
          <button className="btn danger" disabled={busy} onClick={() => void act(async () => {
            setToken(null);
            setToken(await invoke("remote_access_token", { rotate: true }));
            setConfirmRotate(false); setNotice(t("remoteAccess.rotated"));
          })}>{t("common.confirm")}</button>
        </div>
      </div>}
    </section>
    {(error || status.lastError) && <p className="remote-error" role="alert">{error ?? status.lastError}</p>}
    {notice && <p className="hint" role="status">{notice}</p>}
  </div>;
}
