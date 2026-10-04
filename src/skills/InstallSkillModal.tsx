// 安装技能到全局 store（~/.av/skills）的弹层：来源 + 选装策略 + dry-run 预览。
// 只装不启用 —— 启用由 Agent 工作台的「技能」页完成（写 [resources.skills].use）。

import { useEffect, useState } from "react";
import { formatRuntimeError } from "../chat-runtime";
import { IconClose } from "../icons";
import { invoke } from "../platform";
import type { SkillInstallReport } from "../types";
import { shortHash } from "./labels";

type SelectionMode = "auto" | "all" | "only";

const MODE_LABELS: Record<SelectionMode, string> = {
  auto: "自动（来源恰好一个技能）",
  all: "全部技能",
  only: "指定技能名",
};

export default function InstallSkillModal({
  onClose,
  onInstalled,
  onError,
}: {
  onClose: () => void;
  onInstalled: () => Promise<void>;
  onError: (message: string) => void;
}) {
  const [source, setSource] = useState("");
  const [mode, setMode] = useState<SelectionMode>("auto");
  const [names, setNames] = useState("");
  const [dryRun, setDryRun] = useState(false);
  const [working, setWorking] = useState(false);
  const [report, setReport] = useState<SkillInstallReport | null>(null);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  const submit = async () => {
    const trimmed = source.trim();
    if (!trimmed) {
      onError("请填写技能来源（owner/repo[@ref][#子目录]、git URL 或本地目录）");
      return;
    }
    const selected =
      mode === "only"
        ? names.split(/[\s,，]+/).map((name) => name.trim()).filter(Boolean)
        : null;
    if (mode === "only" && (!selected || selected.length === 0)) {
      onError("请填写要安装的技能名，或改用「自动 / 全部」策略");
      return;
    }
    setWorking(true);
    setReport(null);
    try {
      const result = await invoke("install_skill", {
        source: trimmed,
        skills: selected,
        all: mode === "all",
        dryRun,
      });
      setReport(result);
      if (!dryRun) await onInstalled();
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(false);
    }
  };

  const finished = Boolean(report && !report.dryRun);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal skill-install-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="skill-install-title"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="modal-header">
          <h2 id="skill-install-title">安装技能</h2>
          <button type="button" className="icon-btn close-btn" onClick={onClose} title="关闭">
            <IconClose />
          </button>
        </div>
        <div className="modal-section">
          <div className="settings-field">
            <label className="field-label" htmlFor="skill-install-source">
              来源
            </label>
            <input
              id="skill-install-source"
              className="mono"
              value={source}
              onChange={(event) => setSource(event.target.value)}
              placeholder="owner/repo@ref#子目录 或 /本地/技能目录"
              spellCheck={false}
              autoFocus
            />
            <span className="hint">
              git 源走系统 git 浅取到精确 commit（会联网，可能需要一会儿）；本地目录按内容快照复制。
            </span>
          </div>
          <div className="settings-field">
            <span className="field-label">选装策略</span>
            <div className="tool-row vertical-choices">
              {(Object.keys(MODE_LABELS) as SelectionMode[]).map((candidate) => (
                <label key={candidate} className="tool-check">
                  <input
                    type="radio"
                    name="skill-install-mode"
                    checked={mode === candidate}
                    onChange={() => setMode(candidate)}
                  />
                  <span>{MODE_LABELS[candidate]}</span>
                </label>
              ))}
            </div>
          </div>
          {mode === "only" && (
            <div className="settings-field">
              <label className="field-label" htmlFor="skill-install-names">
                技能名
              </label>
              <input
                id="skill-install-names"
                className="mono"
                value={names}
                onChange={(event) => setNames(event.target.value)}
                placeholder="pdf, writer"
                spellCheck={false}
              />
              <span className="hint">逗号或空格分隔；来源里必须存在这些技能，否则整体拒绝。</span>
            </div>
          )}
          <label className="ios-toggle">
            <input
              type="checkbox"
              checked={dryRun}
              onChange={(event) => setDryRun(event.target.checked)}
            />
            <span className="slider" />
            <span className="toggle-label">仅预览（dry-run，不写盘）</span>
          </label>

          {report && (
            <div className="skill-report">
              {report.installed.map((item) => (
                <div className="skill-report-row" key={item.name}>
                  <span className={`tag${item.unchanged ? "" : " ok"}`}>
                    {item.unchanged ? "已是最新" : report.dryRun ? "将安装" : "已安装"}
                  </span>
                  <span className="mono">{item.name}</span>
                  <span className="dim">{shortHash(item.contentHash)}</span>
                </div>
              ))}
              {report.skipped.map((item) => (
                <div className="skill-report-row" key={item.path}>
                  <span className="tag warn">跳过</span>
                  <span className="mono">{item.path}</span>
                  <span className="dim">{item.reason}</span>
                </div>
              ))}
              {report.log.length > 0 && <pre className="skill-log">{report.log.join("\n")}</pre>}
            </div>
          )}

          <p className="hint">
            安装 ≠ 启用：装入 store 只是可用；在 Agent 工作台的「技能」页启用后才会注入会话。
          </p>
        </div>
        <div className="modal-actions">
          <button type="button" className="btn ghost" onClick={onClose} disabled={working}>
            {finished ? "完成" : "取消"}
          </button>
          <button
            type="button"
            className="btn primary"
            onClick={() => void submit()}
            disabled={working}
          >
            {working ? "处理中…（git 源会联网）" : dryRun ? "预览" : "安装"}
          </button>
        </div>
      </div>
    </div>
  );
}
