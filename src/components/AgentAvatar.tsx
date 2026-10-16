import { IconAgent, IconSubagent } from "../icons";

/** 名字稳定映射到色调，帮助在概览、会话与导航中辨认同一个 Agent。 */
export default function AgentAvatar({
  name,
  subagent = false,
  className = "",
}: {
  name: string;
  subagent?: boolean;
  className?: string;
}) {
  let hash = 0;
  for (const char of name)
    hash = (Math.imul(hash, 31) + char.codePointAt(0)!) | 0;
  const tone = ["blue", "teal", "amber", "rose"][(hash >>> 0) % 4];
  return (
    <span
      className={`agent-avatar avatar-${tone} ${className}`}
      aria-hidden="true"
    >
      {subagent ? <IconSubagent /> : <IconAgent />}
    </span>
  );
}
