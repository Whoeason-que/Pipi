import { SANDBOX_LABELS, type BashMode, type SandboxMode } from "../types";
import { BASH_MODE_LABELS, KNOWN_TOOLS } from "./agent-fields";

export function ToolChoices({ value, onToggle, className = "tool-row" }: {
  value: string[]; onToggle: (tool: string) => void; className?: string;
}) {
  return <div className={className}>{KNOWN_TOOLS.map(tool => (
    <label key={tool} className="ios-toggle tool-grid-toggle">
      <input type="checkbox" checked={value.includes(tool)} onChange={() => onToggle(tool)} />
      <span className="slider" /><span className="toggle-label mono">{tool}</span>
    </label>
  ))}</div>;
}

export function BashChoices({ value, onChange, name, className = "tool-row" }: {
  value: BashMode; onChange: (mode: BashMode) => void; name: string; className?: string;
}) {
  return <div className={className}>{(Object.keys(BASH_MODE_LABELS) as BashMode[]).map(mode => (
    <label key={mode} className="tool-check">
      <input type="radio" name={name} checked={value === mode} onChange={() => onChange(mode)} />
      <span>{BASH_MODE_LABELS[mode]}</span>
    </label>
  ))}</div>;
}

export function SandboxChoices({ value, onChange, name, className = "tool-row" }: {
  value: SandboxMode; onChange: (mode: SandboxMode) => void; name: string; className?: string;
}) {
  return <div className={className}>{(Object.keys(SANDBOX_LABELS) as SandboxMode[]).map(mode => (
    <label key={mode} className="tool-check">
      <input type="radio" name={name} checked={value === mode} onChange={() => onChange(mode)} />
      <span>{SANDBOX_LABELS[mode]}</span>
    </label>
  ))}</div>;
}
