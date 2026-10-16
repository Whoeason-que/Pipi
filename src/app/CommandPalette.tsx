import { useCallback, useId, useMemo, useRef, useState } from "react";
import { IconClose, IconSearch } from "../icons";
import { useDialogFocus } from "../components/use-dialog-focus";
import { filterQuickItems, type QuickItem, type QuickTarget } from "./navigation";

export default function CommandPalette({ items, onChoose, onClose }: {
  items: QuickItem[]; onChoose: (target: QuickTarget) => void; onClose: () => void;
}) {
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
    <div ref={root} className="command-palette" role="dialog" aria-modal="true" aria-label="快捷操作"
      onClick={event => event.stopPropagation()}>
      <div className="palette-input"><IconSearch /><input value={query} placeholder="搜索 Agent、会话或操作…"
        aria-label="搜索快捷操作" role="combobox" aria-autocomplete="list" aria-expanded="true" aria-controls={id}
        aria-activedescendant={results.length ? `${id}-${selected}` : undefined}
        onChange={event => { setQuery(event.target.value); setIndex(0); }} onKeyDown={event => {
          if (event.nativeEvent.isComposing) return;
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            const next = results.length ? (selected + (event.key === "ArrowDown" ? 1 : -1) + results.length) % results.length : 0;
            setIndex(next); root.current?.querySelector(`#${CSS.escape(`${id}-${next}`)}`)?.scrollIntoView({ block: "nearest" });
          }
          if (event.key === "Enter" && results[selected]) { event.preventDefault(); choose(results[selected]); }
        }} /><button type="button" className="icon-btn" aria-label="关闭快捷操作" onClick={onClose}><IconClose /></button></div>
      <div className="palette-results" id={id} role="listbox" aria-label="快捷操作结果">
        {results.map((item, position) => <div key={item.id} id={`${id}-${position}`} role="option"
          aria-selected={selected === position} className={`palette-item${selected === position ? " selected" : ""}`}
          onMouseEnter={() => setIndex(position)} onClick={() => choose(item)}>
          <span className="palette-kind">{item.target.kind === "agent" ? "Agent" : item.target.kind === "chat" ? "会话" : "操作"}</span>
          <span><b>{item.label}</b><small>{item.detail}</small></span><span className="palette-return">↵</span>
        </div>)}
        {!results.length && <div className="palette-empty">没有匹配结果，试试其他关键词。</div>}
      </div>
      <div className="palette-footer"><span>↑ ↓ 选择</span><span>Enter 打开</span><span>Esc 关闭</span></div>
    </div>
  </div>;
}
