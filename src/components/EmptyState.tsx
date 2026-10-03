
export default function EmptyState({ hasAgents }: { hasAgents: boolean }) {
  return (
    <div className="empty">
      <div className="brand">Pipi</div>
      <h1>{hasAgents ? "选择一个 Agent" : "创建你的第一个 Agent"}</h1>
      <p>
        在 Pipi 里，你维护的不是一条条会话，而是一群有名字、有工作目录、
        有技能和记忆的 Agent。会话只是 Agent 的一次运行记录，是副产品。
      </p>
    </div>
  );
}
