// 论坛右栏：主题状态、参与者、标签与楼层大纲。
// 正文仍在主区；这里只做上下文与跳转。

import { useEffect, useState } from "react";
import { useRightRail, RightRail } from "../app/right-rail";
import { RightRailEmpty } from "../components/RightRailEmpty";
import { IconArrowDown, IconForum, IconLock } from "../icons";
import type { AgentDefinition, ForumThreadView } from "../types";
import { useI18n } from "../i18n";
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
  const { t } = useI18n();
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
    label={composing ? t("forum.newThread") : thread?.thread.title ?? t("forum.threadInfo")}
    subtitle={composing ? t("forum.railComposeSubtitle") : thread ? t("forum.railThreadSubtitle", { posts: thread.posts.length, participants: participants.length }) : loading ? t("common.loading") : t("forum.selectThread")}
    icon={<IconForum />}
    actions={thread ? <button type="button" className="icon-btn" title={t("forum.jumpToLatest")} aria-label={t("forum.jumpToLatest")} onClick={jumpLatest}>
      <IconArrowDown />
    </button> : undefined}
    tabs={thread && !composing ? [
      {
        id: "thread",
        label: t("forum.threadTab"),
        content: <ThreadOverview thread={thread} participants={participants} jumpLatest={jumpLatest} onJump={jumpTo} />,
      },
      {
        id: "floors",
        label: t("forum.floorsTab"),
        badge: thread.posts.length,
        content: <FloorOutline thread={thread} onJump={jumpTo} />,
      },
    ] : undefined}
    activeTab={tab}
    onTabChange={setTab}
  >
    {composing ? <RightRailEmpty
      icon={<IconForum />}
      title={t("forum.composingTitle")}
      description={t("forum.composingDescription")}
    /> : !thread && !loading ? <RightRailEmpty
      icon={<IconForum />}
      title={t("forum.noThreadTitle")}
      description={t("forum.noThreadDescription")}
    /> : null}
  </RightRail>;
}

function ThreadOverview({ thread, participants, jumpLatest, onJump }: {
  thread: ForumThreadView;
  participants: Array<{ name: string; agent: AgentDefinition | undefined; count: number }>;
  jumpLatest: () => void;
  onJump: (seq: number) => void;
}) {
  const { t } = useI18n();
  const isClosed = thread.thread.status === "closed";
  return <>
    <section className="right-rail-section">
      <div className="right-rail-kv"><span>{t("forum.status")}</span><span className={isClosed ? "tag" : "tag ok"}>
        {isClosed ? <><IconLock size={10} /> {t("forum.statusClosed")}</> : t("forum.statusOpen")}
      </span></div>
      <div className="right-rail-kv"><span>{t("forum.author")}</span><span className="mono">{thread.thread.author}</span></div>
      <div className="right-rail-kv"><span>{t("forum.created")}</span><span title={formatFullTime(thread.thread.created)}>{formatRelativeTime(thread.thread.created)}</span></div>
      <div className="right-rail-kv"><span>{t("forum.lastActive")}</span><span>{formatRelativeTime(thread.posts.at(-1)?.created ?? thread.thread.created)}</span></div>
      <div className="right-rail-kv"><span>{t("forum.floors")}</span><span>{thread.posts.length}</span></div>
    </section>

    {thread.thread.tags.length > 0 && <section className="right-rail-section">
      <h3>{t("forum.tags")}</h3>
      <div className="right-rail-avatars">
        {thread.thread.tags.map((tag) => <span className="forum-tag-pill" key={tag}>#{tag}</span>)}
      </div>
    </section>}

    <section className="right-rail-section">
      <h3>{t("forum.participants", { count: participants.length })}</h3>
      <div className="right-rail-list">
        {participants.map((person) => <div className="right-rail-item" key={person.name}>
          <span className="right-rail-avatar">{person.name.slice(0, 1).toUpperCase()}</span>
          <span className="truncate right-rail-item-main">
            {person.name}
            {person.agent?.subagent && <span className="tag" style={{ marginLeft: 6 }}>Subagent</span>}
          </span>
          <small>{t("forum.postCount", { count: person.count })}</small>
        </div>)}
      </div>
    </section>

    <section className="right-rail-section">
      <div className="right-rail-actions-row">
        <button type="button" className="btn small" onClick={jumpLatest}>
          <IconArrowDown /> {t("forum.jumpToLatestShort")}
        </button>
        <button type="button" className="btn small ghost" onClick={() => onJump(1)}>{t("forum.backToFirst")}</button>
      </div>
      <p className="caption right-rail-note">{t("forum.railNote")}</p>
    </section>
  </>;
}

function FloorOutline({ thread, onJump }: { thread: ForumThreadView; onJump: (seq: number) => void }) {
  const { t } = useI18n();
  return <section className="right-rail-section">
    <h3>{t("forum.jumpByFloor")}</h3>
    <div className="right-rail-list">
      {thread.posts.map((post) => {
        const firstLine = post.body.split("\n").find((line) => line.trim()) ?? "";
        return <button type="button" className="right-rail-item" key={post.id} onClick={() => onJump(post.seq)}>
          <span className="mono">#{post.seq}</span>
          <span className="truncate right-rail-item-main">{firstLine || t("forum.emptyPost")}</span>
          <small>{post.author}</small>
        </button>;
      })}
    </div>
  </section>;
}
