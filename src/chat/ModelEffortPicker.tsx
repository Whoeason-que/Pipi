import { useEffect, useId, useRef, useState, type CSSProperties } from "react";
import { IconChevronRight, IconRefresh } from "../icons";
import { useI18n } from "../i18n";
import { reasoningChoices } from "../reasoning-effort";
import type { ModelConfig, ReasoningEffort } from "../types";

interface Props {
  model: ModelConfig | null;
  customModel: boolean;
  disabled: boolean;
  disabledReason?: string;
  onOpenModel: () => void;
  onEffortChange: (effort: ReasoningEffort | null) => Promise<boolean>;
}

/** One entry for model selection and session reasoning effort. */
export default function ModelEffortPicker({ model, customModel, disabled, disabledReason, onOpenModel, onEffortChange }: Props) {
  const { locale } = useI18n();
  const en = locale === "en";
  const choices = reasoningChoices(en);
  const savedIndex = choices.findIndex(choice => choice.value === (model?.reasoningEffort ?? "default"));
  const [open, setOpen] = useState(false);
  const [index, setIndex] = useState(savedIndex);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const committing = useRef(false);
  const titleId = useId();
  const modelName = model?.name || model?.id || (en ? "No model" : "未配置模型");
  const close = (restoreFocus = false) => { setOpen(false); if (restoreFocus) trigger.current?.focus(); };

  useEffect(() => { setIndex(savedIndex); }, [savedIndex, model?.id]);
  useEffect(() => { if (disabled && !committing.current) setOpen(false); }, [disabled]);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { event.preventDefault(); close(true); } };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape); };
  }, [open]);

  const commit = async (next: number) => {
    if (disabled || !model || committing.current || next === savedIndex) return;
    committing.current = true;
    try {
      const value = choices[next].value;
      const accepted = await onEffortChange(value === "default" ? null : value);
      if (!accepted) setIndex(savedIndex);
    } finally { committing.current = false; }
  };
  return <div className="model-effort" ref={root} onBlur={event => {
    if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget as Node)) setOpen(false);
  }}>
    <button ref={trigger} type="button" className="model-effort-trigger" disabled={disabled}
      aria-expanded={open} aria-haspopup="dialog" aria-label={`${en ? "Model and reasoning effort" : "模型与思考程度"}: ${modelName}, ${choices[savedIndex].label}`}
      title={disabledReason || `${modelName}${customModel ? (en ? " · Custom" : " · 自定义") : ""}`}
      onClick={() => { if (!model) onOpenModel(); else { setIndex(savedIndex); setOpen(!open); } }}>
      <span className="model-effort-copy"><span className="truncate model-effort-name">{modelName}</span><span className="model-effort-value">{choices[savedIndex].label}</span></span>
      <span className="model-effort-chevron" aria-hidden="true">⌄</span>
    </button>
    {open && <div className="model-effort-panel" role="dialog" aria-labelledby={titleId}>
      <div className="model-effort-heading">
        <span id={titleId} className="model-effort-current">{choices[index].label}</span>
        <button type="button" className="icon-btn" aria-label={en ? "Reset reasoning effort" : "恢复模型默认思考程度"} disabled={disabled || savedIndex === 0} onClick={() => { setIndex(0); void commit(0); }}><IconRefresh /></button>
      </div>
      <button type="button" className="model-effort-model" disabled={disabled || index !== savedIndex} onClick={() => { close(); onOpenModel(); }}>
        <span className="truncate">{modelName}</span><IconChevronRight />
      </button>
      <div className="model-effort-slider" style={{ "--effort-progress": `${index / (choices.length - 1) * 100}%` } as CSSProperties}>
        <div className="model-effort-track" aria-hidden="true"><span className="model-effort-fill" /><div className="model-effort-ticks">{choices.map(choice => <i key={choice.value} />)}</div></div>
        <input autoFocus type="range" min="0" max={choices.length - 1} step="1" value={index} aria-disabled={disabled}
          aria-label={en ? "Reasoning effort" : "思考程度"} aria-valuetext={choices[index].label}
          onChange={event => { if (!disabled) setIndex(Number(event.target.value)); }}
          onPointerUp={event => void commit(Number(event.currentTarget.value))}
          onPointerCancel={() => setIndex(savedIndex)}
          onKeyUp={event => { if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"].includes(event.key)) void commit(Number(event.currentTarget.value)); }}
          onBlur={event => void commit(Number(event.currentTarget.value))} />
      </div>
      <div className="model-effort-scale" aria-hidden="true"><span>{en ? "Default" : "默认"}</span><span>{choices[choices.length - 1].label}</span></div>
    </div>}
  </div>;
}
