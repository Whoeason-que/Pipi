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
  const { locale, t } = useI18n();
  const en = locale === "en";
  // default 不是档位：它是「不发送 reasoningEffort」的状态，由重置按钮恢复。
  const choices = reasoningChoices(en).filter(
    (choice): choice is { value: ReasoningEffort; label: string } => choice.value !== "default",
  );
  const savedIndex = choices.findIndex(choice => choice.value === model?.reasoningEffort);
  const isDefault = savedIndex < 0;
  const currentLabel = isDefault ? t("app.reasoning.default") : choices[savedIndex].label;
  const [open, setOpen] = useState(false);
  const [index, setIndex] = useState(isDefault ? 0 : savedIndex);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const committing = useRef(false);
  const dirty = useRef(false);
  const titleId = useId();
  const modelName = model?.name || model?.id || t("chat.noModel");
  const close = (restoreFocus = false) => { setOpen(false); if (restoreFocus) trigger.current?.focus(); };

  useEffect(() => { setIndex(savedIndex < 0 ? 0 : savedIndex); dirty.current = false; }, [savedIndex, model?.id]);
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
      const accepted = await onEffortChange(choices[next].value);
      if (accepted) dirty.current = false;
      else setIndex(savedIndex < 0 ? 0 : savedIndex);
    } finally { committing.current = false; }
  };

  const reset = async () => {
    if (disabled || !model || committing.current || isDefault) return;
    committing.current = true;
    try {
      const accepted = await onEffortChange(null);
      if (accepted) { setIndex(0); dirty.current = false; }
      else setIndex(savedIndex);
    } finally { committing.current = false; }
  };

  return <div className="model-effort" ref={root} onBlur={event => {
    if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget as Node)) setOpen(false);
  }}>
    <button ref={trigger} type="button" className="model-effort-trigger" disabled={disabled}
      aria-expanded={open} aria-haspopup="dialog" aria-label={t("chat.modelEffortAria", { title: t("chat.modelAndEffort"), model: modelName, effort: currentLabel })}
      title={disabledReason || (customModel ? t("chat.modelCustomTitle", { model: modelName }) : modelName)}
      onClick={() => { if (!model) onOpenModel(); else { setIndex(isDefault ? 0 : savedIndex); setOpen(!open); } }}>
      <span className="model-effort-copy"><span className="truncate model-effort-name">{modelName}</span><span className="model-effort-value">{currentLabel}</span></span>
      <span className="model-effort-chevron" aria-hidden="true">⌄</span>
    </button>
    {open && <div className="model-effort-panel" role="dialog" aria-labelledby={titleId}>
      <div className="model-effort-heading">
        <span id={titleId} className="model-effort-current">{currentLabel}</span>
        <button type="button" className="icon-btn" aria-label={t("chat.resetEffort")} disabled={disabled || isDefault} onClick={() => { void reset(); }}><IconRefresh /></button>
      </div>
      <button type="button" className="model-effort-model" disabled={disabled || index !== savedIndex} onClick={() => { close(); onOpenModel(); }}>
        <span className="truncate">{modelName}</span><IconChevronRight />
      </button>
      <div className={`model-effort-slider${isDefault ? " is-default" : ""}`} style={{ "--effort-progress": `${index / (choices.length - 1) * 100}%` } as CSSProperties}>
        <div className="model-effort-track" aria-hidden="true"><span className="model-effort-fill" /><div className="model-effort-ticks">{choices.map(choice => <i key={choice.value} />)}</div></div>
        <input autoFocus type="range" min="0" max={choices.length - 1} step="1" value={index} aria-disabled={disabled}
          aria-label={t("chat.reasoningEffort")} aria-valuetext={isDefault && index === 0 && !dirty.current ? t("app.reasoning.default") : choices[index].label}
          onChange={event => { if (disabled) return; dirty.current = true; setIndex(Number(event.target.value)); }}
          onPointerUp={event => { if (dirty.current) void commit(Number(event.currentTarget.value)); }}
          onPointerCancel={() => { dirty.current = false; setIndex(savedIndex < 0 ? 0 : savedIndex); }}
          onKeyUp={event => { if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"].includes(event.key)) void commit(Number(event.currentTarget.value)); }}
          onBlur={event => { if (dirty.current) void commit(Number(event.currentTarget.value)); }} />
      </div>
      <div className="model-effort-scale" aria-hidden="true"><span>{isDefault && index === 0 && !dirty.current ? t("app.reasoning.default") : choices[0].label}</span><span>{choices[choices.length - 1].label}</span></div>
    </div>}
  </div>;
}
