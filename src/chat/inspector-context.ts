import type { ChatEntry, Chip } from "../chat-runtime";
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
}
