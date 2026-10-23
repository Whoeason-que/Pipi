// 技能库侧栏：已安装技能列表（选择 + 过滤）。安装 / 校验等动作在主区。

import { useMemo, useState } from "react";
import { IconClose, IconRefresh, IconSearch, IconSkill } from "../icons";
import type { SkillLibrary } from "../types";
import { shortHash, sourceLabel } from "./labels";

interface SkillsSidebarProps {
  library: SkillLibrary | null;
  selected: string | null;
  error: string | null;
  onSelect: (name: string | null) => void;
  onRefresh: () => void;
}

export function SkillsSidebar({
  library,
  selected,
  error,
  onSelect,
  onRefresh,
}: SkillsSidebarProps) {
  const [query, setQuery] = useState("");
  const skills = library?.skills ?? [];
  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return skills;
    return skills.filter(
      (skill) =>
        skill.name.toLowerCase().includes(needle)
        || (skill.description ?? "").toLowerCase().includes(needle)
        || skill.source.toLowerCase().includes(needle),
    );
  }, [skills, query]);

  return (
    <>
      <div className="sb-sec skills-heading">
        <span>技能库</span>
        <span className="spacer" />
        <span className="sb-count">{skills.length}</span>
        <button
          type="button"
          className="icon-btn"
          title="重新读取 store"
          aria-label="重新读取 store"
          onClick={onRefresh}
        >
          <IconRefresh />
        </button>
      </div>
      <div className="sb-filter">
        <div className="search">
          <IconSearch />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="搜索已安装技能…"
            aria-label="搜索已安装技能"
            autoComplete="off"
            spellCheck={false}
          />
          {query && (
            <button type="button" className="search-clear" title="清空搜索" onClick={() => setQuery("")}>
              <IconClose />
            </button>
          )}
        </div>
      </div>

      {error && <div className="session pending skills-side-error">{error}</div>}
      {!library && !error && <div className="session pending">正在读取全局 store…</div>}
      {library && skills.length === 0 && (
        <div className="session pending">store 为空：在右侧「安装技能」装入第一个技能</div>
      )}

      <nav className="skills-list" aria-label="已安装技能">
        {filtered.map((skill) => {
          const current = skill.versions.find((version) => version.contentHash === skill.currentHash);
          const missing = !current || !current.present;
          return (
            <button
              key={skill.name}
              type="button"
              className={`search-item skills-item${selected === skill.name ? " active" : ""}`}
              title={skill.description || skill.name}
              aria-current={selected === skill.name ? "true" : undefined}
              onClick={() => onSelect(selected === skill.name ? null : skill.name)}
            >
              <span className="skills-item-icon"><IconSkill /></span>
              <span className="skills-item-body">
                <span className="search-item-name">
                  {skill.name}
                  {missing && <span className="skill-dot error" title="当前版本内容缺失" />}
                </span>
                <span className="search-item-meta">
                  {sourceLabel(skill.source, skill.sourceType, current?.revision ?? null)}
                </span>
                <span className="skills-item-foot">
                  <span className="mono">{shortHash(skill.currentHash)}</span>
                  <span>{skill.versions.length} 个版本</span>
                </span>
              </span>
            </button>
          );
        })}
      </nav>
      {library && skills.length > 0 && filtered.length === 0 && (
        <div className="session pending">没有匹配的技能</div>
      )}
    </>
  );
}
