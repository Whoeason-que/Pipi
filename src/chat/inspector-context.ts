import type { ChatEntry, Chip } from "../chat-runtime";
import type { SessionRuntime } from "../session-runtime";
import type { AgentDefinition, ModelConfig, SessionStatsView } from "../types";
export interface ChatInspectorContext {
  agent: AgentDefinition;
  sessionId: string | null;
  entries: ChatEntry[];
  stats: SessionStatsView | null;
  running: boolean;
  ready: boolean;
  sessionModel: ModelConfig | null;
  isCustomModel: boolean;
  blockedCount: number;
  chipDetail: Chip | null;
  chipDetailMode: "preview" | "pinned";
  onUnpinChip: () => void;
  /** 全局会话索引：Subagent 预览用它接管 child session 的实时事件。 */
  runtime: SessionRuntime;
}
