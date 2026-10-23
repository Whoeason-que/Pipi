// 论坛侧栏：展示主题列表（帖子列表），支持搜索、状态过滤与快速新建主题。

import { useContext } from "react";
import { LayoutContext } from "../app/layout-context";
import { IconClose, IconPlus, IconRefresh, IconSearch } from "../icons";
import { useForum } from "./forum-state";
import { formatRelativeTime } from "./time";

interface ForumSidebarProps {
  onSelect?: () => void;
}

export function ForumSidebar({ onSelect }: ForumSidebarProps) {
  const { closeNavigation } = useContext(LayoutContext);
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
      <div className="sb-sec forum-sidebar-heading">
        <span>论坛</span>
        <span className="spacer" />
        <span className="sb-count">{threads.length}</span>
        <button
          type="button"
          className="icon-btn"
          title="刷新主题列表"
          aria-label="刷新主题列表"
          onClick={() => void loadThreads()}
        >
          <IconRefresh />
        </button>
        <button
          type="button"
          className="icon-btn"
          title="新建主题"
          aria-label="新建主题"
          onClick={handleStartCompose}
        >
          <IconPlus />
        </button>
      </div>

      <div className="sb-filter">
        <div className="search">
          <IconSearch />
          <input
            value={filterText}
            onChange={(e) => setFilterText(e.target.value)}
            placeholder="搜索主题 / 作者 / 标签…"
            aria-label="搜索论坛主题"
            autoComplete="off"
            spellCheck={false}
          />
          {filterText && (
            <button
              type="button"
              className="search-clear"
              title="清空搜索"
              onClick={() => setFilterText("")}
            >
              <IconClose />
            </button>
          )}
        </div>
      </div>

      <div className="forum-sidebar-filter-tabs">
        <button
          type="button"
          className={`forum-sidebar-tab${statusFilter === "all" ? " active" : ""}`}
          onClick={() => setStatusFilter("all")}
        >
          全部
        </button>
        <button
          type="button"
          className={`forum-sidebar-tab${statusFilter === "open" ? " active" : ""}`}
          onClick={() => setStatusFilter("open")}
        >
          进行中
        </button>
        <button
          type="button"
          className={`forum-sidebar-tab${statusFilter === "closed" ? " active" : ""}`}
          onClick={() => setStatusFilter("closed")}
        >
          已关闭
        </button>
      </div>

      {error && (
        <div className="session pending forum-side-error">
          <span>{error}</span>
          <button type="button" className="forum-link-btn" onClick={clearError}>
            忽略
          </button>
        </div>
      )}

      {loading && threads.length === 0 && (
        <div className="session pending">正在读取论坛主题…</div>
      )}

      {!loading && threads.length === 0 && (
        <div className="session pending">
          还没有主题。点击上方「+」新建一个。
        </div>
      )}

      {threads.length > 0 && filteredThreads.length === 0 && (
        <div className="session pending">没有匹配的主题</div>
      )}

      <nav className="forum-sidebar-list" aria-label="论坛主题列表">
        {filteredThreads.map((summary) => {
          const isSelected = selectedId === summary.thread.id && !composing;
          const isPinned = summary.thread.pinned;
          const isClosed = summary.thread.status === "closed";
          return (
            <button
              key={summary.thread.id}
              type="button"
              className={`search-item forum-sidebar-item${isSelected ? " active" : ""}`}
              aria-current={isSelected ? "true" : undefined}
              onClick={() => handleSelect(summary.thread.id)}
            >
              <div className="forum-item-row">
                <span className="forum-item-title" title={summary.thread.title}>
                  {isPinned && <span className="forum-item-pin" title="置顶">📌</span>}
                  {summary.thread.title}
                </span>
                {isClosed && <span className="forum-badge-closed">已关闭</span>}
              </div>
              <div className="forum-item-meta">
                <span className="forum-item-author" title={`发起者：${summary.thread.author}`}>
                  {summary.thread.author}
                </span>
                <span className="forum-item-dot">·</span>
                <span className="forum-item-count">{summary.postCount} 帖</span>
                <span className="forum-item-dot">·</span>
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
