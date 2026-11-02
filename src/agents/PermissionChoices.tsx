import { useI18n } from "../i18n";
import {
  SANDBOX_LABELS,
  type PromptScope,
  type SandboxMode,
} from "../types";
import { KNOWN_TOOLS } from "./agent-fields";

const PROMPT_SCOPE_KEYS: Record<PromptScope, string> = {
  both: "agents.promptScopeBoth",
  agent: "agents.promptScopeAgent",
  workspace: "agents.promptScopeWorkspace",
  none: "agents.promptScopeNone",
};

export function ToolChoices({
  value,
  onToggle,
  className = "tool-row",
}: {
  value: string[];
  onToggle: (tool: string) => void;
  className?: string;
}) {
  return (
    <div className={className}>
      {KNOWN_TOOLS.map((tool) => (
        <label
          key={tool}
          className={`ios-toggle tool-grid-toggle${tool.length > 18 ? " wide" : ""}`}
        >
          <input
            type="checkbox"
            checked={value.includes(tool)}
            onChange={() => onToggle(tool)}
          />
          <span className="slider" />
          <span className="toggle-label mono">{tool}</span>
        </label>
      ))}
    </div>
  );
}

export function SandboxChoices({
  value,
  onChange,
  name,
  className = "tool-row",
}: {
  value: SandboxMode;
  onChange: (mode: SandboxMode) => void;
  name: string;
  className?: string;
}) {
  return (
    <div className={className}>
      {(Object.keys(SANDBOX_LABELS) as SandboxMode[]).map((mode) => (
        <label key={mode} className="tool-check">
          <input
            type="radio"
            name={name}
            checked={value === mode}
            onChange={() => onChange(mode)}
          />
          <span>{SANDBOX_LABELS[mode]}</span>
        </label>
      ))}
    </div>
  );
}

export function PromptScopeChoices({
  value,
  onChange,
  name,
  className = "tool-row vertical-choices",
}: {
  value: PromptScope;
  onChange: (scope: PromptScope) => void;
  name: string;
  className?: string;
}) {
  const { t } = useI18n();
  return (
    <div className={className}>
      {(Object.keys(PROMPT_SCOPE_KEYS) as PromptScope[]).map((scope) => (
        <label key={scope} className="tool-check">
          <input
            type="radio"
            name={name}
            checked={value === scope}
            onChange={() => onChange(scope)}
          />
          <span>{t(PROMPT_SCOPE_KEYS[scope])}</span>
        </label>
      ))}
    </div>
  );
}
