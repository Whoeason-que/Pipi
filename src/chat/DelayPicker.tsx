import { useEffect, useRef, useState } from "react";
import { Popover } from "radix-ui";
import { useI18n } from "../i18n";
import { IconClock } from "../icons";
import DelayDial from "./DelayDial";

interface Props {
  value: string;
  onChange: (value: string) => void;
}

/** Delay selection is independent of sending; presets work without precise gestures. */
export default function DelayPicker({ value, onChange }: Props) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const [max, setMax] = useState(60);
  const [maxInput, setMaxInput] = useState("60");
  // 回调 ref：面板只在打开时挂载，用 state 让滚轮监听跟着节点生命周期重挂。
  const [panel, setPanel] = useState<HTMLDivElement | null>(null);
  const lastWheel = useRef(0);
  const setLimit = (limit: number) => {
    const next = Math.max(5, Math.min(360, Math.round(limit)));
    setMax(next);
    setMaxInput(String(next));
    if (Number(value) > next) onChange(String(next));
  };
  useEffect(() => {
    const element = panel;
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
  }, [panel, open, max, value]);
  const selected = Number(value) > 0;
  return <div className="delay-picker">
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>
        <button type="button" className={`delay-trigger${selected ? " active" : ""}`} aria-label={selected ? t("chat.setDelayWithValue", { minutes: value }) : t("chat.setDelay")} title={t("chat.setDelay")}>
          <IconClock size={16} />{selected && <span>{t("chat.delayShort", { minutes: value })}</span>}
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content ref={setPanel} className="delay-panel" side="top" align="end" sideOffset={8} collisionPadding={8}>
          <div className="delay-panel-title">{t("chat.sendLater")}</div>
          <div className="delay-presets">
            {[5, 15, 60].map(minutes => <button type="button" key={minutes} className={Number(value) === minutes ? "active" : ""} onClick={() => { onChange(String(minutes)); setOpen(false); }}>{minutes === 60 ? t("chat.oneHour") : t("chat.minutesShort", { count: minutes })}</button>)}
          </div>
          <label className="delay-custom"><span>{t("chat.minutes")}</span><input type="number" min="1" max="43200" step="1" value={value === "0" ? "" : value} aria-label={t("chat.delayMinutesAria")} aria-invalid={value !== "0" && (!Number.isInteger(Number(value)) || Number(value) < 1 || Number(value) > 43200)} onChange={event => onChange(event.target.value)} onBlur={() => { if (!value.trim()) onChange("0"); }} /></label>
          <div className="delay-dial-row">
            <label>MAX <input type="number" min="5" max="360" value={maxInput} aria-label={t("chat.maxDialMinutes")} onChange={event => setMaxInput(event.target.value)} onBlur={() => { const n = Number(maxInput); if (maxInput.trim() && Number.isFinite(n)) setLimit(n); else setMaxInput(String(max)); }} onKeyDown={event => { if (event.key === "Enter") event.currentTarget.blur(); }} /></label>
            <span>{t("chat.dragDialHint")}</span>
            <DelayDial minutes={Number(value) || 0} maxMinutes={max} onMaxChange={setLimit} label={t("chat.adjustDelay")} hint={t("chat.adjustDelayHint")} onChange={n => onChange(String(n))} />
          </div>
          <button type="button" className="delay-clear" onClick={() => { onChange("0"); setOpen(false); }}>{t("chat.noDelay")}</button>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  </div>;
}
