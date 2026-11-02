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
  const { locale, t } = useI18n();
  const en = locale === "en";
  const choices = reasoningChoices(en);
  return (
    <>
      <label className="label" htmlFor={id}>
        {t("app.reasoningEffort")}
      </label>
      <ChoiceSelect
        id={id}
        value={value ?? "default"}
        choices={choices}
        disabled={disabled}
        ariaLabel={t("app.reasoningEffort")}
        onChange={(next) => {
          const choice = choices.find((candidate) => candidate.value === next);
          onChange(choice && choice.value !== "default" ? choice.value : null);
        }}
        menuInPortal
        menuPlacement="auto"
      />
      <span className="hint">
        {t("app.reasoningHint")}
      </span>
    </>
  );
}
