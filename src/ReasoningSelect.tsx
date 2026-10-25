import { useI18n } from "./i18n";
import { ChoiceSelect } from "./Select";
import type { ReasoningEffort } from "./types";

interface ReasoningSelectProps {
  id: string;
  value: ReasoningEffort | null | undefined;
  onChange: (effort: ReasoningEffort | null) => void;
  disabled?: boolean;
  compact?: boolean;
}

export function ReasoningSelect({ id, value, onChange, disabled, compact = false }: ReasoningSelectProps) {
  const { locale } = useI18n();
  const en = locale === "en";
  const choices: { value: ReasoningEffort | "default"; label: string }[] = [
    { value: "default", label: en ? "Model default" : "模型默认" },
    { value: "none", label: en ? "Off" : "关闭" },
    { value: "minimal", label: en ? "Minimal" : "极低" },
    { value: "low", label: en ? "Low" : "低" },
    { value: "medium", label: en ? "Medium" : "中" },
    { value: "high", label: en ? "High" : "高" },
    { value: "xhigh", label: en ? "Extra high" : "极高" },
    { value: "max", label: en ? "Maximum" : "最高" },
  ];
  return (
    <>
      <label className={compact ? "composer-reasoning-label" : "label"} htmlFor={id}>
        {compact ? (en ? "Thinking" : "思考") : (en ? "Reasoning effort" : "思考程度")}
      </label>
      <ChoiceSelect
        id={id}
        value={value ?? "default"}
        choices={choices}
        disabled={disabled}
        ariaLabel={en ? "Reasoning effort" : "思考程度"}
        onChange={(next) => {
          const choice = choices.find((candidate) => candidate.value === next);
          onChange(choice && choice.value !== "default" ? choice.value : null);
        }}
        menuInPortal
        menuPlacement={compact ? "top" : "auto"}
      />
      {!compact && <span className="hint">
        {en
          ? "Higher effort can take longer and use more tokens. Available levels depend on the model and provider; default leaves the choice to the model."
          : "较高档位可能增加耗时和 token 用量。支持的档位取决于模型和供应商；默认沿用模型自身行为。"}
      </span>}
    </>
  );
}
