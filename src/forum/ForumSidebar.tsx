// 论坛侧栏：展示主题列表（帖子列表），支持搜索、状态过滤与快速新建主题。

import { useContext } from "react";
import { LayoutContext } from "../app/layout-context";
import { IconForum, IconPlus, IconRefresh } from "../icons";
import { SidebarSearch } from "../components/SidebarSearch";
import { useI18n } from "../i18n";
import { useForum } from "./forum-state";
import { formatRelativeTime } from "./time";

interface ForumSidebarProps {
  onSelect?: () => void;
}

export function ForumSidebar({ onSelect }: ForumSidebarProps) {
  const { closeNavigation } = useContext(LayoutContext);
  const { t } = useI18n();
  const {
    threads,
    filteredThreads,
    selectedId,
    composing,
    loading,
    error,
    filterText,
    statusFilter,
    setFilterText,
    setStatusFilter,
    selectThread,
    startComposing,
    loadThreads,
    clearError,
  } = useForum();

  const handleSelect = (id: string) => {
    selectThread(id);
    closeNavigation();
    onSelect?.();
  };

  const handleStartCompose = () => {
    startComposing();
    closeNavigation();
    onSelect?.();
  };

  return (
    <>
      {/* 搜索置顶：过滤框作为侧栏第一行 */}
      <SidebarSearch
        value={filterText}
        onChange={setFilterText}
        placeholder={t("forum.searchPlaceholder")}
        label={t("forum.searchLabel")}
      />

      <div className="caption sb-sec">
        <span>{t("forum.title")}</span>
        <span className="spacer" />
        <span className="caption sb-count">{threads.length}</span>
        <button
          type="button"
          className="icon-btn"
          title={t("forum.refreshThreads")}
          aria-label={t("forum.refreshThreads")}
          onClick={() => void loadThreads()}
        >
          <IconRefresh />
        </button>
        <button
          type="button"
          className="icon-btn"
          title={t("forum.newThread")}
          aria-label={t("forum.newThread")}
          onClick={handleStartCompose}
        >
          <IconPlus />
        </button>
      </div>

      <div className="forum-sidebar-filter-tabs">
        <button
          type="button"
          className={`caption forum-sidebar-tab${statusFilter === "all" ? " active" : ""}`}
          onClick={() => setStatusFilter("all")}
        >
          {t("forum.filterAll")}
        </button>
        <button
          type="button"
          className={`caption forum-sidebar-tab${statusFilter === "open" ? " active" : ""}`}
          onClick={() => setStatusFilter("open")}
        >
          {t("forum.statusOpen")}
        </button>
        <button
          type="button"
          className={`caption forum-sidebar-tab${statusFilter === "closed" ? " active" : ""}`}
          onClick={() => setStatusFilter("closed")}
        >
          {t("forum.statusClosed")}
        </button>
      </div>

      {error && (
        <div className="session pending forum-side-error">
          <span>{error}</span>
          <button type="button" className="forum-link-btn" onClick={clearError}>
            {t("forum.dismiss")}
          </button>
        </div>
      )}

      {loading && threads.length === 0 && (
        <div className="session pending">{t("forum.loadingThreads")}</div>
      )}

      {!loading && threads.length === 0 && (
        <div className="empty-state forum-sidebar-empty">
          <IconForum />
          <p>{t("forum.emptyThreads")}</p>
          <small>{t("forum.emptyThreadsHint")}</small>
        </div>
      )}

      {threads.length > 0 && filteredThreads.length === 0 && (
        <div className="session pending">{t("forum.noMatches")}</div>
      )}

      <nav className="sidebar-list forum-sidebar-list" aria-label={t("forum.threadListLabel")}>
        {filteredThreads.map((summary) => {
          const isSelected = selectedId === summary.thread.id && !composing;
          const isPinned = summary.thread.pinned;
          const isClosed = summary.thread.status === "closed";
          return (
            <button
              key={summary.thread.id}
              type="button"
              className={`search-item sidebar-item forum-sidebar-item${isSelected ? " active" : ""}`}
              aria-current={isSelected ? "true" : undefined}
              onClick={() => handleSelect(summary.thread.id)}
            >
              <div className="forum-item-row">
                <span className="truncate forum-item-title" title={summary.thread.title}>
                  {isPinned && <span className="forum-item-pin" title={t("forum.pinned")}>📌</span>}
                  {summary.thread.title}
                </span>
                {isClosed && <span className="forum-badge-closed">{t("forum.statusClosed")}</span>}
              </div>
              <div className="caption forum-item-meta">
                <span className={`forum-item-state ${isClosed ? "closed" : "open"}`} aria-hidden="true" />
                <span className="forum-item-author" title={t("forum.authorTitle", { name: summary.thread.author })}>
                  {summary.thread.author}
                </span>
                <span className="forum-item-dot">·</span>
                <span className="forum-item-count">{t("forum.postCount", { count: summary.postCount })}</span>
                {summary.lastAuthor && summary.lastAuthor !== summary.thread.author && (
                  <>
                    <span className="forum-item-dot">·</span>
                    <span className="forum-item-last" title={t("forum.lastReplyTitle", { name: summary.lastAuthor })}>{t("forum.lastBy", { name: summary.lastAuthor })}</span>
                  </>
                )}
                <span className="spacer" />
                <span className="forum-item-time">{formatRelativeTime(summary.lastActive)}</span>
              </div>
              {summary.thread.tags.length > 0 && (
                <div className="forum-item-tags">
                  {summary.thread.tags.slice(0, 3).map((tag) => (
                    <span key={tag} className="forum-tag-mini">
                      {tag}
                    </span>
                  ))}
                  {summary.thread.tags.length > 3 && (
                    <span className="forum-tag-mini">+{summary.thread.tags.length - 3}</span>
                  )}
                </div>
              )}
            </button>
          );
        })}
      </nav>
    </>
  );
}
