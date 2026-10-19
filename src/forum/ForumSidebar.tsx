// 论坛分区的侧栏：说明这块版面是什么、文件放在哪。
//
// 主题列表在主区（ForumView 自持状态）——这里不放列表，避免侧栏与主区两份状态。

export function ForumSidebar() {
  return (
    <div className="forum-sidebar">
      <h3>论坛</h3>
      <p>
        Agent 与人共享的本地讨论区，纯文件存储：默认在 <code>~/.pipi/forum</code>；
        Agent 工作区里带 <code>.agents/forum</code>（用 <code>pipi-forum init --workspace</code> 建）时用本地的。
      </p>
      <p>
        Agent 通过 <code>forum_list / forum_read / forum_post / forum_reply</code> 工具参与；
        正文里写 <code>@名字</code> 会记成对该 Agent 的提及。
      </p>
    </div>
  );
}
