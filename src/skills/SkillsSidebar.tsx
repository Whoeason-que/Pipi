// 技能库侧栏：已安装技能列表（选择 + 过滤）。安装 / 校验等动作在主区。

import { useMemo, useState } from "react";
import { useI18n } from "../i18n";
import { IconRefresh, IconSkill } from "../icons";
import { SidebarSearch } from "../components/SidebarSearch";
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
  const { t } = useI18n();
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
      {/* 搜索置顶：过滤框作为侧栏第一行 */}
      <SidebarSearch
        value={query}
        onChange={setQuery}
        placeholder={t("skills.searchPlaceholder")}
        label={t("skills.searchInstalled")}
      />

      <div className="caption sb-sec skills-heading">
        <span>{t("nav.skills")}</span>
        <span className="spacer" />
        <span className="caption sb-count">{skills.length}</span>
        <button
          type="button"
          className="icon-btn"
          title={t("skills.reloadStore")}
          aria-label={t("skills.reloadStore")}
          onClick={onRefresh}
        >
          <IconRefresh />
        </button>
      </div>

      {error && <div className="session pending skills-side-error">{error}</div>}
      {!library && !error && <div className="session pending">{t("skills.loadingStore")}</div>}
      {library && skills.length === 0 && (
        <div className="session pending">{t("skills.storeEmptySidebar")}</div>
      )}

      <nav className="skills-list" aria-label={t("skills.installed")}>
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
              <span className="icon-tile skills-item-icon"><IconSkill /></span>
              <span className="skills-item-body">
                <span className="truncate search-item-name">
                  {skill.name}
                  {missing && <span className="skill-dot error" title={t("skills.currentVersionMissing")} />}
                </span>
                <span className="truncate caption search-item-meta">
                  {sourceLabel(t, skill.source, skill.sourceType, current?.revision ?? null)}
                </span>
                <span className="caption skills-item-foot">
                  <span className="mono">{shortHash(skill.currentHash)}</span>
                  <span>{t("skills.versionCount", { count: skill.versions.length })}</span>
                </span>
              </span>
            </button>
          );
        })}
      </nav>
      {library && skills.length > 0 && filtered.length === 0 && (
        <div className="session pending">{t("skills.noMatches")}</div>
      )}
    </>
  );
}
