import { useEffect, useState } from "react";
import { errorMessage } from "../errors";
import { selectIconTheme, type IconThemeList } from "../files/icon-theme";
import { useI18n } from "../i18n";
import { invoke } from "../platform";

export default function IconThemeSettings() {
  const { t } = useI18n();
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
    <label className="label" htmlFor="file-icon-theme">{t("settings.fileIconTheme")}</label>
    <div className="icon-theme-controls">
      <select id="file-icon-theme" className="text-input" disabled={busy || !list} value={list?.selected ?? ""} onChange={event => void choose(event.target.value)}>
        <option value="">{t("settings.pipiDefault")}</option>
        {list?.selected && !list.themes.some(theme => theme.id === list.selected) && <option value={list.selected}>{t("settings.unavailableTheme")}</option>}
        {list?.themes.map(theme => <option key={theme.id} value={theme.id}>{theme.label}</option>)}
      </select>
      <button type="button" className="btn" disabled={busy} onClick={() => void refresh()}>{t("common.refresh")}</button>
    </div>
    <span className="hint settings-block-note">{t("settings.fileIconThemeHint")}</span>
    {error && <p className="file-error" role="alert">{error}</p>}
  </section>;
}
