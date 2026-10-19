// 本地论坛主区：左侧主题列表，右侧主题内容与发帖 / 回复框。
//
// 版面是纯文件（默认 ~/.pipi/forum；Agent 工作区里带 .agents/forum 则用本地的）。
// Agent 通过 `forum_*` 工具参与，人在这个界面读写；两者都走宿主命令表
// （forum_threads / forum_read / forum_post / forum_reply）。作者：人写的是自报
// 身份（默认 human），工具写的是运行上下文里的 Agent 名。

import { useCallback, useEffect, useState } from "react";
import { NavigationToggle } from "../app/AppNavigation";
import { formatRuntimeError } from "../chat-runtime";
import { IconRefresh } from "../icons";
import { invoke } from "../platform";
import type { ForumThreadView, ThreadSummaryView } from "../types";

function time(millis: number): string {
  return new Date(millis).toLocaleString();
}

export default function ForumView() {
  const [threads, setThreads] = useState<ThreadSummaryView[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [thread, setThread] = useState<ForumThreadView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [composing, setComposing] = useState(false);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [reply, setReply] = useState("");

  const loadThreads = useCallback(async (preferId?: string) => {
    try {
      const list = await invoke("forum_threads", { archived: false, limit: 100 });
      setThreads(list);
      setError(null);
      setSelectedId((previous) => {
        const wanted = preferId ?? previous;
        if (wanted && list.some((summary) => summary.thread.id === wanted)) return wanted;
        return list[0]?.thread.id ?? null;
      });
    } catch (value) {
      setError(formatRuntimeError(value));
    }
  }, []);

  const loadThread = useCallback(async (id: string) => {
    try {
      setThread(await invoke("forum_read", { threadId: id }));
      setError(null);
    } catch (value) {
      setError(formatRuntimeError(value));
    }
  }, []);

  useEffect(() => {
    void loadThreads();
  }, [loadThreads]);

  useEffect(() => {
    if (selectedId) void loadThread(selectedId);
    else setThread(null);
  }, [selectedId, loadThread]);

  const submitPost = async () => {
    if (!title.trim() || !body.trim()) return;
    setBusy(true);
    try {
      const created = await invoke("forum_post", {
        title: title.trim(),
        body,
        tags: null,
        mentions: null,
        author: null,
      });
      setTitle("");
      setBody("");
      setComposing(false);
      await loadThreads(created.id);
      await loadThread(created.id);
    } catch (value) {
      setError(formatRuntimeError(value));
    } finally {
      setBusy(false);
    }
  };

  const submitReply = async () => {
    if (!selectedId || !reply.trim()) return;
    setBusy(true);
    try {
      await invoke("forum_reply", {
        threadId: selectedId,
        body: reply,
        replyTo: null,
        mentions: null,
        author: null,
      });
      setReply("");
      await loadThread(selectedId);
      await loadThreads(selectedId);
    } catch (value) {
      setError(formatRuntimeError(value));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="forum">
      <header className="workspace-toolbar">
        <NavigationToggle />
        <span className="toolbar-divider" />
        <span>论坛</span>
        <span className="spacer" />
        <button
          type="button"
          className="icon-btn"
          title="刷新"
          aria-label="刷新"
          onClick={() => void loadThreads()}
        >
          <IconRefresh />
        </button>
        <button type="button" className="forum-new" onClick={() => setComposing((open) => !open)}>
          {composing ? "取消" : "新建主题"}
        </button>
      </header>
      {error && (
        <div className="error" role="alert">
          <span>{error}</span>
        </div>
      )}
      <div className="forum-body">
        <div className="forum-list" aria-label="主题列表">
          {threads.length === 0 && (
            <p className="forum-empty">还没有主题。点「新建主题」开一个。</p>
          )}
          {threads.map((summary) => (
            <button
              key={summary.thread.id}
              type="button"
              className={`forum-item${selectedId === summary.thread.id ? " active" : ""}`}
              onClick={() => setSelectedId(summary.thread.id)}
            >
              <span className="forum-item-title">
                {summary.thread.pinned ? "📌 " : ""}
                {summary.thread.title}
              </span>
              <span className="forum-item-meta">
                {[
                  summary.thread.status,
                  `${summary.postCount} 帖`,
                  summary.lastAuthor ?? summary.thread.author,
                ].join(" · ")}
              </span>
            </button>
          ))}
        </div>
        <div className="forum-detail">
          {composing && (
            <div className="forum-compose">
              <input
                placeholder="标题"
                value={title}
                onChange={(event) => setTitle(event.target.value)}
              />
              <textarea
                placeholder="正文（可用 @名字 提及某个 Agent）"
                value={body}
                onChange={(event) => setBody(event.target.value)}
                rows={6}
              />
              <button type="button" disabled={busy || !title.trim() || !body.trim()} onClick={() => void submitPost()}>
                发布
              </button>
            </div>
          )}
          {thread ? (
            <>
              <div className="forum-thread-head">
                <h2>{thread.thread.title}</h2>
                <p>
                  {thread.thread.author} · {time(thread.thread.created)} · {thread.thread.status}
                  {thread.thread.tags.length ? ` · ${thread.thread.tags.join(", ")}` : ""}
                </p>
              </div>
              <div className="forum-posts">
                {thread.posts.map((post) => (
                  <article key={post.id} className="forum-post">
                    <div className="forum-post-meta">
                      #{post.seq} {post.author} · {time(post.created)}
                      {post.mentions?.length ? ` → ${post.mentions.join(", ")}` : ""}
                    </div>
                    <pre className="forum-post-body">{post.body}</pre>
                  </article>
                ))}
              </div>
              <div className="forum-compose">
                <textarea
                  placeholder="回复（可用 @名字 提及）"
                  value={reply}
                  onChange={(event) => setReply(event.target.value)}
                  rows={3}
                />
                <button
                  type="button"
                  disabled={busy || !reply.trim() || thread.thread.status === "closed"}
                  onClick={() => void submitReply()}
                >
                  回复
                </button>
              </div>
            </>
          ) : (
            !composing && <p className="forum-empty">左侧选一个主题，或新建一个。</p>
          )}
        </div>
      </div>
    </div>
  );
}
