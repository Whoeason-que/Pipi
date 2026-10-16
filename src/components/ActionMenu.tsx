import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { IconMore } from "../icons";

export interface MenuAction { label: string; run: () => void; danger?: boolean }

export default function ActionMenu({ label, actions }: { label: string; actions: MenuAction[] }) {
  const [position, setPosition] = useState<{ top: number; left: number } | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const close = useCallback(() => { setPosition(null); trigger.current?.focus(); }, []);
  useEffect(() => {
    if (!position) return;
    menu.current?.querySelector<HTMLButtonElement>("button")?.focus();
    const outside = (event: PointerEvent) => {
      if (!menu.current?.contains(event.target as Node) && !trigger.current?.contains(event.target as Node)) setPosition(null);
    };
    const resize = () => setPosition(null);
    document.addEventListener("pointerdown", outside);
    window.addEventListener("resize", resize);
    return () => { document.removeEventListener("pointerdown", outside); window.removeEventListener("resize", resize); };
  }, [position]);
  return <>
    <button ref={trigger} type="button" className="icon-btn more-trigger" aria-label={label}
      title={label} aria-haspopup="menu" aria-expanded={Boolean(position)} onClick={() => {
        if (position) { close(); return; }
        const rect = trigger.current!.getBoundingClientRect();
        setPosition({ top: Math.min(rect.bottom + 4, window.innerHeight - actions.length * 38 - 20),
          left: Math.max(8, Math.min(rect.right - 176, window.innerWidth - 184)) });
      }}><IconMore /></button>
    {position && createPortal(<div ref={menu} className="action-menu" role="menu" aria-label={label}
      style={position} onKeyDown={event => {
        const buttons = [...menu.current!.querySelectorAll<HTMLButtonElement>("button")];
        const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
        if (event.key === "Escape" || event.key === "Tab") {
          if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); }
          close();
        }
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault(); buttons[(index + (event.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length]?.focus();
        }
      }}>{actions.map(action => <button key={action.label} type="button" role="menuitem"
        className={action.danger ? "danger" : ""} onClick={() => { close(); action.run(); }}>{action.label}</button>)}</div>, document.body)}
  </>;
}
