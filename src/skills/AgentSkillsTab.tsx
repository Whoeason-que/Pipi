// Agent 工作台「技能」页：这个 Agent 声明启用的 store 技能（use + agent.lock 状态）、
// 可启用清单，以及同步 / 修复。写操作立即落盘（与 av CLI 共用实现），随后重建
// 临时测试会话，让改动当场生效并暴露 fail-closed 错误。

import { useCallback, useEffect, useState } from "react";
import { formatRuntimeError } from "../chat-runtime";
import { IconRefresh } from "../icons";
import { invoke } from "../platform";
import type { AgentSkills, SkillSyncReport } from "../types";
import {
  AGENT_SKILL_STATUS_HINTS,
  AGENT_SKILL_STATUS_LABELS,
  AGENT_SKILL_STATUS_TONES,
  shortHash,
  toneClass,
} from "./labels";

interface AgentSkillsTabProps {
  agentName: string;
  onError: (message: string) => void;
  /** 改动落盘后：重建临时测试会话，让改动立刻生效。 */
  onChanged: () => Promise<void> | void;
}

function reportLines(report: SkillSyncReport): string[] {
  const lines: string[] = [];
  if (report.resolved.length) lines.push(`补解析锁：${report.resolved.join("、")}`);
  if (report.installed.length) lines.push(`复现内容：${report.installed.join("、")}`);
  if (report.repaired.length) lines.push(`重装漂移：${report.repaired.join("、")}`);
  if (report.ready.length) lines.push(`已就绪：${report.ready.join("、")}`);
  return [...lines, ...report.log];
}

export function AgentSkillsTab({ agentName, onError, onChanged }: AgentSkillsTabProps) {
  const [view, setView] = useState<AgentSkills | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [working, setWorking] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);
  const [repair, setRepair] = useState(false);
  const [syncLog, setSyncLog] = useState<string[] | null>(null);

  useEffect(() => {
    let active = true;
    setView(null);
    setLoadError(null);
    setSyncLog(null);
    void invoke("agent_skills", { agentName })
      .then((next) => {
        if (active) setView(next);
      })
      .catch((errorValue) => {
        if (!active) return;
        const message = formatRuntimeError(errorValue);
        setLoadError(message);
        onError(message);
      });
    return () => {
      active = false;
    };
  }, [agentName, onError, reloadToken]);

  const reload = useCallback(async () => {
    setReloadToken((token) => token + 1);
    await onChanged();
  }, [onChanged]);

  const toggle = async (name: string, enabled: boolean) => {
    setWorking(`toggle:${name}`);
    try {
      await invoke("set_skill_enabled", { agentName, name, enabled });
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(null);
      await reload();
    }
  };

  const sync = async () => {
    setWorking("sync");
    try {
      const report = await invoke("sync_agent_skills", { agentName, repair });
      setSyncLog(reportLines(report));
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(null);
      await reload();
    }
  };

  if (!view) {
    return (
      <section className="settings-section">
        {loadError ? (
          <>
            <p className="field-error">读取技能声明失败：{loadError}</p>
            <button
              type="button"
              className="btn ghost"
              onClick={() => setReloadToken((token) => token + 1)}
            >
              <IconRefresh /> 重试
            </button>
          </>
        ) : (
          <p className="dim">正在读取技能声明…</p>
        )}
      </section>
    );
  }

  const busy = working !== null;
  const declaredByLabel =
    view.declaredBy === "project-local"
      ? "项目本地层 agent.local.toml"
      : view.declaredBy === "project"
        ? "项目层 agent.toml"
        : "Agent 层 agent.toml";

  return (
    <section className="settings-section agent-skills">
      <div className="settings-summary-row">
        <span className="field-label">声明</span>
        {view.declared.length > 0 ? (
          <span>
            {declaredByLabel}
            {view.declaresInProject && <span className="tag warn" title="项目层声明覆盖 Agent 层；编辑会写工作区里的文件">项目层</span>}
          </span>
        ) : view.declaredBy ? (
          <span>{declaredByLabel}（use = []，显式禁用）</span>
        ) : (
          <span className="dim">未声明 use</span>
        )}
      </div>
      <div className="settings-summary-row">
        <span className="field-label">契约</span>
        <span className="mono break-all" title={view.contractPath}>
          {view.contractPath}
        </span>
      </div>
      <div className="settings-summary-row">
        <span className="field-label">解析锁</span>
        <span className="mono break-all" title={view.lockPath}>
          {view.lockPath}
        </span>
      </div>

      {view.lockError && (
        <p className="field-error" role="alert">
          {view.lockError}
        </p>
      )}

      <div className="settings-field">
        <span className="field-label">已启用（{view.declared.length}）</span>
        {view.declared.length === 0 && <span className="dim">没有声明启用的技能</span>}
        <div className="agent-skills-list">
          {view.declared.map((skill) => (
            <div className="agent-skills-row" key={skill.name}>
              <div className="info">
                <div className="name">
                  <span className="mono">{skill.name}</span>
                  <span className={toneClass(AGENT_SKILL_STATUS_TONES[skill.status])} title={skill.filter ?? AGENT_SKILL_STATUS_HINTS[skill.status]}>
                    {AGENT_SKILL_STATUS_LABELS[skill.status]}
                  </span>
                </div>
                <div className="dim ellipsis" title={skill.description ?? undefined}>
                  {skill.description ?? (skill.contentHash ? shortHash(skill.contentHash) : "—")}
                </div>
              </div>
              <button
                type="button"
                className="link danger"
                disabled={busy}
                onClick={() => void toggle(skill.name, false)}
              >
                {working === `toggle:${skill.name}` ? "…" : "禁用"}
              </button>
            </div>
          ))}
        </div>
      </div>

      <div className="settings-field">
        <span className="field-label">可启用（store 里 {view.available.length} 个）</span>
        {view.available.length === 0 && (
          <span className="dim">
            store 里没有别的技能；在左侧「技能」分区安装（装 ≠ 启用）。
          </span>
        )}
        <div className="agent-skills-list">
          {view.available.map((skill) => (
            <div className="agent-skills-row" key={skill.name}>
              <div className="info">
                <div className="name">
                  <span className="mono">{skill.name}</span>
                  {!skill.present && <span className="tag warn">内容缺失</span>}
                </div>
                <div className="dim ellipsis" title={skill.description ?? undefined}>
                  {skill.description ?? shortHash(skill.contentHash)}
                </div>
              </div>
              <button
                type="button"
                className="link"
                disabled={busy || !skill.present}
                title={skill.present ? "写入 use 并生成解析锁" : "store 内容缺失：先安装或同步"}
                onClick={() => void toggle(skill.name, true)}
              >
                {working === `toggle:${skill.name}` ? "…" : "启用"}
              </button>
            </div>
          ))}
        </div>
      </div>

      <div className="settings-field">
        <span className="field-label">修复</span>
        <div className="agent-skills-sync">
          <button type="button" className="btn ghost" disabled={busy || view.declared.length === 0} onClick={() => void sync()}>
            <IconRefresh /> {working === "sync" ? "同步中…" : "同步声明技能"}
          </button>
          <label className="ios-toggle">
            <input type="checkbox" checked={repair} disabled={busy} onChange={(event) => setRepair(event.target.checked)} />
            <span className="slider" />
            <span className="toggle-label">修复漂移</span>
          </label>
        </div>
        <span className="hint">
          按 agent.lock 复现缺失内容（git 源会联网）；「修复漂移」才会重装与锁不一致的内容。
        </span>
        {syncLog && syncLog.length > 0 && <pre className="skill-log">{syncLog.join("\n")}</pre>}
      </div>

      {view.only || view.exclude || view.sources ? (
        <div className="settings-field">
          <span className="field-label">过滤（only / exclude / sources）</span>
          <div className="settings-tags">
            {view.only && <span className="tag" title="只注入匹配的技能名（glob）">only: {view.only.join("、")}</span>}
            {view.exclude && <span className="tag" title="匹配的技能名不注入（优先于 only）">exclude: {view.exclude.join("、")}</span>}
            {view.sources && <span className="tag" title="按层替换约定技能目录">sources: {view.sources.join("、")}</span>}
          </div>
        </div>
      ) : null}

      <p className="hint">
        启用/禁用立即写入契约与 agent.lock（与 `av skill add/remove` 同一实现），不需要「保存配置」；
        正在运行的会话不受影响，下一个会话启动时生效。
      </p>
    </section>
  );
}
