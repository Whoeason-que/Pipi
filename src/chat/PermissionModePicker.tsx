import { useState } from "react";
import { DropdownMenu } from "radix-ui";
import ConfirmModal from "../components/ConfirmModal";
import { useI18n } from "../i18n";
import { IconCheck, IconChevronDown, IconShield, IconShieldAlert } from "../icons";
import type { PermissionMode, SandboxMode } from "../types";

interface ModeOption {
  value: PermissionMode;
  label: string;
  description: string;
  tone: string;
}

/** 会话权限 chip：定位 / 键盘导航交给 Radix，外观与延迟面板同一套语言。 */
export default function PermissionModePicker({ mode, activeMode, agentSandbox, running, disabled, onChange, onError }: {
  mode: PermissionMode | null; activeMode: PermissionMode | null; agentSandbox: SandboxMode;
  running: boolean; disabled: boolean; onChange: (mode: PermissionMode | null, confirmed: boolean) => Promise<void>; onError: (message: string) => void;
}) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState(false);
  const [confirmFull, setConfirmFull] = useState(false);
  const [confirmAuto, setConfirmAuto] = useState(false);
  const options: ModeOption[] = [
    { value: "plan", label: t("chat.permissionPlan"), description: t("chat.permissionPlanDesc"), tone: "tone-accent" },
    { value: "ask", label: t("chat.permissionAsk"), description: t("chat.permissionAskDesc"), tone: "tone-warn" },
    { value: "workspace", label: t("chat.permissionWorkspace"), description: t("chat.permissionWorkspaceDesc"), tone: "tone-neutral" },
    { value: "auto-review", label: t("chat.permissionAuto"), description: t("chat.permissionAutoDesc"), tone: "tone-accent" },
    { value: "full-access", label: t("chat.permissionFull"), description: t("chat.permissionFullDesc"), tone: "tone-danger" },
  ];
  const defaultLabel = t("chat.permissionDefault", { mode: agentSandbox });
  const currentOption = options.find(option => option.value === mode);
  const label = currentOption?.label ?? defaultLabel;
  const pending = running && mode !== activeMode;
  const limitations = mode !== null && mode !== "full-access"
    ? t(mode === "workspace" ? "chat.permissionWorkspaceTools" : "chat.permissionLimitedTools")
    : null;

  const change = async (next: PermissionMode | null, confirmed = false) => {
    setBusy(true);
    try { await onChange(next, confirmed); }
    catch (error) { onError(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(false); }
  };

  const choose = (next: string) => {
    const value = next as PermissionMode;
    if (value === mode) return;
    setOpen(false);
    if (value === "full-access") { window.setTimeout(() => setConfirmFull(true), 0); return; }
    if (value === "auto-review") { window.setTimeout(() => setConfirmAuto(true), 0); return; }
    void change(value);
  };

  return <div className="permission-picker">
    <DropdownMenu.Root open={open} onOpenChange={setOpen}>
      <DropdownMenu.Trigger asChild>
        <button type="button" className={`permission-trigger ${currentOption?.tone ?? "tone-neutral"}`}
          disabled={disabled || busy} data-pending={pending || undefined}
          aria-label={`${t("chat.permissionMode")}: ${label}${pending ? ` · ${t("chat.permissionNextTurn")}` : ""}`}
          title={pending ? `${label} · ${t("chat.permissionNextTurn")}` : label}>
          <span className="permission-trigger-icon" aria-hidden="true">
            {mode === "full-access" ? <IconShieldAlert /> : <IconShield />}
          </span>
          <span className="permission-trigger-label">{label}</span>
          <span className="permission-trigger-chevron" aria-hidden="true"><IconChevronDown /></span>
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content className="permission-menu" side="top" align="start" sideOffset={8}
          collisionPadding={8} aria-label={t("chat.permissionMode")}>
          <div className="permission-menu-head">
            <span className="permission-menu-title">{t("chat.permissionMode")}</span>
            {pending && <span className="permission-menu-pending">{t("chat.permissionNextTurn")}</span>}
          </div>
          <DropdownMenu.RadioGroup value={mode ?? ""} onValueChange={choose} className="permission-menu-list">
            {options.map(option => (
              <DropdownMenu.RadioItem key={option.value} value={option.value} className={`permission-menu-item ${option.tone}`}>
                <span className="permission-menu-text">
                  <span className="permission-menu-label">{option.label}</span>
                  <span className="permission-menu-desc">{option.description}</span>
                </span>
                <DropdownMenu.ItemIndicator className="permission-menu-check"><IconCheck /></DropdownMenu.ItemIndicator>
              </DropdownMenu.RadioItem>
            ))}
          </DropdownMenu.RadioGroup>
          <div className="permission-menu-footer">
            {limitations && <p className="permission-menu-note">{limitations}</p>}
            {mode === null
              ? <p className="permission-menu-default">{defaultLabel}</p>
              : <DropdownMenu.Item className="permission-menu-reset" disabled={disabled || busy}
                  onSelect={() => { void change(null); }}>
                  {t("chat.permissionUseDefault", { mode: agentSandbox })}
                </DropdownMenu.Item>}
          </div>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
    {confirmFull && <ConfirmModal title={t("chat.permissionFull")} message={t("chat.permissionFullConfirm")}
      confirmLabel={t("chat.permissionFullConfirmButton")} onCancel={() => setConfirmFull(false)}
      onConfirm={async () => { await change("full-access", true); setConfirmFull(false); }} />}
    {confirmAuto && <ConfirmModal title={t("chat.permissionAuto")} message={t("chat.permissionAutoConfirm")}
      confirmLabel={t("chat.permissionAutoConfirmButton")} onCancel={() => setConfirmAuto(false)}
      onConfirm={async () => { await change("auto-review"); setConfirmAuto(false); }} />}
  </div>;
}
