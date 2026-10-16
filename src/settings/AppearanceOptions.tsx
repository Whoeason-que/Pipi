import { IconCheck } from "../icons";
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
      aria-label={theme.label}
      aria-pressed={active}
      onClick={onClick}
      title={theme.description}
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
        <span>{theme.label}</span>
        <SelectionMark active={active} />
      </span>
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
        <span>{code === "ZH" ? "Chinese, Simplified" : "英语"}</span>
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
