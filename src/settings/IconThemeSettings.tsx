import { useEffect, useState } from "react";
import { errorMessage } from "../errors";
import { selectIconTheme, type IconThemeList } from "../files/icon-theme";
import { useI18n } from "../i18n";
import { invoke } from "../platform";

export default function IconThemeSettings() {
  const { locale } = useI18n();
  const en = locale === "en";
  const [list, setList] = useState<IconThemeList | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const refresh = async () => {
    setBusy(true); setError("");
    try { setList(await invoke("list_icon_themes")); } catch (reason) { setError(errorMessage(reason)); }
    finally { setBusy(false); }
  };
  useEffect(() => { void refresh(); }, []);
  const choose = async (id: string) => {
    setBusy(true); setError("");
    try { await selectIconTheme(id || null); setList(previous => previous && { ...previous, selected: id || null }); }
    catch (reason) { setError(errorMessage(reason)); }
    finally { setBusy(false); }
  };
  return <section className="surface settings-block">
    <label className="label" htmlFor="file-icon-theme">{en ? "File icon theme" : "文件图标主题"}</label>
    <div className="icon-theme-controls">
      <select id="file-icon-theme" className="text-input" disabled={busy || !list} value={list?.selected ?? ""} onChange={event => void choose(event.target.value)}>
        <option value="">{en ? "Pipi default" : "Pipi 默认"}</option>
        {list?.selected && !list.themes.some(theme => theme.id === list.selected) && <option value={list.selected}>{en ? "Unavailable theme" : "主题不可用"}</option>}
        {list?.themes.map(theme => <option key={theme.id} value={theme.id}>{theme.label}</option>)}
      </select>
      <button type="button" className="btn" disabled={busy} onClick={() => void refresh()}>{en ? "Refresh" : "刷新"}</button>
    </div>
    <span className="hint settings-block-note">{en ? "Uses file icon themes installed in VS Code on the Pipi host. Seti is selected by default when available; install other themes in VS Code and refresh this list." : "使用 Pipi 宿主上 VS Code 已安装的文件图标主题。可用时默认使用 Seti；安装其他主题后，点击刷新列表。"}</span>
    {error && <p className="file-error" role="alert">{error}</p>}
  </section>;
}
