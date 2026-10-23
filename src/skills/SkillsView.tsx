// 技能库主区：store 概览（安装 / 全部校验）与技能卡片网格。
// 选中技能的版本、来源与启用情况放在右侧面板（见 SkillRail）。

import { useEffect, useState, type CSSProperties } from "react";
import { NavigationToggle } from "../app/AppNavigation";
import { useRightRail } from "../app/right-rail";
import { formatRuntimeError } from "../chat-runtime";
import ConfirmModal from "../components/ConfirmModal";
import { IconCheck, IconDownload, IconPanelRight, IconRefresh, IconSkill } from "../icons";
import { invoke } from "../platform";
import type { SkillLibrary, SkillVerify, StoreSkill } from "../types";
import InstallSkillModal from "./InstallSkillModal";
import { SkillRail } from "./SkillRail";
import { shortHash, sourceLabel } from "./labels";

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
  const [notice, setNotice] = useState<string | null>(null);
  const rail = useRightRail("skills");

  const skill = library?.skills.find((candidate) => candidate.name === selected) ?? null;
  const skillName = skill?.name ?? null;

  // 从左侧栏选择技能时也直接展开右栏；用户手动关闭后不会反复弹开。
  useEffect(() => { if (selected) rail.setOpen(true); }, [selected]);
  // 没有选中项时不在启动阶段留下一块空面板。
  useEffect(() => { if (!selected) rail.setOpen(false); }, []);

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

  const chooseSkill = (name: string) => {
    onSelect(name);
    rail.setOpen(true);
  };

  const busy = working !== null;
  const totalVersions = library?.skills.reduce((total, item) => total + item.versions.length, 0) ?? 0;
  const missing = library?.skills.filter((item) => !currentVersion(item)?.present).length ?? 0;

  return (
    <div className="screen skills-screen" style={{ "--right-rail-width": `${rail.width}px` } as CSSProperties}>
      <div className="skills-main">
      <div className="screen-bar">
        <NavigationToggle />
        <span className="crumb">
          技能库 · <b>{library?.storePath ?? "~/.av/skills"}</b>
        </span>
        <span className="spacer" />
        <button
          type="button"
          className={`icon-btn${rail.open ? " active" : ""}`}
          title="技能详情"
          aria-label="切换技能详情面板"
          aria-pressed={rail.open}
          onClick={rail.toggle}
        >
          <IconPanelRight />
        </button>
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
            <div className="empty compact-empty skills-loading">
              <span className="skills-hero-icon"><IconSkill /></span>
              <h2>{loadError ? "读取技能 store 失败" : "正在读取全局技能 store…"}</h2>
              {loadError && <p>{loadError}</p>}
              <p>位置：~/.av/skills（AV_HOME 可覆盖）</p>
              <button type="button" className="btn ghost" onClick={() => void onRefresh()}>
                <IconRefresh /> 重试
              </button>
            </div>
          ) : (
            <>
              <header className="skills-hero skills-overview-hero">
                <span className="skills-hero-icon"><IconSkill /></span>
                <div className="skills-hero-copy">
                  <h2>全局技能 store</h2>
                  <p className="mono">{library.storePath}</p>
                  <p>安装把能力包放进本地 store；在 Agent 工作台启用后，才会进入下一个会话的提示词。</p>
                </div>
                <button type="button" className="btn primary" onClick={() => setInstallOpen(true)} disabled={busy}>
                  <IconDownload /> 安装技能
                </button>
              </header>

              <div className="skills-stat-grid overview">
                <div className="skills-stat">
                  <span>已安装技能</span>
                  <strong>{library.skills.length}</strong>
                </div>
                <div className="skills-stat">
                  <span>内容版本</span>
                  <strong>{totalVersions}</strong>
                </div>
                <div className={`skills-stat${missing ? " warn" : ""}`}>
                  <span>内容异常</span>
                  <strong>{missing}</strong>
                </div>
                <div className="skills-stat">
                  <span>存储方式</span>
                  <strong className="mono">本地文件</strong>
                </div>
              </div>

              {library.skills.length === 0 ? (
                <div className="skills-empty">
                  <span className="skills-hero-icon"><IconDownload /></span>
                  <h2>store 里还没有技能</h2>
                  <p>从 GitHub 短名（owner/repo）、git URL 或本地目录安装第一个技能。</p>
                  <button type="button" className="btn primary" onClick={() => setInstallOpen(true)} disabled={busy}>
                    <IconDownload /> 安装技能
                  </button>
                </div>
              ) : (
                <section className="skills-panel">
                  <div className="skills-panel-head">
                    <h3 className="skills-sub">已安装技能 <span>{library.skills.length}</span></h3>
                    <span className="dim">
                      {skillName ? `正在查看 ${skillName}，详情在右侧` : "选择卡片查看版本、来源与启用它的 Agent"}
                    </span>
                  </div>
                  <div className="skills-cards">
                    {library.skills.map((item) => (
                      <SkillCard key={item.name} skill={item} active={item.name === skillName}
                        onSelect={() => chooseSkill(item.name)} />
                    ))}
                  </div>
                </section>
              )}

              <details className="skills-notes">
                <summary>store 与注入规则</summary>
                <p>
                  <b>安装 ≠ 启用</b>：安装只把内容放进 store；在 Agent 工作台的「技能」页写
                  <span className="mono"> [resources.skills].use </span>
                  才生效。
                </p>
                <p>
                  <b>会话启动只读</b>：运行时只按同层 <span className="mono">agent.lock</span>{" "}
                  解析本地内容，不联网、不静默安装；缺锁 / 缺内容一律 fail-closed。
                </p>
                <p>
                  命令行的等价操作：<span className="mono">av skill install | add | sync | verify | remove</span>
                </p>
              </details>
            </>
          )}
        </div>
      </div>

      <SkillRail
          controller={rail}
          skill={skill}
          verifyRows={skill ? verifyRows[skill.name] ?? [] : []}
          verifyBusy={working === `verify:${skillName}`}
          onVerify={() => { if (skill) void runVerify(skill.name); }}
          onRequestRemove={setRemoveTarget}
          onError={onError}
        />

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

function currentVersion(skill: StoreSkill) {
  return skill.versions.find((version) => version.contentHash === skill.currentHash);
}

function SkillCard({ skill, active, onSelect }: { skill: StoreSkill; active: boolean; onSelect: () => void }) {
  const current = currentVersion(skill);
  const healthy = !!current?.present;
  return (
    <button type="button" className={`skills-card${active ? " active" : ""}`} aria-pressed={active} onClick={onSelect}>
      <div className="skills-card-head">
        <span className="skills-card-icon"><IconSkill /></span>
        <span className="skills-card-name mono">{skill.name}</span>
        <span className={`skill-dot ${healthy ? "ok" : "error"}`} title={healthy ? "当前版本内容在" : "当前版本内容缺失"} />
      </div>
      <p className="skills-card-desc">{skill.description ?? "（SKILL.md 没有可读描述）"}</p>
      <div className="skills-card-foot">
        <span className="mono" title={skill.source}>{sourceLabel(skill.source, skill.sourceType)}</span>
        <span>{skill.versions.length} 个版本</span>
        <span className="mono">{shortHash(skill.currentHash)}</span>
      </div>
    </button>
  );
}
