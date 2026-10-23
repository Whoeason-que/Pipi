// 本地论坛主区：完全展示正文与回复内容，以及新建主题与回复编辑器。
// 支持 Markdown 渲染、语法高亮代码块、@Agent 快捷提及与实时预览。

import { useRef, useState } from "react";
import { NavigationToggle } from "../app/AppNavigation";
import {
  IconAgent,
  IconClose,
  IconForum,
  IconLock,
  IconPlus,
  IconRefresh,
  IconSend,
  IconSubagent,
  IconUser,
} from "../icons";
import { Markdown } from "../Markdown";
import type { PostView } from "../types";
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

  // 新建主题表单状态
  const [newTitle, setNewTitle] = useState("");
  const [newTags, setNewTags] = useState("");
  const [newBody, setNewBody] = useState("");
  const [composeTab, setComposeTab] = useState<"edit" | "preview">("edit");
  const composeTextareaRef = useRef<HTMLTextAreaElement>(null);

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
        const quoteHint = `回复 #${post.seq} 楼：\n`;
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
    <div className="forum">
      <header className="workspace-toolbar">
        <NavigationToggle />
        <span className="toolbar-divider" />
        {composing ? (
          <span className="forum-toolbar-title">新建主题</span>
        ) : thread ? (
          <div className="forum-toolbar-thread">
            <span className="forum-toolbar-title" title={thread.thread.title}>
              {thread.thread.pinned ? "📌 " : ""}
              {thread.thread.title}
            </span>
            <span className={`forum-status-tag ${thread.thread.status}`}>
              {thread.thread.status === "open" ? "进行中" : "已关闭"}
            </span>
          </div>
        ) : (
          <span className="forum-toolbar-title">论坛</span>
        )}
        <span className="spacer" />
        {composing ? (
          <button
            type="button"
            className="forum-btn forum-btn-ghost"
            onClick={cancelComposing}
          >
            取消
          </button>
        ) : (
          <>
            <button
              type="button"
              className="icon-btn"
              title="刷新当前讨论"
              aria-label="刷新当前讨论"
              onClick={() => {
                if (selectedId) void loadThread(selectedId);
                void loadThreads(selectedId ?? undefined);
              }}
            >
              <IconRefresh />
            </button>
            <button
              type="button"
              className="forum-btn forum-btn-primary"
              onClick={startComposing}
            >
              <IconPlus />
              <span>新建主题</span>
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
            aria-label="关闭错误提示"
            onClick={clearError}
          >
            <IconClose />
          </button>
        </div>
      )}

      <div className="forum-main-content">
        {composing ? (
          <div className="forum-content-container">
            <div className="forum-compose-box">
              <div className="forum-compose-head">
                <h2>新建讨论主题</h2>
                <p className="forum-compose-desc">
                  与 Agent 和团队成员共同讨论。正文支持 GitHub 风格 Markdown，输入 <code>@名字</code> 会记为对对应 Agent 的提及。
                </p>
              </div>

              <div className="forum-field">
                <label className="forum-field-label">主题标题</label>
                <input
                  type="text"
                  className="forum-input"
                  placeholder="清晰简明的主题标题…"
                  value={newTitle}
                  onChange={(e) => setNewTitle(e.target.value)}
                  autoFocus
                />
              </div>

              <div className="forum-field">
                <label className="forum-field-label">标签（可选）</label>
                <input
                  type="text"
                  className="forum-input"
                  placeholder="标签以逗号或空格分隔，例如：bug, refactor, discussion…"
                  value={newTags}
                  onChange={(e) => setNewTags(e.target.value)}
                />
              </div>

              {mentionableAgents.length > 0 && (
                <div className="forum-mention-bar">
                  <span className="forum-mention-title">快捷提及 Agent：</span>
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
                  <label className="forum-field-label">主题正文</label>
                  <div className="forum-tabs">
                    <button
                      type="button"
                      className={`forum-tab${composeTab === "edit" ? " active" : ""}`}
                      onClick={() => setComposeTab("edit")}
                    >
                      编辑
                    </button>
                    <button
                      type="button"
                      className={`forum-tab${composeTab === "preview" ? " active" : ""}`}
                      onClick={() => setComposeTab("preview")}
                    >
                      预览
                    </button>
                  </div>
                </div>

                {composeTab === "edit" ? (
                  <textarea
                    ref={composeTextareaRef}
                    className="forum-textarea"
                    placeholder="输入正文内容（支持 Markdown 语法与 @AgentName 提及）…"
                    value={newBody}
                    onChange={(e) => setNewBody(e.target.value)}
                    rows={12}
                  />
                ) : (
                  <div className="forum-preview-box">
                    {newBody.trim() ? (
                      <Markdown text={newBody} />
                    ) : (
                      <p className="forum-preview-empty">还没有输入正文内容</p>
                    )}
                  </div>
                )}
              </div>

              <div className="forum-info-card">
                <p>
                  <strong>存储机制：</strong>纯文件存储，默认保存在 <code>~/.pipi/forum</code>；
                  若当前 Agent 工作区包含 <code>.agents/forum</code> 则优先保存在本地工作区。
                </p>
                <p>
                  <strong>Agent 参与：</strong>Agent 通过 <code>forum_list / forum_read / forum_post / forum_reply</code> 工具参与协同讨论。
                </p>
              </div>

              <div className="forum-compose-actions">
                <button
                  type="button"
                  className="forum-btn forum-btn-ghost"
                  onClick={cancelComposing}
                >
                  取消
                </button>
                <button
                  type="button"
                  className="forum-btn forum-btn-primary"
                  disabled={busy || !newTitle.trim() || !newBody.trim()}
                  onClick={() => void handlePublish()}
                >
                  <IconSend />
                  <span>{busy ? "正在发布…" : "发布主题"}</span>
                </button>
              </div>
            </div>
          </div>
        ) : thread ? (
          <div className="forum-content-container">
            {/* 主题顶部信息卡片 */}
            <div className="forum-thread-hero">
              <div className="forum-hero-top">
                <h1 className="forum-hero-title">
                  {thread.thread.pinned && <span className="forum-hero-pin" title="置顶">📌 </span>}
                  {thread.thread.title}
                </h1>
                <span className={`forum-status-pill ${thread.thread.status}`}>
                  {thread.thread.status === "open" ? (
                    <>
                      <span className="status-dot-pulse" />
                      进行中
                    </>
                  ) : (
                    <>
                      <IconLock size={12} />
                      已关闭
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
                      <span className="forum-role-tag">
                        {isHuman ? "用户" : isSubagent ? "Subagent" : "Agent"}
                      </span>
                    </div>
                  );
                })()}
                <span className="forum-dot">·</span>
                <span className="forum-hero-time" title={formatFullTime(thread.thread.created)}>
                  创建于 {formatRelativeTime(thread.thread.created)}
                </span>
                <span className="forum-dot">·</span>
                <span className="forum-hero-count">共 {thread.posts.length} 帖</span>

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
                    className={`forum-post-card${isOp ? " op" : ""}`}
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
                        {isSubagent && <span className="forum-role-tag">Subagent</span>}
                        {isOp ? (
                          <span className="forum-floor-tag op">#1 楼主</span>
                        ) : (
                          <span className="forum-floor-tag">#{post.seq}</span>
                        )}
                        {post.via && post.via !== "other" && (
                          <span className={`forum-via-tag ${post.via}`}>
                            {post.via === "tool" ? "Agent 工具" : post.via === "cli" ? "CLI" : "Web"}
                          </span>
                        )}
                      </div>

                      <div className="forum-post-actions-group">
                        <span className="forum-post-time" title={formatFullTime(post.created)}>
                          {formatRelativeTime(post.created)}
                        </span>
                        {thread.thread.status !== "closed" && (
                          <button
                            type="button"
                            className="forum-quote-btn"
                            title="回复此楼"
                            onClick={() => handleQuotePost(post)}
                          >
                            回复此楼
                          </button>
                        )}
                      </div>
                    </div>

                    {post.mentions && post.mentions.length > 0 && (
                      <div className="forum-post-mentions">
                        <span className="forum-mention-label">提及：</span>
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
                  <span>此讨论主题已被设为关闭状态，不可追加回复。</span>
                </div>
              ) : (
                <div className="forum-reply-card">
                  <div className="forum-reply-header">
                    <span className="forum-reply-title">发表回复</span>
                    <div className="forum-tabs">
                      <button
                        type="button"
                        className={`forum-tab${replyTab === "edit" ? " active" : ""}`}
                        onClick={() => setReplyTab("edit")}
                      >
                        编辑
                      </button>
                      <button
                        type="button"
                        className={`forum-tab${replyTab === "preview" ? " active" : ""}`}
                        onClick={() => setReplyTab("preview")}
                      >
                        预览
                      </button>
                    </div>
                  </div>

                  {replyTab === "edit" ? (
                    <textarea
                      ref={replyTextareaRef}
                      className="forum-reply-textarea"
                      placeholder="写下你的回复…（支持 Markdown 语法与代码块，可用 @Agent 提及对应助手）"
                      value={replyText}
                      onChange={(e) => setReplyText(e.target.value)}
                      rows={5}
                    />
                  ) : (
                    <div className="forum-reply-preview">
                      {replyText.trim() ? (
                        <Markdown text={replyText} />
                      ) : (
                        <p className="forum-preview-empty">还没有输入回复内容</p>
                      )}
                    </div>
                  )}

                  {mentionableAgents.length > 0 && (
                    <div className="forum-mention-bar">
                      <span className="forum-mention-title">提及 Agent：</span>
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
                    <span className="forum-reply-hint">
                      正文中写入 <code>@名字</code> 将通知该 Agent 参与协作
                    </span>
                    <button
                      type="button"
                      className="forum-btn forum-btn-primary"
                      disabled={busy || !replyText.trim()}
                      onClick={() => void handleSendReply()}
                    >
                      <IconSend />
                      <span>{busy ? "正在回复…" : "发送回复"}</span>
                    </button>
                  </div>
                </div>
              )}
            </div>
          </div>
        ) : threadLoading ? (
          <div className="forum-loading-container">
            <span className="forum-loading-text">正在加载主题讨论…</span>
          </div>
        ) : (
          <div className="forum-empty-view">
            <div className="forum-empty-icon">
              <IconForum />
            </div>
            <h3>欢迎使用本地论坛讨论区</h3>
            <p>
              从左侧帖子列表中选择一个主题开始阅读；或点击「新建主题」发起讨论与 Agent 协作。
            </p>
            <button
              type="button"
              className="forum-btn forum-btn-primary"
              onClick={startComposing}
            >
              <IconPlus />
              <span>新建讨论主题</span>
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
