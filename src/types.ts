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
  provider?: ModelConfig | null;
  workspace: string | null;
  permissions: PermissionsConfig;
  mcpServers: McpServer[];
  /** 是否归入由 Agent 组合工具创建的 Subagent 分组。 */
  subagent: boolean;
  /** 自动压缩阈值：上下文占用达到模型窗口的这个百分比时压缩（默认 75）。 */
  compactThresholdPercent: number;
  /** 可选：压缩后上下文目标占本次压缩前估算 token 的百分比。 */
  compactTargetPercent?: number | null;
}

export interface ProviderConfig {
  id: string;
  name: string;
  api: ApiKind;
  baseUrl: string;
  envKey?: string | null;
  apiKey?: string | null;
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
  defaultProviderId?: string | null;
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

// ---- 技能（av 全局 store 与 Agent 契约；Rust 侧 pipi_core::skill_center）----

export type SkillSourceType = "git" | "local";

/** store 里的一条版本记录（同名多版本并存）。 */
export interface SkillVersion {
  contentHash: string;
  source: string;
  sourceType: SkillSourceType;
  reference: string | null;
  revision: string | null;
  skillPath: string | null;
  installedAt: string | null;
  updatedAt: string | null;
  /** 内容目录是否存在（哈希漂移由「校验」判定）。 */
  present: boolean;
}

/** store 里的一个技能（同名多版本聚合）。 */
export interface StoreSkill {
  name: string;
  description: string | null;
  /** 当前版本（最近安装/更新）的哈希。 */
  currentHash: string;
  source: string;
  sourceType: SkillSourceType;
  skillPath: string | null;
  installedAt: string | null;
  versions: SkillVersion[];
}

export interface SkillLibrary {
  storePath: string;
  skills: StoreSkill[];
}

export type AgentSkillStatus = "ready" | "missing" | "broken" | "unresolved" | "filtered";

/** `use` 里一个技能及其解析状态。 */
export interface AgentSkill {
  name: string;
  description: string | null;
  status: AgentSkillStatus;
  contentHash: string | null;
  source: string | null;
  revision: string | null;
  /** 被 only / exclude 过滤时的原因。 */
  filter: string | null;
}

/** store 里可启用（尚未声明）的技能。 */
export interface AvailableSkill {
  name: string;
  description: string | null;
  contentHash: string;
  present: boolean;
  source: string;
  sourceType: SkillSourceType;
  versionCount: number;
}

export interface AgentSkills {
  agentName: string;
  /** 启用/禁用会写入的契约文件（最高声明层；无声明时是 Agent 层的 agent.toml）。 */
  contractPath: string;
  lockPath: string;
  /** 当前 `use` 由谁声明："agent" | "project" | "project-local"。 */
  declaredBy: string | null;
  declaredByPath: string | null;
  agentContractExists: boolean;
  /** 声明落在项目层（覆盖 Agent 层）时为 true。 */
  declaresInProject: boolean;
  declared: AgentSkill[];
  available: AvailableSkill[];
  only: string[] | null;
  exclude: string[] | null;
  sources: string[] | null;
  lockError: string | null;
}

export type SkillVerifyStatus = "ok" | "missing" | "mismatch";

export interface SkillVerify {
  name: string;
  contentHash: string;
  status: SkillVerifyStatus;
  actualHash: string | null;
}

export interface InstalledSkillView {
  name: string;
  contentHash: string;
  source: string;
  sourceType: SkillSourceType;
  skillPath: string | null;
  revision: string | null;
  unchanged: boolean;
}

export interface SkippedSkill {
  path: string;
  reason: string;
}

export interface SkillInstallReport {
  installed: InstalledSkillView[];
  skipped: SkippedSkill[];
  log: string[];
  dryRun: boolean;
}

export interface SkillSyncReport {
  ready: string[];
  installed: string[];
  repaired: string[];
  resolved: string[];
  log: string[];
}

/** 某技能在各个 Agent 上的启用情况（技能库详情用）。 */
export interface SkillUsage {
  agentName: string;
  enabled: boolean;
  status: AgentSkillStatus | null;
  filter: string | null;
  declaredBy: string | null;
}
