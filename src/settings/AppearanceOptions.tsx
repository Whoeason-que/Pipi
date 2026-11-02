import { IconCheck } from "../icons";
import { useI18n } from "../i18n";
import type { ThemeDefinition } from "../theme";

export function ThemeOption({
  active,
  theme,
  onClick,
}: {
  active: boolean;
  theme: ThemeDefinition;
  onClick: () => void;
}) {
  const { t } = useI18n();
  const label = t(`themes.${theme.id}.label`);
  const description = t(`themes.${theme.id}.description`);
  const { preview } = theme;
  const previewStyle = {
    "--preview-bg": preview.bg,
    "--preview-nav": preview.nav,
    "--preview-rail": preview.rail,
    "--preview-card": preview.card,
    "--preview-line": preview.line,
    "--preview-accent": preview.accent,
  } as React.CSSProperties;

  return (
    <button
      type="button"
      className={`theme-option${active ? " active" : ""}`}
      aria-label={label}
      aria-pressed={active}
      onClick={onClick}
      title={description}
    >
      <span
        className={`theme-preview ${theme.mode}`}
        style={previewStyle}
        aria-hidden="true"
      >
        <span className="theme-preview-rail">
          <i />
          <i />
          <i />
        </span>
        <span className="theme-preview-sidebar">
          <b />
          <i />
          <i />
          <i />
        </span>
        <span className="theme-preview-chat">
          <span className="theme-preview-bar">
            <i />
            <i />
            <i />
          </span>
          <span className="theme-preview-message user" />
          <span className="theme-preview-message" />
          <span className="theme-preview-message short" />
          <span className="theme-preview-composer" />
        </span>
      </span>
      <span className="option-caption">
        <span>{label}</span>
        <SelectionMark active={active} />
      </span>
      <span className="theme-description">{description}</span>
    </button>
  );
}

export function LocaleOption({
  active,
  code,
  label,
  onClick,
}: {
  active: boolean;
  code: string;
  label: string;
  onClick: () => void;
}) {
  const { t } = useI18n();
  return (
    <button
      type="button"
      className={`locale-option${active ? " active" : ""}`}
      aria-label={`${code} ${label}`}
      aria-pressed={active}
      onClick={onClick}
    >
      <span className="locale-badge" aria-hidden="true">
        {code === "ZH" ? "文" : "Aa"}
      </span>
      <span className="locale-caption">
        <strong>{label}</strong>
        <span>{code === "ZH" ? t("settings.chineseSimplified") : t("settings.english")}</span>
      </span>
      <SelectionMark active={active} />
    </button>
  );
}

function SelectionMark({ active }: { active: boolean }) {
  return (
    <span
      className={`option-check${active ? " selected" : ""}`}
      aria-hidden="true"
    >
      {active && <IconCheck />}
    </span>
  );
}
