import { useEffect, useRef, useState } from "react";
import { IconClock } from "../icons";
import DelayDial from "./DelayDial";

interface Props {
  value: string;
  en: boolean;
  onChange: (value: string) => void;
}

/** Delay selection is independent of sending; presets work without precise gestures. */
export default function DelayPicker({ value, en, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const [max, setMax] = useState(60);
  const [maxInput, setMaxInput] = useState("60");
  const root = useRef<HTMLDivElement>(null);
  const lastWheel = useRef(0);
  const setLimit = (limit: number) => {
    const next = Math.max(5, Math.min(360, Math.round(limit)));
    setMax(next);
    setMaxInput(String(next));
    if (Number(value) > next) onChange(String(next));
  };
  useEffect(() => {
    if (!open) return;
    const close = (event: globalThis.PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    const key = (event: KeyboardEvent) => { if (event.key === "Escape") setOpen(false); };
    document.addEventListener("pointerdown", close);
    document.addEventListener("keydown", key);
    return () => { document.removeEventListener("pointerdown", close); document.removeEventListener("keydown", key); };
  }, [open]);
  useEffect(() => {
    const element = root.current;
    if (!element || !open) return;
    const wheel = (event: WheelEvent) => {
      if (!(event.target instanceof Element) || !event.target.closest(".delay-dial-row") || event.ctrlKey || !(event.deltaY || event.deltaX)) return;
      event.preventDefault();
      const now = performance.now();
      if (now - lastWheel.current < 65) return;
      lastWheel.current = now;
      const steps = [5, 10, 15, 20, 25, 30, 45, 60, 90, 120, 180, 240, 300, 360];
      const direction = (event.deltaY || event.deltaX) < 0 ? 1 : -1;
      setLimit(event.shiftKey || event.altKey ? max + direction : direction > 0
        ? steps.find(step => step > max) ?? 360 : [...steps].reverse().find(step => step < max) ?? 5);
    };
    element.addEventListener("wheel", wheel, { passive: false });
    return () => element.removeEventListener("wheel", wheel);
  }, [open, max, value]);
  const selected = Number(value) > 0;
  return <div className="delay-picker" ref={root}>
    <button type="button" className={`delay-trigger${selected ? " active" : ""}`} onClick={() => setOpen(!open)} aria-expanded={open} aria-label={en ? `Set delay${selected ? `, ${value} minutes` : ""}` : `设置延迟${selected ? `，${value} 分钟` : ""}`} title={en ? "Set delay" : "设置延迟"}>
      <IconClock size={16} />{selected && <span>{value}{en ? "m" : "分"}</span>}
    </button>
    {open && <div className="delay-panel">
      <div className="delay-panel-title">{en ? "Send later" : "延迟发送"}</div>
      <div className="delay-presets">
        {[5, 15, 60].map(minutes => <button type="button" key={minutes} className={Number(value) === minutes ? "active" : ""} onClick={() => { onChange(String(minutes)); setOpen(false); }}>{minutes === 60 ? (en ? "1 hr" : "1 小时") : `${minutes} ${en ? "min" : "分钟"}`}</button>)}
      </div>
      <label className="delay-custom"><span>{en ? "Minutes" : "分钟"}</span><input type="number" min="1" max="43200" step="1" value={value === "0" ? "" : value} aria-label={en ? "Delay in minutes" : "延迟分钟数"} aria-invalid={value !== "0" && (!Number.isInteger(Number(value)) || Number(value) < 1 || Number(value) > 43200)} onChange={event => onChange(event.target.value)} onBlur={() => { if (!value.trim()) onChange("0"); }} /></label>
      <div className="delay-dial-row">
        <label>MAX <input type="number" min="5" max="360" value={maxInput} aria-label={en ? "Maximum dial minutes" : "拨盘上限（分钟）"} onChange={event => setMaxInput(event.target.value)} onBlur={() => { const n = Number(maxInput); if (maxInput.trim() && Number.isFinite(n)) setLimit(n); else setMaxInput(String(max)); }} onKeyDown={event => { if (event.key === "Enter") event.currentTarget.blur(); }} /></label>
        <span>{en ? "Drag dial · scroll for max" : "拖动圆周 · 滚轮调上限"}</span>
        <DelayDial minutes={Number(value) || 0} maxMinutes={max} onMaxChange={setLimit} label={en ? "Adjust delay" : "调整延迟"} hint={en ? "Drag or use arrow keys" : "拖动或使用方向键"} onChange={n => onChange(String(n))} />
      </div>
      <button type="button" className="delay-clear" onClick={() => { onChange("0"); setOpen(false); }}>{en ? "No delay" : "不延迟"}</button>
    </div>}
  </div>;
}
