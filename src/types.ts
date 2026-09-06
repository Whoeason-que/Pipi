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
}

export interface ProviderConfig {
  id: string;
  name: string;
  api: ApiKind;
  baseUrl: string;
  envKey: string | null;
  apiKey: string | null;
}

export interface Settings {
  theme: Theme;
  providers: ProviderConfig[];
  defaultProviderId: string | null;
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
