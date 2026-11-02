import { useCallback, useId, useMemo, useRef, useState } from "react";
import { useI18n } from "../i18n";
import { IconClose, IconSearch } from "../icons";
import { useDialogFocus } from "../components/use-dialog-focus";
import { filterQuickItems, type QuickItem, type QuickTarget } from "./navigation";

export default function CommandPalette({ items, onChoose, onClose }: {
  items: QuickItem[]; onChoose: (target: QuickTarget) => void; onClose: () => void;
}) {
  const { t } = useI18n();
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const results = useMemo(() => filterQuickItems(items, query), [items, query]);
  const selected = Math.min(index, Math.max(0, results.length - 1));
  const root = useRef<HTMLDivElement>(null);
  const close = useCallback(onClose, [onClose]);
  const id = useId();
  useDialogFocus(root, close);
  const choose = (item: QuickItem) => { onClose(); onChoose(item.target); };
  return <div className="modal-backdrop palette-backdrop" onClick={onClose}>
    <div ref={root} className="command-palette" role="dialog" aria-modal="true" aria-label={t("app.quickActions")}
      onClick={event => event.stopPropagation()}>
      <div className="palette-input"><IconSearch /><input value={query} placeholder={t("app.quickSearchPlaceholder")}
        aria-label={t("app.quickSearchLabel")} role="combobox" aria-autocomplete="list" aria-expanded="true" aria-controls={id}
        aria-activedescendant={results.length ? `${id}-${selected}` : undefined}
        onChange={event => { setQuery(event.target.value); setIndex(0); }} onKeyDown={event => {
          if (event.nativeEvent.isComposing) return;
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            const next = results.length ? (selected + (event.key === "ArrowDown" ? 1 : -1) + results.length) % results.length : 0;
            setIndex(next); root.current?.querySelector(`#${CSS.escape(`${id}-${next}`)}`)?.scrollIntoView({ block: "nearest" });
          }
          if (event.key === "Enter" && results[selected]) { event.preventDefault(); choose(results[selected]); }
        }} /><button type="button" className="icon-btn" aria-label={t("app.closeQuickActions")} onClick={onClose}><IconClose /></button></div>
      <div className="palette-results" id={id} role="listbox" aria-label={t("app.quickResults")}>
        {results.map((item, position) => <div key={item.id} id={`${id}-${position}`} role="option"
          aria-selected={selected === position} className={`palette-item${selected === position ? " selected" : ""}`}
          onMouseEnter={() => setIndex(position)} onClick={() => choose(item)}>
          <span className="caption palette-kind">{item.target.kind === "agent" ? "Agent" : item.target.kind === "chat" ? t("app.kindSession") : t("app.kindAction")}</span>
          <span><b>{item.label}</b><small>{item.detail}</small></span><span className="palette-return">↵</span>
        </div>)}
        {!results.length && <div className="palette-empty">{t("app.noQuickResults")}</div>}
      </div>
      <div className="caption palette-footer"><span>{t("app.selectHint")}</span><span>{t("app.openHint")}</span><span>{t("app.closeHint")}</span></div>
    </div>
  </div>;
}
