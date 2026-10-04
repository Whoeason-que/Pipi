// 技能库主区：store 概览（安装 / 全部校验）与单个技能的详情
// （版本与来源、内容校验、启用它的 Agent 及开关）。

import { useCallback, useEffect, useState } from "react";
import { formatRuntimeError } from "../chat-runtime";
import ConfirmModal from "../components/ConfirmModal";
import { IconCheck, IconDownload, IconRefresh } from "../icons";
import { invoke } from "../platform";
import type { SkillLibrary, SkillUsage, SkillVerify } from "../types";
import InstallSkillModal from "./InstallSkillModal";
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

interface SkillsViewProps {
  library: SkillLibrary | null;
  /** 读取 store 失败时的可读原因（与侧栏同一份）。 */
  loadError: string | null;
  selected: string | null;
  onSelect: (name: string | null) => void;
  onRefresh: () => Promise<SkillLibrary | null>;
  onError: (message: string) => void;
}

export function SkillsView({
  library,
  loadError,
  selected,
  onSelect,
  onRefresh,
  onError,
}: SkillsViewProps) {
  const [working, setWorking] = useState<string | null>(null);
  const [verifyRows, setVerifyRows] = useState<Record<string, SkillVerify[]>>({});
  const [installOpen, setInstallOpen] = useState(false);
  const [removeTarget, setRemoveTarget] = useState<string | null>(null);
  const [usage, setUsage] = useState<SkillUsage[] | null>(null);
  const [usageError, setUsageError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const skill = library?.skills.find((candidate) => candidate.name === selected) ?? null;
  const skillName = skill?.name ?? null;

  const loadUsage = useCallback(async (name: string) => {
    try {
      setUsage(await invoke("skill_usage", { name }));
      setUsageError(null);
    } catch (errorValue) {
      setUsage(null);
      setUsageError(formatRuntimeError(errorValue));
    }
  }, []);

  useEffect(() => {
    setNotice(null);
    if (!skillName) {
      setUsage(null);
      setUsageError(null);
      return;
    }
    void loadUsage(skillName);
  }, [skillName, loadUsage]);

  const runVerify = async (name: string) => {
    setWorking(`verify:${name}`);
    try {
      const rows = await invoke("verify_store_skills", { names: [name] });
      setVerifyRows((previous) => ({ ...previous, [name]: rows }));
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(null);
    }
  };

  const runVerifyAll = async () => {
    setWorking("verify:all");
    try {
      const rows = await invoke("verify_store_skills", {});
      const grouped: Record<string, SkillVerify[]> = {};
      for (const row of rows) {
        const list = grouped[row.name] ?? [];
        list.push(row);
        grouped[row.name] = list;
      }
      setVerifyRows(grouped);
      setNotice(
        rows.length === 0
          ? "store 里还没有技能"
          : rows.every((row) => row.status === "ok")
            ? `已校验 ${rows.length} 个版本：内容与索引一致`
            : "有版本的内容缺失或漂移（见列表标记）；漂移需要重新安装来源才能消除",
      );
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(null);
    }
  };

  const toggleAgent = async (agentName: string, enabled: boolean) => {
    if (!skillName) return;
    setWorking(`toggle:${agentName}`);
    try {
      await invoke("set_skill_enabled", { agentName, name: skillName, enabled });
      if (enabled) {
        setNotice(`已在 ${agentName} 启用 ${skillName}：下一个会话启动时注入（测试会话会重建）`);
      }
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(null);
      await loadUsage(skillName);
      await onRefresh();
    }
  };

  const removeSkill = async (name: string) => {
    setWorking("remove");
    try {
      const removed = await invoke("remove_store_skill", { name });
      setNotice(`已从 store 移除 ${name}（${removed} 个版本）`);
      onSelect(null);
      await onRefresh();
    } catch (errorValue) {
      onError(formatRuntimeError(errorValue));
    } finally {
      setWorking(null);
    }
  };

  const busy = working !== null;
  const totalVersions = library?.skills.reduce((total, item) => total + item.versions.length, 0) ?? 0;

  return (
    <div className="screen skills-screen">
      <div className="screen-bar">
        <span className="crumb">
          技能库 · <b>{library?.storePath ?? "~/.av/skills"}</b>
        </span>
        <span className="spacer" />
        <button
          type="button"
          className="btn ghost"
          onClick={() => void runVerifyAll()}
          disabled={busy || !library || library.skills.length === 0}
          title="重算 store 内容哈希，检查缺失与漂移"
        >
          <IconCheck /> 校验全部
        </button>
        <button
          type="button"
          className="btn primary"
          onClick={() => setInstallOpen(true)}
          disabled={busy}
        >
          <IconDownload /> 安装技能
        </button>
      </div>

      <div className="skills-body">
        {notice && (
          <div className="skills-notice" role="status">
            <span>{notice}</span>
            <button type="button" className="error-dismiss" onClick={() => setNotice(null)} aria-label="关闭提示">
              ✕
            </button>
          </div>
        )}
        {!library ? (
          <div className="empty compact-empty">
            <h2>{loadError ? "读取技能 store 失败" : "正在读取全局技能 store…"}</h2>
            {loadError && <p>{loadError}</p>}
            <p>位置：~/.av/skills（AV_HOME 可覆盖）</p>
            <button type="button" className="btn ghost" onClick={() => void onRefresh()}>
              <IconRefresh /> 重试
            </button>
          </div>
        ) : skill ? (
          <>
            <div className="skills-detail-head">
              <div className="info">
                <h2 className="mono">{skill.name}</h2>
                <p className="dim">{skill.description ?? "（SKILL.md 没有可读描述）"}</p>
              </div>
              <div className="skills-actions">
                <button
                  type="button"
                  className="btn ghost"
                  disabled={busy}
                  onClick={() => void runVerify(skill.name)}
                >
                  <IconRefresh /> 校验内容
                </button>
                <button
                  type="button"
                  className="btn danger"
                  disabled={busy}
                  onClick={() => setRemoveTarget(skill.name)}
                >
                  从 store 移除
                </button>
              </div>
            </div>

            <div className="skills-meta">
              <div className="settings-summary-row">
                <span className="field-label">当前版本</span>
                <span className="mono" title={skill.currentHash}>{skill.currentHash}</span>
              </div>
              <div className="settings-summary-row">
                <span className="field-label">来源</span>
                <span className="mono" title={skill.source}>
                  {sourceLabel(skill.source, skill.sourceType)}
                </span>
              </div>
              <div className="settings-summary-row">
                <span className="field-label">来源内路径</span>
                <span className="mono">{skill.skillPath ?? "—"}</span>
              </div>
              <div className="settings-summary-row">
                <span className="field-label">安装时间</span>
                <span>{skill.installedAt ?? "—"}</span>
              </div>
            </div>

            <h3 className="skills-sub">版本（{skill.versions.length}）</h3>
            <div className="skills-versions">
              {skill.versions.map((version) => {
                const rows =
                  verifyRows[skill.name]?.filter((row) => row.contentHash === version.contentHash) ?? [];
                return (
                  <div className="skills-version" key={version.contentHash}>
                    <span className="mono">{shortHash(version.contentHash)}</span>
                    {version.contentHash === skill.currentHash && <span className="tag ok">当前</span>}
                    <span className="dim mono">
                      {version.revision
                        ? `${version.revision.slice(0, 12)}${version.reference ? ` · ${version.reference}` : ""}`
                        : "本地来源"}
                    </span>
                    <span className="spacer" />
                    {version.present ? (
                      <span className="tag ok">内容在</span>
                    ) : (
                      <span className="tag warn">内容缺失</span>
                    )}
                    {rows.map((row) => (
                      <span
                        className={toneClass(SKILL_VERIFY_TONES[row.status])}
                        key={`${row.status}:${row.actualHash ?? ""}`}
                        title={row.actualHash ? `实际哈希：${row.actualHash}` : undefined}
                      >
                        校验：{SKILL_VERIFY_LABELS[row.status]}
                      </span>
                    ))}
                  </div>
                );
              })}
            </div>

            <h3 className="skills-sub">启用它的 Agent</h3>
            {usageError && <p className="field-error">{usageError}</p>}
            {!usage && !usageError && <p className="dim">读取中…</p>}
            {usage && usage.length === 0 && <p className="dim">还没有 Agent。</p>}
            {usage && usage.length > 0 && (
              <div className="skills-usage">
                {usage.map((row) => (
                  <div className="skills-usage-row" key={row.agentName}>
                    <span className="name mono">{row.agentName}</span>
                    {row.enabled && row.status ? (
                      <span
                        className={toneClass(AGENT_SKILL_STATUS_TONES[row.status])}
                        title={row.filter ?? AGENT_SKILL_STATUS_HINTS[row.status]}
                      >
                        {AGENT_SKILL_STATUS_LABELS[row.status]}
                      </span>
                    ) : (
                      <span className="dim">未启用</span>
                    )}
                    {row.enabled && row.declaredBy && row.declaredBy !== "agent" && (
                      <span className="tag" title="声明落在项目层文件（随仓库走）">
                        项目层声明
                      </span>
                    )}
                    <span className="spacer" />
                    <button
                      type="button"
                      className={row.enabled ? "link danger" : "link"}
                      disabled={busy}
                      onClick={() => void toggleAgent(row.agentName, !row.enabled)}
                    >
                      {working === `toggle:${row.agentName}` ? "处理中…" : row.enabled ? "禁用" : "启用"}
                    </button>
                  </div>
                ))}
              </div>
            )}
            <p className="hint">
              启用会写入该 Agent 生效的最高声明层契约（`[resources.skills].use`）并生成同层
              agent.lock；禁用移除声明与锁记录，不动 store 内容。
            </p>
          </>
        ) : (
          <>
            <div className="skills-overview-head">
              <h2>全局技能 store</h2>
              <p className="dim">
                {library.storePath} · {library.skills.length} 个技能 · {totalVersions} 个版本
              </p>
            </div>
            <div className="skills-notes">
              <p>
                <b>安装 ≠ 启用</b>：安装只把内容放进 store；在 Agent 工作台的「技能」页写
                <span className="mono"> [resources.skills].use </span>
                才生效。左侧点选技能可以看到哪些 Agent 正在用它。
              </p>
              <p>
                <b>会话启动只读</b>：运行时只按同层 <span className="mono">agent.lock</span>{" "}
                解析本地内容，不联网、不静默安装；缺锁 / 缺内容一律 fail-closed。
              </p>
              <p>
                命令行的等价操作：<span className="mono">av skill install | add | sync | verify | remove</span>
              </p>
            </div>
            {library.skills.length === 0 ? (
              <div className="skills-empty">
                <p>store 里还没有技能。从 GitHub 短名（owner/repo）、git URL 或本地目录安装第一个：</p>
                <button type="button" className="btn primary" onClick={() => setInstallOpen(true)} disabled={busy}>
                  <IconDownload /> 安装技能
                </button>
              </div>
            ) : (
              <p className="dim">左侧选择一个技能查看版本、来源与启用它的 Agent。</p>
            )}
          </>
        )}
      </div>

      {installOpen && (
        <InstallSkillModal
          onClose={() => setInstallOpen(false)}
          onInstalled={async () => {
            await onRefresh();
          }}
          onError={onError}
        />
      )}
      {removeTarget && (
        <ConfirmModal
          title="从 store 移除技能"
          message={`将删除 ${removeTarget} 在 store 里的全部版本（不可恢复）。仍被 Agent 声明启用时会被拒绝 —— 先在 Agent 的技能页禁用。`}
          confirmLabel="移除"
          onConfirm={async () => {
            const name = removeTarget;
            setRemoveTarget(null);
            await removeSkill(name);
          }}
          onCancel={() => setRemoveTarget(null)}
        />
      )}
    </div>
  );
}
