import { useI18n } from "./i18n";
import { ChoiceSelect } from "./Select";
import type { ReasoningEffort } from "./types";
import { reasoningChoices } from "./reasoning-effort";

interface ReasoningSelectProps {
  id: string;
  value: ReasoningEffort | null | undefined;
  onChange: (effort: ReasoningEffort | null) => void;
  disabled?: boolean;
}

export function ReasoningSelect({ id, value, onChange, disabled }: ReasoningSelectProps) {
  const { locale } = useI18n();
  const en = locale === "en";
  const choices = reasoningChoices(en);
  return (
    <>
      <label className="label" htmlFor={id}>
        {en ? "Reasoning effort" : "思考程度"}
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
        menuPlacement="auto"
      />
      <span className="hint">
        {en
          ? "Higher effort can take longer and use more tokens. Available levels depend on the model and provider; default leaves the choice to the model."
          : "较高档位可能增加耗时和 token 用量。支持的档位取决于模型和供应商；默认沿用模型自身行为。"}
      </span>
    </>
  );
}
