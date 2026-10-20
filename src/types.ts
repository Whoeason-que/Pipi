// 与 Rust 侧类型对齐（serde camelCase）—— 前端共享类型
import type { MessageView } from "./chat-runtime";

export type Theme = string;

export type ApiKind = "anthropic-messages" | "openai-completions";

/**
 * 提供商协议的来源（Rust 侧 settings::ApiMode）：
 * `auto`（默认）= 协议由系统按模型目录与运行期实证决定；`manual` = 用户把
 * `ProviderConfig.api` 钉死，系统不改写、也不做换协议协商。
 */
export type ApiMode = "auto" | "manual";

export const API_MODE_LABELS: Record<ApiMode, string> = {
  auto: "自动",
  manual: "固定",
};

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

/**
 * 「固定模式不会自动改写」的统一提示文案：目录标注的协议与钉死的协议冲突时，
 * 模型选择器（ModelPicker）与聊天里的换模型弹窗共用同一句。
 */
export function pinnedProtocolHint(conflict: ApiKind, providerApi: ApiKind): string {
  return `目录标注该模型走 ${API_LABELS[conflict]} 协议，但此提供商的协议已固定为 `
    + `${API_LABELS[providerApi]}：固定模式不会自动改写，改为「自动」即可按模型选择。`;
}

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
  /** 系统提示词的取材范围（缺字段 = both）。 */
  systemPrompt: PromptScope;
}

/**
 * Agent 可编辑素材视图（Rust 侧 agents::AgentEditableFiles）：路径来自
 * `agent.toml` 的 `[resources]` 声明，不是硬编码目录。
 */
export interface AgentEditableFiles {
  /** 指令文件的相对路径（按契约声明顺序，界面默认打开第一个）。 */
  instructions: string[];
  /** 记忆文件的相对路径。 */
  memory: string[];
  /** 记忆目录的相对前缀（无尾斜杠），新建记忆文件时拼路径用。 */
  memoryPrefix: string;
}

/**
 * 系统提示词策略（Rust 侧 agents::PromptScope）：决定 Agent 侧 `harness/`
 * 与工作区侧的项目上下文各自是否参与组装。只影响提示词 —— 权限、沙箱与
 * 工具可用性不受影响。
 */
export type PromptScope = "both" | "agent" | "workspace" | "none";

export const PROMPT_SCOPE_LABELS: Record<PromptScope, string> = {
  both: "Pipi + 工作区",
  agent: "仅 Pipi",
  workspace: "仅工作区",
  none: "无系统提示词",
};

export interface ProviderConfig {
  id: string;
  name: string;
  /** 该条目声明的协议：`apiMode === "auto"` 时是最后回退，`manual` 时是钉死值。 */
  api: ApiKind;
  /** 协议由谁决定（默认 auto：系统按模型目录与运行期实证决定）。 */
  apiMode: ApiMode;
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

/** 旧 JSONL 会话导入 git 的结果（逐条校验、幂等；失败逐条回报）。 */
export interface SessionImportReportView {
  imported: string[];
  skipped: string[];
  failed: Array<{ sessionId: string; reason: string }>;
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

export interface SessionSnapshotView {
  info: SessionInfoView;
  messages: MessageView[];
  stats: SessionStatsView;
}

// ---- 技能（av 全局 store 与 Agent 契约；Rust 侧 pipi-venv）----

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

/** 本地论坛（纯文件版面）的前端视图。字段与 `pipi_protocol::forum` 对齐。 */
export type ThreadStatusView = "open" | "closed" | "unknown";
export type PostViaView = "tool" | "cli" | "web" | "other";

export interface ThreadView {
  id: string;
  title: string;
  author: string;
  created: number;
  tags: string[];
  status: ThreadStatusView;
  pinned: boolean;
}

export interface PostView {
  id: string;
  seq: number;
  author: string;
  created: number;
  via: PostViaView;
  replyTo?: string | null;
  mentions?: string[];
  rootId?: string | null;
  causalityDepth: number;
  body: string;
}

export interface ThreadSummaryView {
  thread: ThreadView;
  postCount: number;
  lastActive: number;
  lastAuthor: string | null;
}

export interface ForumThreadView {
  thread: ThreadView;
  posts: PostView[];
}

export interface SearchHitView {
  threadId: string;
  threadTitle: string;
  postId: string;
  postSeq: number;
  author: string;
  snippet: string;
}
export interface RemoteAccessConfig {
  port: number;
  startWithApp: boolean;
  publicUrl: string;
  tunnelTokenFile: string;
}

export interface RemoteAccessStatus {
  config: RemoteAccessConfig;
  running: boolean;
  localRunning: boolean;
  tunnelRunning: boolean;
  tunnelConnected: boolean;
  localUrl: string | null;
  connectedClients: number;
  lastError: string | null;
}
