// 本地论坛主区：完全展示正文与回复内容，以及新建主题与回复编辑器。
// 支持 Markdown 渲染、语法高亮代码块、@Agent 快捷提及与实时预览。

import { useEffect, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import { NavigationToggle } from "../app/AppNavigation";
import { useRightRail } from "../app/right-rail";
import {
  IconAgent,
  IconArrowDown,
  IconClose,
  IconForum,
  IconLock,
  IconPanelRight,
  IconPlus,
  IconRefresh,
  IconSend,
  IconSubagent,
  IconUser,
} from "../icons";
import { Markdown } from "../Markdown";
import type { PostView } from "../types";
import { useI18n } from "../i18n";
import { ForumRail } from "./ForumRail";
import { useForum } from "./forum-state";
import { formatFullTime, formatRelativeTime } from "./time";

export default function ForumView() {
  const {
    thread,
    selectedId,
    composing,
    busy,
    threadLoading,
    error,
    agents,
    mentionableAgents,
    startComposing,
    cancelComposing,
    submitPost,
    submitReply,
    loadThread,
    loadThreads,
    clearError,
  } = useForum();

  const { t } = useI18n();

  // 新建主题表单状态
  const [newTitle, setNewTitle] = useState("");
  const [newTags, setNewTags] = useState("");
  const [newBody, setNewBody] = useState("");
  const [composeTab, setComposeTab] = useState<"edit" | "preview">("edit");
  const composeTextareaRef = useRef<HTMLTextAreaElement>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const [atBottom, setAtBottom] = useState(true);
  const rail = useRightRail("forum");

  // 回帖表单状态
  const [replyText, setReplyText] = useState("");
  const [replyTab, setReplyTab] = useState<"edit" | "preview">("edit");
  const replyTextareaRef = useRef<HTMLTextAreaElement>(null);

  // 发布新主题
  const handlePublish = async () => {
    if (!newTitle.trim() || !newBody.trim() || busy) return;
    const tagList = newTags
      .split(/[,，\s]+/)
      .map((t) => t.trim())
      .filter(Boolean);
    const success = await submitPost(newTitle, newBody, tagList);
    if (success) {
      setNewTitle("");
      setNewTags("");
      setNewBody("");
      setComposeTab("edit");
    }
  };

  // 发送回复
  const handleSendReply = async () => {
    if (!replyText.trim() || busy) return;
    const success = await submitReply(replyText);
    if (success) {
      setReplyText("");
      setReplyTab("edit");
      requestAnimationFrame(() => scrollerRef.current?.scrollTo({ top: scrollerRef.current.scrollHeight, behavior: "smooth" }));
    }
  };

  const scrollToBottom = () => scrollerRef.current?.scrollTo({ top: scrollerRef.current.scrollHeight, behavior: "smooth" });
  const onScroller = () => {
    const element = scrollerRef.current;
    if (!element) return;
    setAtBottom(element.scrollHeight - element.scrollTop - element.clientHeight < 120);
  };
  useEffect(() => {
    scrollerRef.current?.scrollTo({ top: 0 });
    setAtBottom(true);
  }, [selectedId, composing]);

  const composeKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      void handlePublish();
    }
  };
  const replyKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      void handleSendReply();
    }
  };

  // 插入提及到发帖正文
  const insertComposeMention = (agentName: string) => {
    const mention = `@${agentName} `;
    setNewBody((prev) => (prev.endsWith(" ") || prev === "" ? `${prev}${mention}` : `${prev} ${mention}`));
    composeTextareaRef.current?.focus();
  };

  // 插入提及到回复正文
  const insertReplyMention = (agentName: string) => {
    const mention = `@${agentName} `;
    setReplyText((prev) => (prev.endsWith(" ") || prev === "" ? `${prev}${mention}` : `${prev} ${mention}`));
    replyTextareaRef.current?.focus();
  };

  // 引用楼层（如作者是 subagent 则不 @subagent，改以楼层提示回复）
  const handleQuotePost = (post: PostView) => {
    const isSubagent = agents.some((a) => a.name === post.author && a.subagent);
    setReplyText((prev) => {
      if (isSubagent) {
        const quoteHint = `${t("forum.replyToFloor", { seq: post.seq })}:\n`;
        if (prev.includes(quoteHint)) return prev;
        return prev ? `${prev}\n${quoteHint}` : quoteHint;
      }
      const mention = `@${post.author} `;
      if (prev.includes(mention)) return prev;
      return prev ? `${prev}\n${mention}` : mention;
    });
    setReplyTab("edit");
    replyTextareaRef.current?.focus();
  };

  return (
    <div className={`forum${rail.open ? " rail-open" : ""}`} style={{ "--right-rail-width": `${rail.width}px` } as CSSProperties}>
      <div className="content-pane forum-main-shell">
      <header className="workspace-toolbar">
        <NavigationToggle />
        <span className="toolbar-divider" />
        {composing ? (
          <span className="truncate forum-toolbar-title">{t("forum.newThread")}</span>
        ) : thread ? (
          <div className="forum-toolbar-thread">
            <span className="truncate forum-toolbar-title" title={thread.thread.title}>
              {thread.thread.pinned ? "📌 " : ""}
              {thread.thread.title}
            </span>
            <span className={`badge forum-status-tag ${thread.thread.status === "open" ? "ok" : "neutral"}`}>
              {thread.thread.status === "open" ? t("forum.statusOpen") : t("forum.statusClosed")}
            </span>
          </div>
        ) : (
          <span className="truncate forum-toolbar-title">{t("forum.title")}</span>
        )}
        <span className="spacer" />
        <button
          type="button"
          className={`icon-btn${rail.open ? " active" : ""}`}
          title={t("forum.threadInfo")}
          aria-label={t("forum.toggleInfoPanel")}
          aria-pressed={rail.open}
          onClick={rail.toggle}
        >
          <IconPanelRight />
        </button>
        {composing ? (
          <button
            type="button"
            className="btn"
            onClick={cancelComposing}
          >
            {t("common.cancel")}
          </button>
        ) : (
          <>
            <button
              type="button"
              className="icon-btn"
              title={t("forum.refreshThread")}
              aria-label={t("forum.refreshThread")}
              onClick={() => {
                if (selectedId) void loadThread(selectedId);
                void loadThreads(selectedId ?? undefined);
              }}
            >
              <IconRefresh />
            </button>
            <button
              type="button"
              className="btn primary"
              onClick={startComposing}
            >
              <IconPlus />
              <span>{t("forum.newThread")}</span>
            </button>
          </>
        )}
      </header>

      {error && (
        <div className="error" role="alert">
          <span>{error}</span>
          <button
            type="button"
            className="error-dismiss"
            aria-label={t("forum.dismissError")}
            onClick={clearError}
          >
            <IconClose />
          </button>
        </div>
      )}

      <div className="forum-main-content" ref={scrollerRef} onScroll={onScroller}>
        {composing ? (
          <div className="forum-content-container">
            <div className="surface forum-compose-box">
              <div className="forum-compose-head">
                <h2>{t("forum.newDiscussion")}</h2>
                <p className="forum-compose-desc">
                  {t("forum.composeDescription")}
                </p>
              </div>

              <div className="forum-field">
                <label className="forum-field-label" htmlFor="forum-title">{t("forum.threadTitle")}</label>
                <input
                  type="text"
                  id="forum-title"
                  className="text-input"
                  placeholder={t("forum.titlePlaceholder")}
                  value={newTitle}
                  onChange={(e) => setNewTitle(e.target.value)}
                  autoFocus
                />
              </div>

              <div className="forum-field">
                <label className="forum-field-label" htmlFor="forum-tags">{t("forum.tagsLabel")}</label>
                <input
                  type="text"
                  id="forum-tags"
                  className="text-input"
                  placeholder={t("forum.tagsPlaceholder")}
                  value={newTags}
                  onChange={(e) => setNewTags(e.target.value)}
                />
              </div>

              {mentionableAgents.length > 0 && (
                <div className="forum-mention-bar">
                  <span className="forum-mention-title">{t("forum.quickMention")}</span>
                  <div className="forum-mention-list">
                    {mentionableAgents.map((agent) => (
                      <button
                        key={agent.name}
                        type="button"
                        className="forum-mention-chip"
                        onClick={() => insertComposeMention(agent.name)}
                      >
                        @{agent.name}
                      </button>
                    ))}
                  </div>
                </div>
              )}

              <div className="forum-field">
                <div className="forum-editor-header">
                  <label className="forum-field-label" htmlFor="forum-body">{t("forum.bodyLabel")}</label>
                  <div className="forum-tabs">
                    <button
                      type="button"
                      className={`forum-tab${composeTab === "edit" ? " active" : ""}`}
                      onClick={() => setComposeTab("edit")}
                    >
                      {t("common.edit")}
                    </button>
                    <button
                      type="button"
                      className={`forum-tab${composeTab === "preview" ? " active" : ""}`}
                      onClick={() => setComposeTab("preview")}
                    >
                      {t("forum.preview")}
                    </button>
                  </div>
                </div>

                {composeTab === "edit" ? (
                  <textarea
                    ref={composeTextareaRef}
                    id="forum-body"
                    className="text-input forum-textarea"
                    placeholder={t("forum.bodyPlaceholder")}
                    value={newBody}
                    onChange={(e) => setNewBody(e.target.value)}
                    onKeyDown={composeKeyDown}
                    rows={12}
                  />
                ) : (
                  <div className="forum-preview-box">
                    {newBody.trim() ? (
                      <Markdown text={newBody} />
                    ) : (
                      <p className="forum-preview-empty">{t("forum.previewEmpty")}</p>
                    )}
                  </div>
                )}
              </div>

              <div className="forum-info-card">
                <p>
                  <strong>{t("forum.storageLabel")}</strong> {t("forum.storageBody")}
                </p>
                <p>
                  <strong>{t("forum.agentToolsLabel")}</strong> {t("forum.agentToolsBody")}
                </p>
              </div>

              <div className="forum-compose-actions">
                <span className="caption forum-kbd-hint"><kbd>Ctrl</kbd> + <kbd>Enter</kbd> {t("forum.publishShortcut")}</span>
                <span className="spacer" />
                <button
                  type="button"
                  className="btn"
                  onClick={cancelComposing}
                >
                  {t("common.cancel")}
                </button>
                <button
                  type="button"
                  className="btn primary"
                  disabled={busy || !newTitle.trim() || !newBody.trim()}
                  onClick={() => void handlePublish()}
                >
                  <IconSend />
                  <span>{busy ? t("forum.publishing") : t("forum.publishThread")}</span>
                </button>
              </div>
            </div>
          </div>
        ) : thread ? (
          <div className="forum-content-container">
            {/* 主题顶部信息卡片 */}
            <div className="surface forum-thread-hero">
              <div className="forum-hero-top">
                <h1 className="forum-hero-title">
                  {thread.thread.pinned && <span className="forum-hero-pin" title={t("forum.pinned")}>📌 </span>}
                  {thread.thread.title}
                </h1>
                <span className={`badge forum-status-pill ${thread.thread.status === "open" ? "ok" : "neutral"}`}>
                  {thread.thread.status === "open" ? (
                    <>
                      <span className="status-dot-pulse" />
                      {t("forum.statusOpen")}
                    </>
                  ) : (
                    <>
                      <IconLock size={12} />
                      {t("forum.statusClosed")}
                    </>
                  )}
                </span>
              </div>

              <div className="forum-hero-meta">
                {(() => {
                  const isHuman = thread.thread.author.toLowerCase() === "human";
                  const isSubagent = agents.some((a) => a.name === thread.thread.author && a.subagent);
                  return (
                    <div className="forum-author-badge">
                      <span
                        className={`forum-avatar-icon ${isHuman ? "human" : isSubagent ? "subagent" : "agent"}`}
                      >
                        {isHuman ? <IconUser size={14} /> : isSubagent ? <IconSubagent /> : <IconAgent />}
                      </span>
                      <span className="forum-author-name">{thread.thread.author}</span>
                      <span className="caption forum-role-tag">
                        {isHuman ? t("forum.roleUser") : isSubagent ? "Subagent" : "Agent"}
                      </span>
                    </div>
                  );
                })()}
                <span className="forum-dot">·</span>
                <span className="forum-hero-time" title={formatFullTime(thread.thread.created)}>
                  {t("forum.createdAt", { time: formatRelativeTime(thread.thread.created) })}
                </span>
                <span className="forum-dot">·</span>
                <span className="forum-hero-count">{t("forum.postCount", { count: thread.posts.length })}</span>
                <span className="forum-dot">·</span>
                <span className="forum-hero-count">{t("forum.participantCount", { count: new Set(thread.posts.map(post => post.author)).size })}</span>

                {thread.thread.tags.length > 0 && (
                  <>
                    <span className="forum-dot">·</span>
                    <div className="forum-hero-tags">
                      {thread.thread.tags.map((tag) => (
                        <span key={tag} className="forum-tag-pill">
                          #{tag}
                        </span>
                      ))}
                    </div>
                  </>
                )}
              </div>
            </div>

            {/* 帖子列表（流式卡片） */}
            <div className="forum-posts-flow">
              {thread.posts.map((post) => {
                const isOp = post.seq === 1;
                const isHuman = post.author.toLowerCase() === "human";
                const isSubagent = agents.some((a) => a.name === post.author && a.subagent);
                return (
                  <article
                    key={post.id}
                    className={`surface forum-post-card${isOp ? " op" : ""}`}
                    id={`post-${post.seq}`}
                  >
                    <div className="forum-post-card-head">
                      <div className="forum-post-author-group">
                        <span
                          className={`forum-avatar-icon ${isHuman ? "human" : isSubagent ? "subagent" : "agent"}`}
                        >
                          {isHuman ? <IconUser size={14} /> : isSubagent ? <IconSubagent /> : <IconAgent />}
                        </span>
                        <span className="forum-post-author-name">{post.author}</span>
                        {isSubagent && <span className="caption forum-role-tag">Subagent</span>}
                        {isOp ? (
                          <span className="caption forum-floor-tag op">{t("forum.floorOp")}</span>
                        ) : (
                          <span className="caption forum-floor-tag">#{post.seq}</span>
                        )}
                        {post.via && post.via !== "other" && (
                          <span className={`forum-via-tag ${post.via}`}>
                            {post.via === "tool" ? t("forum.viaTool") : post.via === "cli" ? "CLI" : "Web"}
                          </span>
                        )}
                        {post.replyTo && <span className="forum-reply-to" title={post.replyTo}>{t("forum.replyToFloor", { seq: thread.posts.find(item => item.id === post.replyTo)?.seq ?? "?" })}</span>}
                      </div>

                      <div className="forum-post-actions-group">
                        <span className="caption forum-post-time" title={formatFullTime(post.created)}>
                          {formatRelativeTime(post.created)}
                        </span>
                        {thread.thread.status !== "closed" && (
                          <button
                            type="button"
                            className="forum-quote-btn"
                            title={t("forum.replyToPost")}
                            onClick={() => handleQuotePost(post)}
                          >
                            {t("forum.replyToPost")}
                          </button>
                        )}
                      </div>
                    </div>

                    {post.mentions && post.mentions.length > 0 && (
                      <div className="forum-post-mentions">
                        <span className="forum-mention-label">{t("forum.mentions")}</span>
                        {post.mentions.map((m) => (
                          <span key={m} className="forum-mention-pill">
                            @{m}
                          </span>
                        ))}
                      </div>
                    )}

                    <div className="forum-post-markdown">
                      <Markdown text={post.body} />
                    </div>
                  </article>
                );
              })}
            </div>

            {/* 回复框 */}
            <div className="forum-reply-section">
              {thread.thread.status === "closed" ? (
                <div className="forum-closed-banner">
                  <IconLock size={15} />
                  <span>{t("forum.closedBanner")}</span>
                </div>
              ) : (
                <div className="surface forum-reply-card">
                  <div className="forum-reply-header">
                    <span className="forum-reply-title">{t("forum.replyTitle")}</span>
                    <div className="forum-tabs">
                      <button
                        type="button"
                        className={`forum-tab${replyTab === "edit" ? " active" : ""}`}
                        onClick={() => setReplyTab("edit")}
                      >
                        {t("common.edit")}
                      </button>
                      <button
                        type="button"
                        className={`forum-tab${replyTab === "preview" ? " active" : ""}`}
                        onClick={() => setReplyTab("preview")}
                      >
                        {t("forum.preview")}
                      </button>
                    </div>
                  </div>

                  {replyTab === "edit" ? (
                    <textarea
                      ref={replyTextareaRef}
                      className="text-input forum-reply-textarea"
                      placeholder={t("forum.replyPlaceholder")}
                      value={replyText}
                      onChange={(e) => setReplyText(e.target.value)}
                      onKeyDown={replyKeyDown}
                      rows={5}
                    />
                  ) : (
                    <div className="forum-reply-preview">
                      {replyText.trim() ? (
                        <Markdown text={replyText} />
                      ) : (
                        <p className="forum-preview-empty">{t("forum.previewEmpty")}</p>
                      )}
                    </div>
                  )}

                  {mentionableAgents.length > 0 && (
                    <div className="forum-mention-bar">
                      <span className="forum-mention-title">{t("forum.mentionAgent")}</span>
                      <div className="forum-mention-list">
                        {mentionableAgents.map((agent) => (
                          <button
                            key={agent.name}
                            type="button"
                            className="forum-mention-chip"
                            onClick={() => insertReplyMention(agent.name)}
                          >
                            @{agent.name}
                          </button>
                        ))}
                      </div>
                    </div>
                  )}

                  <div className="forum-reply-footer">
                    <span className="caption forum-reply-hint">
                      {t("forum.replyHint")} · <kbd>Ctrl</kbd> + <kbd>Enter</kbd> {t("forum.sendShortcut")}
                    </span>
                    <button
                      type="button"
                      className="btn primary"
                      disabled={busy || !replyText.trim()}
                      onClick={() => void handleSendReply()}
                    >
                      <IconSend />
                      <span>{busy ? t("forum.replying") : t("forum.sendReply")}</span>
                    </button>
                  </div>
                </div>
              )}
            </div>
          </div>
        ) : threadLoading ? (
          <div className="forum-loading-container">
            <span className="forum-loading-text">{t("forum.loadingThread")}</span>
          </div>
        ) : (
          <div className="empty-state forum-empty-view">
            <div className="icon-tile forum-empty-icon">
              <IconForum />
            </div>
            <h3>{t("forum.emptyTitle")}</h3>
            <p>
              {t("forum.emptyHint")}
            </p>
            <button
              type="button"
              className="btn primary"
              onClick={startComposing}
            >
              <IconPlus />
              <span>{t("forum.newDiscussion")}</span>
            </button>
          </div>
        )}
        {!composing && thread && !atBottom && (
          <button type="button" className="forum-scroll-bottom" onClick={scrollToBottom} aria-label={t("forum.scrollToLatest")}>
            <IconArrowDown />
            <span>{t("forum.latest")}</span>
          </button>
        )}
      </div>
      </div>
      <ForumRail thread={thread} loading={threadLoading} composing={composing} agents={agents} controller={rail} />
    </div>
  );
}
