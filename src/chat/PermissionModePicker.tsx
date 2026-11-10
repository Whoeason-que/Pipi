import { useState } from "react";
import ConfirmModal from "../components/ConfirmModal";
import { useI18n } from "../i18n";
import type { PermissionMode, SandboxMode } from "../types";

export default function PermissionModePicker({ mode, activeMode, agentSandbox, running, disabled, onChange, onError }: {
  mode: PermissionMode | null; activeMode: PermissionMode | null; agentSandbox: SandboxMode;
  running: boolean; disabled: boolean; onChange: (mode: PermissionMode | null, confirmed: boolean) => Promise<void>; onError: (message: string) => void;
}) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [confirmFull, setConfirmFull] = useState(false);
  const [confirmAuto, setConfirmAuto] = useState(false);
  const change = async (next: PermissionMode | null, confirmed = false) => {
    setBusy(true);
    try { await onChange(next, confirmed); }
    catch (error) { onError(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(false); }
  };
  return <div className="permission-picker">
    <label>
      <span className="sr-only">{t("chat.permissionMode")}</span>
      <select value={mode ?? "default"} disabled={disabled || busy} aria-label={t("chat.permissionMode")}
        onChange={event => {
          const next = event.target.value === "default" ? null : event.target.value as PermissionMode;
          if (next === "full-access") setConfirmFull(true);
          else if (next === "auto-review") setConfirmAuto(true);
          else void change(next);
        }}>
        <option value="default">{t("chat.permissionDefault", { mode: agentSandbox })}</option>
        <option value="plan">{t("chat.permissionPlan")}</option>
        <option value="ask">{t("chat.permissionAsk")}</option>
        <option value="workspace">{t("chat.permissionWorkspace")}</option>
        <option value="auto-review">{t("chat.permissionAuto")}</option>
        <option value="full-access">{t("chat.permissionFull")}</option>
      </select>
    </label>
    {running && mode !== activeMode && <span className="permission-next">{t("chat.permissionNextTurn")}</span>}
    {mode !== null && mode !== "full-access" && <span className="permission-limit">{t(mode === "workspace" ? "chat.permissionWorkspaceTools" : "chat.permissionLimitedTools")}</span>}
    {confirmFull && <ConfirmModal title={t("chat.permissionFull")} message={t("chat.permissionFullConfirm")}
      confirmLabel={t("chat.permissionFullConfirmButton")} onCancel={() => setConfirmFull(false)}
      onConfirm={async () => { await change("full-access", true); setConfirmFull(false); }} />}
    {confirmAuto && <ConfirmModal title={t("chat.permissionAuto")} message={t("chat.permissionAutoConfirm")}
      confirmLabel={t("chat.permissionAutoConfirmButton")} onCancel={() => setConfirmAuto(false)}
      onConfirm={async () => { await change("auto-review"); setConfirmAuto(false); }} />}
  </div>;
}
