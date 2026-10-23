// 论坛右栏：主题状态、参与者、标签与楼层大纲。
// 正文仍在主区；这里只做上下文与跳转。

import { useEffect, useState } from "react";
import { useRightRail, RightRail } from "../app/right-rail";
import { IconArrowDown, IconForum, IconLock } from "../icons";
import type { AgentDefinition, ForumThreadView } from "../types";
import { formatFullTime, formatRelativeTime } from "./time";

export function ForumRail({
  thread,
  loading,
  composing,
  agents,
  controller,
}: {
  thread: ForumThreadView | null;
  loading: boolean;
  composing: boolean;
  agents: AgentDefinition[];
  controller: ReturnType<typeof useRightRail>;
}) {
  const [tab, setTab] = useState("thread");
  const threadId = thread?.thread.id ?? null;

  useEffect(() => { setTab("thread"); }, [threadId, composing]);

  const jumpTo = (seq: number) => {
    document.getElementById(`post-${seq}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
  };
  const jumpLatest = () => {
    const scroller = document.querySelector(".forum-main-content");
    scroller?.scrollTo({ top: scroller.scrollHeight, behavior: "smooth" });
  };

  const participants = thread
    ? [...new Set(thread.posts.map((post) => post.author))].map((name) => ({
      name,
      agent: agents.find((agent) => agent.name === name),
      count: thread.posts.filter((post) => post.author === name).length,
    }))
    : [];

  return <RightRail
    controller={controller}
    id="forum-rail"
    label={composing ? "新建主题" : thread?.thread.title ?? "主题信息"}
    subtitle={composing ? "发布后会显示主题上下文" : thread ? `${thread.posts.length} 帖 · ${participants.length} 位参与者` : loading ? "正在加载…" : "选择左侧主题"}
    icon={<IconForum />}
    actions={thread ? <button type="button" className="icon-btn" title="跳到最新回复" aria-label="跳到最新回复" onClick={jumpLatest}>
      <IconArrowDown />
    </button> : undefined}
    tabs={thread && !composing ? [
      {
        id: "thread",
        label: "主题",
        content: <ThreadOverview thread={thread} participants={participants} jumpLatest={jumpLatest} onJump={jumpTo} />,
      },
      {
        id: "floors",
        label: "楼层",
        badge: thread.posts.length,
        content: <FloorOutline thread={thread} onJump={jumpTo} />,
      },
    ] : undefined}
    activeTab={tab}
    onTabChange={setTab}
  >
    {composing ? <div className="right-rail-empty">
      <IconForum />
      <strong>正在新建主题</strong>
      <p>标题、标签和首帖会保存为本地文件。发布后这里会显示参与者与楼层大纲。</p>
    </div> : !thread && !loading ? <div className="right-rail-empty">
      <IconForum />
      <strong>还没有打开主题</strong>
      <p>从左侧选择一个讨论主题，右栏会显示状态、参与者和快速跳转。</p>
    </div> : null}
  </RightRail>;
}

function ThreadOverview({ thread, participants, jumpLatest, onJump }: {
  thread: ForumThreadView;
  participants: Array<{ name: string; agent: AgentDefinition | undefined; count: number }>;
  jumpLatest: () => void;
  onJump: (seq: number) => void;
}) {
  const isClosed = thread.thread.status === "closed";
  return <>
    <section className="right-rail-section">
      <div className="right-rail-kv"><span>状态</span><span className={isClosed ? "tag" : "tag ok"}>
        {isClosed ? <><IconLock size={10} /> 已关闭</> : "进行中"}
      </span></div>
      <div className="right-rail-kv"><span>发起者</span><span className="mono">{thread.thread.author}</span></div>
      <div className="right-rail-kv"><span>创建时间</span><span title={formatFullTime(thread.thread.created)}>{formatRelativeTime(thread.thread.created)}</span></div>
      <div className="right-rail-kv"><span>最后活跃</span><span>{formatRelativeTime(thread.posts.at(-1)?.created ?? thread.thread.created)}</span></div>
      <div className="right-rail-kv"><span>楼层</span><span>{thread.posts.length}</span></div>
    </section>

    {thread.thread.tags.length > 0 && <section className="right-rail-section">
      <h3>标签</h3>
      <div className="right-rail-avatars">
        {thread.thread.tags.map((tag) => <span className="forum-tag-pill" key={tag}>#{tag}</span>)}
      </div>
    </section>}

    <section className="right-rail-section">
      <h3>参与者 · {participants.length}</h3>
      <div className="right-rail-list">
        {participants.map((person) => <div className="right-rail-item" key={person.name}>
          <span className="right-rail-avatar">{person.name.slice(0, 1).toUpperCase()}</span>
          <span className="right-rail-item-main">
            {person.name}
            {person.agent?.subagent && <span className="tag" style={{ marginLeft: 6 }}>Subagent</span>}
          </span>
          <small>{person.count} 帖</small>
        </div>)}
      </div>
    </section>

    <section className="right-rail-section">
      <div className="right-rail-actions-row">
        <button type="button" className="btn small" onClick={jumpLatest}>
          <IconArrowDown /> 跳到最新
        </button>
        <button type="button" className="btn small ghost" onClick={() => onJump(1)}>回到首帖</button>
      </div>
      <p className="right-rail-note">主题与楼层来自本地 Markdown 文件；关闭的主题不能追加回复。</p>
    </section>
  </>;
}

function FloorOutline({ thread, onJump }: { thread: ForumThreadView; onJump: (seq: number) => void }) {
  return <section className="right-rail-section">
    <h3>按楼层跳转</h3>
    <div className="right-rail-list">
      {thread.posts.map((post) => {
        const firstLine = post.body.split("\n").find((line) => line.trim()) ?? "";
        return <button type="button" className="right-rail-item" key={post.id} onClick={() => onJump(post.seq)}>
          <span className="mono">#{post.seq}</span>
          <span className="right-rail-item-main">{firstLine || "（空内容）"}</span>
          <small>{post.author}</small>
        </button>;
      })}
    </div>
  </section>;
}
