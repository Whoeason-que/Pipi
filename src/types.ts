// 与 Rust 侧类型对齐（serde camelCase）—— 前端共享类型

export type Theme = "dark" | "light";

export type ApiKind = "anthropic-messages" | "openai-completions";

export type BashMode = "allowAll" | "allowlist" | "denylist";

export type SandboxMode = "read-only" | "workspace-write" | "danger-full-access";

export const SANDBOX_LABELS: Record<SandboxMode, string> = {
  "read-only": "只读",
  "workspace-write": "工作目录内可写",
  "danger-full-access": "完全访问",
};

export const API_LABELS: Record<ApiKind, string> = {
  "anthropic-messages": "Anthropic",
  "openai-completions": "OpenAI 兼容",
};

export interface BashPermissions {
  mode: BashMode;
  commands: string[];
}

export interface PermissionsConfig {
  tools: string[];
  bash: BashPermissions;
  sandbox: SandboxMode;
}

export interface McpServer {
  name: string;
  command: string;
  args: string[];
  /** MCP server 的环境变量（M3 拉起时叠加到会话 resolved env 之上）。 */
  env: Record<string, string>;
  enabled: boolean;
}

export interface ModelConfig {
  id: string;
  name: string;
  api: ApiKind;
  baseUrl: string;
  maxTokens: number;
  contextWindow: number;
}

export interface AgentDefinition {
  name: string;
  description: string;
  model: string;
  provider: ModelConfig | null;
  workspace: string | null;
  permissions: PermissionsConfig;
  mcpServers: McpServer[];
  /** 是否归入由 Agent 组合工具创建的 Subagent 分组。 */
  subagent: boolean;
  /** 自动压缩阈值：上下文占用达到模型窗口的这个百分比时压缩（默认 75）。 */
  compactThresholdPercent: number;
}

export interface ProviderConfig {
  id: string;
  name: string;
  api: ApiKind;
  baseUrl: string;
  envKey: string | null;
  apiKey: string | null;
}

/** 压缩行为开关（Rust 侧 settings::CompactionSettings）。 */
export interface CompactionSettings {
  /** 压缩前分叉出新会话（原会话保留为完整记录）。 */
  forkBeforeCompact: boolean;
  /** 分叉后把原会话移入归档。 */
  archiveOriginal: boolean;
}

/** 请求失败重发策略（Rust 侧 settings::RetrySettings，可重试错误的分类见核心 retry 模块）。 */
export interface RetrySettings {
  /** 总尝试次数（含首次）：1 = 不重试。 */
  maxAttempts: number;
  /** 退避基数（毫秒）。 */
  baseDelayMs: number;
  /** 退避上限（毫秒）。 */
  maxDelayMs: number;
}

export interface Settings {
  theme: Theme;
  providers: ProviderConfig[];
  defaultProviderId: string | null;
  compaction: CompactionSettings;
  retry: RetrySettings;
}

// ---- 会话统计（hermes 设计，Rust stats::SessionStats 的序列化）----

export interface SessionStatsView {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  calls: number;
  avgTps?: number;
  avgLatencyS?: number;
  cacheHitPct?: number;
  contextUsed?: number;
  contextMax?: number;
  contextPercent?: number;
}

export interface SessionSummaryView {
  id: string;
  title: string;
  messageCount: number;
  startedAt: number;
  lastActive: number;
  model?: string;
}

/** child Agent 创建持久会话后，宿主要求刷新该 Agent 的列表。 */
export interface SessionChangedPayload {
  agentName: string;
  sessionId: string;
  runId: number;
}

export interface SessionInfoView {
  agentName: string;
  sessionId: string;
  /** 设置工作台的内存测试会话，不对应 sessions/*.jsonl。 */
  temporary: boolean;
  running: boolean;
  backgroundTasks?: number;
  runId?: number;
  model?: ModelConfig | null;
  isCustomModel?: boolean;
}
