// 技能右栏：选中技能的版本、来源、校验与 Agent 启用情况。
// 主区保留技能网格，右栏只承担辅助阅读与操作。

import { useCallback, useEffect, useState } from "react";
import { formatRuntimeError } from "../chat-runtime";
import { IconRefresh, IconSkill } from "../icons";
import { invoke } from "../platform";
import type { SkillUsage, SkillVerify, StoreSkill } from "../types";
import { RightRail, type RightRailController } from "../app/right-rail";
import {
  AGENT_SKILL_STATUS_HINTS,
  AGENT_SKILL_STATUS_LABELS,
  AGENT_SKILL_STATUS_TONES,
  SKILL_VERIFY_LABELS,
  SKILL_VERIFY_TONES,
  shortHash,
  sourceLabel,
  toneClass,
} from "./labels";

export function SkillRail({
  controller,
  skill,
  verifyRows,
  verifyBusy,
  onVerify,
  onRequestRemove,
  onError,
}: {
  controller: RightRailController;
  skill: StoreSkill | null;
  verifyRows: SkillVerify[];
  verifyBusy: boolean;
  onVerify: () => void;
  onRequestRemove: (name: string) => void;
  onError: (message: string) => void;
}) {
  const [tab, setTab] = useState("overview");
  const [usage, setUsage] = useState<SkillUsage[] | null>(null);
  const [usageError, setUsageError] = useState<string | null>(null);
  const [working, setWorking] = useState<string | null>(null);
  const name = skill?.name ?? null;

  const loadUsage = useCallback(async (skillName: string) => {
    try {
      setUsage(await invoke("skill_usage", { name: skillName }));
      setUsageError(null);
    } catch (errorValue) {
      setUsage(null);
      setUsageError(formatRuntimeError(errorValue));
    }
  }, []);

  useEffect(() => {
    setTab("overview");
    if (!name) {
      setUsage(null);
      setUsageError(null);
      return;
    }
    void loadUsage(name);
  }, [name, loadUsage]);

  const toggleAgent = async (agentName: string, enabled: boolean) => {
    if (!name) return;
    setWorking(`toggle:${agentName}`);
    try {
      await invoke("set_skill_enabled", { agentName, name, enabled });
      await loadUsage(name);
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(null);
    }
  };

  const current = skill?.versions.find((version) => version.contentHash === skill.currentHash);
  const enabledCount = usage?.filter((row) => row.enabled).length ?? 0;
  const busy = working !== null;

  return <RightRail
    controller={controller}
    id="skill-rail"
    label={skill?.name ?? "技能详情"}
    subtitle={skill ? sourceLabel(skill.source, skill.sourceType) : "从主区选择一个技能"}
    icon={<IconSkill />}
    actions={skill ? <>
      <button type="button" className="icon-btn" title="校验内容" aria-label="校验内容" disabled={verifyBusy} onClick={onVerify}>
        <IconRefresh />
      </button>
      <button type="button" className="btn small" disabled={verifyBusy} onClick={() => onRequestRemove(skill.name)}>
        移除
      </button>
    </> : undefined}
    tabs={skill ? [
      {
        id: "overview",
        label: "概览",
        badge: skill.versions.length,
        content: <Overview skill={skill} current={current} verifyRows={verifyRows} />,
      },
      {
        id: "usage",
        label: "启用",
        badge: usage ? enabledCount : undefined,
        content: <Usage usage={usage} usageError={usageError} busy={busy} working={working}
          onToggle={toggleAgent} />,
      },
    ] : undefined}
    activeTab={tab}
    onTabChange={setTab}
  >
    {!skill && <div className="right-rail-empty">
      <IconSkill />
      <strong>还没有选中技能</strong>
      <p>在主区选择一张技能卡片，这里会显示版本、来源、校验结果和启用它的 Agent。</p>
    </div>}
  </RightRail>;
}

function Overview({ skill, current, verifyRows }: {
  skill: StoreSkill;
  current: StoreSkill["versions"][number] | undefined;
  verifyRows: SkillVerify[];
}) {
  return <>
    <section className="right-rail-section">
      <p>{skill.description ?? "（SKILL.md 没有可读描述）"}</p>
      <div className="right-rail-kv"><span>当前版本</span><span className="mono" title={skill.currentHash}>{shortHash(skill.currentHash)}</span></div>
      <div className="right-rail-kv"><span>来源内路径</span><span className="mono">{skill.skillPath ?? "—"}</span></div>
      <div className="right-rail-kv"><span>安装时间</span><span>{formatInstalledAt(skill.installedAt)}</span></div>
      <div className="right-rail-kv"><span>内容</span><span className={current?.present ? "tag ok" : "tag error"}>{current?.present ? "内容在" : "内容缺失"}</span></div>
    </section>
    <section className="right-rail-section">
      <h3>版本 · {skill.versions.length}</h3>
      <div className="right-rail-list">
        {skill.versions.map((version) => {
          const rows = verifyRows.filter((row) => row.contentHash === version.contentHash);
          const active = version.contentHash === skill.currentHash;
          return <div className={`right-rail-item${active ? " active" : ""}`} key={version.contentHash}>
            <span className="mono right-rail-item-main" title={version.contentHash}>{shortHash(version.contentHash)}</span>
            {active && <small>当前</small>}
            {version.present ? <span className="tag ok">在</span> : <span className="tag error">缺失</span>}
            {rows.map((row) => <span key={`${row.status}:${row.actualHash ?? ""}`}
              className={toneClass(SKILL_VERIFY_TONES[row.status])}
              title={row.actualHash ? `实际哈希：${row.actualHash}` : undefined}>{SKILL_VERIFY_LABELS[row.status]}</span>)}
          </div>;
        })}
      </div>
      <p className="right-rail-note">校验会重算 store 内容哈希；漂移需要重新安装来源才能消除。</p>
    </section>
  </>;
}

function Usage({ usage, usageError, busy, working, onToggle }: {
  usage: SkillUsage[] | null;
  usageError: string | null;
  busy: boolean;
  working: string | null;
  onToggle: (agentName: string, enabled: boolean) => void;
}) {
  if (usageError) return <p className="field-error">{usageError}</p>;
  if (!usage) return <p className="right-rail-note">读取启用情况…</p>;
  if (usage.length === 0) return <div className="right-rail-empty"><strong>还没有 Agent</strong><p>创建 Agent 后可以在这里启用技能。</p></div>;
  return <>
    <section className="right-rail-section">
      <h3>Agent · {usage.filter((row) => row.enabled).length} 个已启用</h3>
      <div className="right-rail-list">
        {usage.map((row) => <div className="right-rail-item" key={row.agentName}>
          <span className="right-rail-avatar">{row.agentName.slice(0, 1).toUpperCase()}</span>
          <span className="right-rail-item-main mono">{row.agentName}</span>
          {row.enabled && row.status ? <span className={toneClass(AGENT_SKILL_STATUS_TONES[row.status])}
            title={row.filter ?? AGENT_SKILL_STATUS_HINTS[row.status]}>{AGENT_SKILL_STATUS_LABELS[row.status]}</span>
            : <small>未启用</small>}
          <button type="button" className={row.enabled ? "btn small ghost" : "btn small"} disabled={busy}
            onClick={() => onToggle(row.agentName, !row.enabled)}>
            {working === `toggle:${row.agentName}` ? "…" : row.enabled ? "禁用" : "启用"}
          </button>
        </div>)}
      </div>
    </section>
    <p className="right-rail-note">启用会写入该 Agent 生效的最高声明层契约与同层 agent.lock，下一个会话启动时生效。</p>
  </>;
}

function formatInstalledAt(value: string | null): string {
  if (!value) return "—";
  const timestamp = Date.parse(value);
  return Number.isFinite(timestamp) ? new Date(timestamp).toLocaleString() : value;
}
