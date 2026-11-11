import { useEffect, useId, useRef, useState, type CSSProperties } from "react";
import { Popover } from "radix-ui";
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
  const committing = useRef(false);
  const dirty = useRef(false);
  const titleId = useId();
  const modelName = model?.name || model?.id || t("chat.noModel");

  useEffect(() => { setIndex(savedIndex < 0 ? 0 : savedIndex); dirty.current = false; }, [savedIndex, model?.id]);
  useEffect(() => { if (disabled && !committing.current) setOpen(false); }, [disabled]);

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

  return <div className="model-effort">
    <Popover.Root open={open} onOpenChange={next => {
      if (next) { setIndex(isDefault ? 0 : savedIndex); dirty.current = false; }
      setOpen(next);
    }}>
      <Popover.Trigger asChild>
        <button type="button" className="model-effort-trigger" disabled={disabled}
          aria-label={t("chat.modelEffortAria", { title: t("chat.modelAndEffort"), model: modelName, effort: currentLabel })}
          title={disabledReason || (customModel ? t("chat.modelCustomTitle", { model: modelName }) : modelName)}
          onClick={event => {
            // 没有模型时先选模型：阻止 Radix 的 toggle，否则会在弹窗背后留下一个空面板。
            if (!model) { event.preventDefault(); onOpenModel(); }
          }}>
          <span className="model-effort-copy"><span className="truncate model-effort-name">{modelName}</span><span className="model-effort-value">{currentLabel}</span></span>
          <span className="model-effort-chevron" aria-hidden="true">⌄</span>
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content className="model-effort-panel" side="top" align="end" sideOffset={10}
          collisionPadding={8} aria-labelledby={titleId}
          // 焦点交给 input 的 autoFocus（直接可拖档位），别让 FocusScope 抢到重置按钮。
          onOpenAutoFocus={event => event.preventDefault()}>
          <div className="model-effort-heading">
            <span id={titleId} className="model-effort-current">{currentLabel}</span>
            <button type="button" className="icon-btn" aria-label={t("chat.resetEffort")} disabled={disabled || isDefault} onClick={() => { void reset(); }}><IconRefresh /></button>
          </div>
          <button type="button" className="model-effort-model" disabled={disabled || index !== savedIndex} onClick={() => { setOpen(false); onOpenModel(); }}>
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
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  </div>;
}
