import type { SessionStatsView } from "./types";

export interface UsageView {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  totalTokens: number;
}

export type MessageContentView =
  | { type: "text"; text: string }
  | { type: "thinking"; thinking: string; thinkingSignature?: string | null }
  | { type: "toolCall"; id: string; name: string; arguments?: unknown }
  | { type: "image"; data: string; mimeType: string }
  | { type: "toolResultText"; text: string };

export interface MessageView {
  role: "user" | "assistant" | "toolResult";
  content: string | MessageContentView[];
  usage?: UsageView;
  stopReason?: string;
  errorMessage?: string | null;
  durationMs?: number | null;
  toolName?: string;
  toolCallId?: string;
  isError?: boolean;
  /** 消息创建时间（毫秒时间戳，Rust 侧 Message::timestamp）。 */
  timestamp?: number;
}

export type ToolResultContentView =
  | { type: "text"; text: string }
  | { type: "image"; data: string; mimeType: string };

export interface ToolOutputView {
  content: ToolResultContentView[];
  details?: unknown;
  terminate?: boolean;
}

export type AgentEvent =
  | { type: "agent_start" }
  | { type: "agent_end"; messages?: MessageView[] }
  | { type: "turn_start" }
  | { type: "turn_end" }
  | { type: "message_start"; message: MessageView }
  | { type: "message_update"; message: MessageView }
  | { type: "message_end"; message: MessageView }
  | { type: "tool_execution_start"; toolCallId: string; toolName: string; args?: unknown }
  | { type: "tool_execution_update"; toolCallId: string; toolName: string; partial: ToolOutputView }
  | {
      type: "tool_execution_end";
      toolCallId: string;
      toolName: string;
      result: ToolOutputView;
      isError: boolean;
    }
  | { type: "compaction_start" }
  | { type: "compaction_end"; summary: string; replaced: number };

export interface SessionEventMeta {
  agentName: string;
  sessionId: string;
  runId: number;
}

export interface AgentEventEnvelope extends SessionEventMeta {
  event: AgentEvent;
}

export type AgentEventPayload = AgentEvent | AgentEventEnvelope;

export type SessionStatsPayload = SessionStatsView | (SessionEventMeta & { stats: SessionStatsView });

export interface SessionErrorPayload extends SessionEventMeta {
  message: string;
}

/** bash 命令审批请求（RuntimeEvent::ApprovalRequest）。 */
export interface ApprovalRequestPayload extends SessionEventMeta {
  requestId: string;
  command: string;
  missing: string[];
}

export type ApprovalDecisionValue = "allow" | "always" | "deny";

export function normalizeAgentEvent(payload: AgentEventPayload): {
  event: AgentEvent;
  meta: SessionEventMeta | null;
} {
  if (typeof payload === "object" && payload !== null && "event" in payload) {
    return {
      event: payload.event,
      meta: payload,
    };
  }
  return { event: payload, meta: null };
}

export function normalizeStatsPayload(payload: SessionStatsPayload): {
  stats: SessionStatsView;
  meta: SessionEventMeta | null;
} {
  if (typeof payload === "object" && payload !== null && "stats" in payload) {
    return {
      stats: payload.stats,
      meta: payload,
    };
  }
  return { stats: payload, meta: null };
}

export function eventMatchesSession(
  meta: SessionEventMeta | null,
  agentName: string,
  identity: { sessionId: string; runId?: number } | null,
): boolean {
  if (!meta || meta.agentName !== agentName) return !meta;
  if (!identity) return true;
  if (meta.sessionId !== identity.sessionId) return false;
  return identity.runId == null || meta.runId >= identity.runId;
}

export function eventMatchesRun(
  meta: SessionEventMeta | null,
  agentName: string,
  identity: { sessionId: string; runId?: number } | null,
  settledRunId: number | null,
  ignoreEvents = false,
): boolean {
  if (ignoreEvents || !eventMatchesSession(meta, agentName, identity)) return false;
  if (!meta) return settledRunId === null;
  if (identity?.runId != null && meta.runId < identity.runId) return false;
  return meta.runId !== settledRunId;
}

export type EntryStatus = "error" | "aborted" | "tool-error";

export interface ChatEntry {
  key: string;
  role: MessageView["role"];
  text: string;
  thinking?: string;
  toolName?: string;
  toolCallId?: string;
  toolArgs?: unknown;
  toolDetails?: unknown;
  isError?: boolean;
  status?: EntryStatus;
  errorMessage?: string;
  stopReason?: string;
  usage?: UsageView;
  durationMs?: number;
  streaming?: boolean;
  toolRunning?: boolean;
  /** 消息创建时间（毫秒），实时条目取本地接收时间。 */
  timestamp?: number;
  /** 尚未被后端历史快照确认的本地/实时条目。 */
  transient?: boolean;
  /** 系统级条目（如上下文压缩标记），不走常规 assistant 渲染。 */
  kind?: "compaction";
  /** compaction 摘要正文（折叠展示）。 */
  summary?: string;
}


export interface ChatState {
  entries: ChatEntry[];
  stats: SessionStatsView | null;
  running: boolean;
  activeAssistantKey: string | null;
  hydrated: boolean;
  liveEventSeen: boolean;
  /** 进行中的压缩条目 key（compaction_start → compaction_end 之间）。 */
  activeCompactionKey: string | null;
}

export type ChatAction =
  | {
      type: "hydrate";
      messages: MessageView[];
      stats: SessionStatsView | null;
      running: boolean;
    }
  | { type: "submit_user"; key: string; text: string }
  | { type: "event"; event: AgentEvent; key: string }
  | { type: "stats"; stats: SessionStatsView }
  | { type: "running"; running: boolean }
  | { type: "reject_user"; key: string }
  | { type: "reset" };

export const INITIAL_CHAT_STATE: ChatState = {
  entries: [],
  stats: null,
  running: false,
  activeAssistantKey: null,
  hydrated: false,
  liveEventSeen: false,
  activeCompactionKey: null,
};

export function messageStatus(message: MessageView): EntryStatus | undefined {
  if (message.role === "toolResult" && message.isError) return "tool-error";
  if (message.role !== "assistant") return undefined;
  if (message.stopReason === "aborted") return "aborted";
  if (message.errorMessage || message.stopReason === "error") return "error";
  return undefined;
}

export function messageText(message: MessageView): string {
  let text = "";
  if (typeof message.content === "string") {
    text = message.content;
  } else if (Array.isArray(message.content)) {
    text = message.content
      .map((content) => {
        if (content.type === "text" || content.type === "toolResultText") return content.text;
        if (content.type === "toolCall") return `[工具调用 ${content.name}]`;
        return "";
      })
      .join("");
  }
  if (text) return text;
  if (message.errorMessage) return `⚠ ${message.errorMessage}`;
  if (message.stopReason === "aborted") return "■ 已中止";
  if (message.stopReason === "error") return "⚠ Agent 返回错误";
  return "";
}

export function messageThinking(message: MessageView): string | undefined {
  if (Array.isArray(message.content)) {
    const thinking = message.content
      .map((content) => (content.type === "thinking" ? content.thinking : ""))
      .join("");
    return thinking || undefined;
  }
  return undefined;
}

export function toolOutputText(result: ToolOutputView | undefined, fallback: string): string {
  const text = result?.content
    .map((content) => (content.type === "text" ? content.text : ""))
    .filter(Boolean)
    .join("\n");
  return text || fallback;
}

export function entryFromMessage(message: MessageView, key: string, transient = false): ChatEntry {
  const status = messageStatus(message);
  return {
    key,
    role: message.role,
    text: messageText(message),
    thinking: messageThinking(message),
    toolName: message.toolName,
    toolCallId: message.toolCallId,
    isError: message.isError || status !== undefined,
    status,
    errorMessage: message.errorMessage ?? undefined,
    stopReason: message.stopReason,
    usage: message.usage,
    durationMs: message.durationMs ?? undefined,
    timestamp: message.timestamp,
    transient,
  };
}

export function assistantFooter(message: MessageView): string | null {
  if (message.role !== "assistant" || !message.usage) return null;
  const parts: string[] = [];
  if (message.durationMs && message.durationMs > 0 && message.usage.output > 0) {
    const tokensPerSecond = message.usage.output / (message.durationMs / 1000);
    parts.push(`${tokensPerSecond.toFixed(1)} tok/s`);
  }
  const promptTotal = message.usage.input + message.usage.cacheRead + message.usage.cacheWrite;
  if (promptTotal > 0 && message.usage.cacheRead > 0) {
    parts.push(`缓存命中 ${((message.usage.cacheRead / promptTotal) * 100).toFixed(0)}%`);
  }
  parts.push(`${message.usage.output} tok`);
  return parts.length ? parts.join(" · ") : null;
}

export function formatRuntimeError(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string" && message.trim()) return message;
  }
  return "未知错误";
}

function sameMessage(left: ChatEntry, message: MessageView): boolean {
  return left.role === message.role
    && left.toolCallId === message.toolCallId
    && left.text === messageText(message)
    && left.thinking === messageThinking(message)
    && left.stopReason === message.stopReason
    && left.errorMessage === (message.errorMessage ?? undefined);
}

function updateAssistantEntry(entry: ChatEntry, message: MessageView, streaming: boolean): ChatEntry {
  const status = messageStatus(message);
  return {
    ...entry,
    text: messageText(message),
    thinking: messageThinking(message) ?? entry.thinking,
    errorMessage: message.errorMessage ?? undefined,
    stopReason: message.stopReason,
    status,
    isError: message.isError || status !== undefined,
    usage: message.usage,
    durationMs: message.durationMs ?? undefined,
    streaming,
    transient: true,
  };
}

function mergeToolMessageEntry(entry: ChatEntry, message: MessageView): ChatEntry {
  const next = entryFromMessage(message, entry.key, true);
  return {
    ...entry,
    ...next,
    toolName: next.toolName ?? entry.toolName,
    toolCallId: next.toolCallId ?? entry.toolCallId,
    toolArgs: entry.toolArgs,
    toolDetails: entry.toolDetails,
    toolRunning: false,
    transient: true,
  };
}


function mergeHydratedEntries(state: ChatState, messages: MessageView[]): ChatEntry[] {
  const merged = messages.map((message, index) => entryFromMessage(message, `restored-${index}`));
  const live = state.entries.filter((entry) => entry.transient);
  const earliestMatch = state.hydrated
    ? Math.max(0, state.entries.length - live.length)
    : 0;
  let searchEnd = merged.length - 1;
  const unmatched: ChatEntry[] = [];

  for (let index = live.length - 1; index >= 0; index -= 1) {
    const liveEntry = live[index];
    let match = -1;
    for (let candidate = searchEnd; candidate >= earliestMatch; candidate -= 1) {
      if (hydratedEntryMatches(merged[candidate], liveEntry)) {
        match = candidate;
        break;
      }
    }
    if (match >= 0) searchEnd = match - 1;
    else unmatched.push(liveEntry);
  }

  return [...merged, ...unmatched.reverse()];
}

function hydratedEntryMatches(left: ChatEntry, right: ChatEntry): boolean {
  if (left.role !== right.role) return false;
  if (left.toolCallId !== undefined || right.toolCallId !== undefined) {
    return left.toolCallId !== undefined && left.toolCallId === right.toolCallId;
  }
  return sameEntryContent(left, right);
}
function sameEntryContent(left: ChatEntry, right: ChatEntry): boolean {
  return left.role === right.role
    && left.text === right.text
    && left.stopReason === right.stopReason
    && left.errorMessage === right.errorMessage;
}
function finishRunningEntries(entries: ChatEntry[]): ChatEntry[] {
  return entries.flatMap((entry) => {
    if (entry.role === "assistant" && entry.streaming) {
      return [{
        ...entry,
        text: entry.text ? `${entry.text}\n■ 已中止` : "■ 已中止",
        isError: true,
        status: entry.status ?? "aborted",
        streaming: false,
        transient: true,
      }];
    }
    if (entry.toolRunning) {
      return [{
        ...entry,
        text: `✕ ${entry.toolName ?? "工具"} 已中止`,
        isError: true,
        status: "aborted",
        toolRunning: false,
        transient: true,
      }];
    }
    return [entry];
  });
}

export function chatReducer(state: ChatState, action: ChatAction): ChatState {
  switch (action.type) {
    case "hydrate":
      return {
        ...state,
        entries: mergeHydratedEntries(state, action.messages),
        stats: state.stats ?? action.stats,
        running: state.liveEventSeen ? state.running : action.running,
        hydrated: true,
        activeCompactionKey: null,
      };
    case "submit_user":
      return {
        ...state,
        entries: [...state.entries, {
          key: action.key,
          role: "user",
          text: action.text,
          timestamp: Date.now(),
          transient: true,
        }],
        running: true,
        liveEventSeen: true,
      };
    case "event": {
      state = state.liveEventSeen ? state : { ...state, liveEventSeen: true };
      const event = action.event;
      switch (event.type) {
        case "agent_start":
          return { ...state, running: true };
        case "agent_end":
          return {
            ...state,
            entries: finishRunningEntries(state.entries),
            running: false,
            activeAssistantKey: null,
            activeCompactionKey: null,
          };
        case "message_start": {
          const message = event.message;
          if (message.role !== "assistant") {
            if (message.role === "toolResult" && message.toolCallId) {
              const toolKey = `tool-${message.toolCallId}`;
              const toolEntry = state.entries.find((entry) => entry.key === toolKey);
              if (toolEntry) {
                return {
                  ...state,
                  entries: state.entries.map((entry) => (
                    entry.key === toolKey ? mergeToolMessageEntry(entry, message) : entry
                  )),
                };
              }
            }
            if (state.entries.some((entry) => sameMessage(entry, message))) return state;
            return {
              ...state,
              entries: [...state.entries, entryFromMessage(message, action.key, true)],
            };
          }
          const active = state.activeAssistantKey
            ? state.entries.find((entry) => entry.key === state.activeAssistantKey)
            : undefined;
          if (active?.streaming) {
            return {
              ...state,
              entries: state.entries.map((entry) => (
                entry.key === active.key ? updateAssistantEntry(entry, message, true) : entry
              )),
              running: true,
            };
          }
          return {
            ...state,
            entries: [...state.entries, {
              ...entryFromMessage(message, action.key, true),
              text: "",
              streaming: true,
            }],
            activeAssistantKey: action.key,
            running: true,
          };
        }
        case "message_update": {
          const message = event.message;
          if (message.role !== "assistant") return state;
          const key = state.activeAssistantKey ?? action.key;
          const exists = state.entries.some((entry) => entry.key === key);
          return {
            ...state,
            entries: exists
              ? state.entries.map((entry) => (
                entry.key === key ? updateAssistantEntry(entry, message, true) : entry
              ))
              : [...state.entries, {
                ...entryFromMessage(message, key, true),
                streaming: true,
              }],
            activeAssistantKey: key,
            running: true,
          };
        }
        case "message_end": {
          const message = event.message;
          if (message.role === "assistant") {
            if (!state.activeAssistantKey && state.entries.some((entry) => sameMessage(entry, message))) {
              return state;
            }
            const key = state.activeAssistantKey ?? action.key;
            const exists = state.entries.some((entry) => entry.key === key);
            return {
              ...state,
              entries: exists
                ? state.entries.map((entry) => (
                  entry.key === key ? updateAssistantEntry(entry, message, false) : entry
                ))
                : [...state.entries, entryFromMessage(message, key, true)],
              activeAssistantKey: null,
            };
          }
          if (message.role === "toolResult" && message.toolCallId) {
            const toolKey = `tool-${message.toolCallId}`;
            const toolEntry = state.entries.find((entry) => entry.key === toolKey);
            if (toolEntry) {
              return {
                ...state,
                entries: state.entries.map((entry) => (
                  entry.key === toolKey ? mergeToolMessageEntry(entry, message) : entry
                )),
              };
            }
          }
          if (state.entries.some((entry) => sameMessage(entry, message))) return state;
          return {
            ...state,
            entries: [...state.entries, entryFromMessage(message, action.key, true)],
          };
        }
        case "tool_execution_start": {
          const key = `tool-${event.toolCallId}`;
          const existing = state.entries.find((entry) => entry.key === key);
          const nextEntry: ChatEntry = {
            key,
            role: "toolResult",
            text: `⚙ ${event.toolName} 运行中…`,
            toolName: event.toolName,
            toolCallId: event.toolCallId,
            toolArgs: event.args ?? existing?.toolArgs,
            timestamp: existing?.timestamp ?? Date.now(),
            toolRunning: true,
            transient: true,
          };
          return {
            ...state,
            entries: existing
              ? state.entries.map((entry) => (entry.key === key ? { ...entry, ...nextEntry } : entry))
              : [...state.entries, nextEntry],
            running: true,
          };
        }
        case "tool_execution_update": {
          const key = `tool-${event.toolCallId}`;
          const partialText = toolOutputText(event.partial, "");
          if (!partialText) return state;
          const existing = state.entries.find((entry) => entry.key === key);
          const nextEntry: ChatEntry = {
            key,
            role: "toolResult",
            text: `⚙ ${event.toolName}\n${partialText}`,
            toolName: event.toolName,
            toolCallId: event.toolCallId,
            toolArgs: existing?.toolArgs,
            toolDetails: event.partial.details ?? existing?.toolDetails,
            toolRunning: true,
            transient: true,
          };
          return {
            ...state,
            entries: existing
              ? state.entries.map((entry) => (entry.key === key ? { ...entry, ...nextEntry } : entry))
              : [...state.entries, nextEntry],
          };
        }
        case "tool_execution_end": {
          const key = `tool-${event.toolCallId}`;
          const existing = state.entries.find((entry) => entry.key === key);
          const nextEntry: ChatEntry = {
            key,
            role: "toolResult",
            text: event.isError
              ? `✕ ${toolOutputText(event.result, "工具执行失败")}`
              : `⚙ ${event.toolName}`,
            toolName: event.toolName,
            toolCallId: event.toolCallId,
            toolArgs: existing?.toolArgs,
            toolDetails: event.result.details ?? existing?.toolDetails,
            isError: event.isError,
            status: event.isError ? "tool-error" : undefined,
            toolRunning: false,
            transient: true,
          };
          return {
            ...state,
            entries: existing
              ? state.entries.map((entry) => (entry.key === key ? { ...entry, ...nextEntry } : entry))
              : [...state.entries, nextEntry],
          };
        }

        case "compaction_start":
          return {
            ...state,
            entries: [
              ...state.entries,
              {
                key: action.key,
                role: "assistant",
                kind: "compaction",
                text: "♻ 正在压缩上下文…",
                timestamp: Date.now(),
                transient: true,
              },
            ],
            running: true,
            activeCompactionKey: action.key,
          };
        case "compaction_end": {
          const pendingKey = state.activeCompactionKey;
          const nextEntry: ChatEntry = {
            key: pendingKey ?? action.key,
            role: "assistant",
            kind: "compaction",
            text:
              event.replaced > 0
                ? `♻ 已压缩上下文：${event.replaced} 条旧消息已并入摘要`
                : "♻ 已压缩上下文",
            summary: event.summary,
            timestamp: Date.now(),
          };
          return {
            ...state,
            entries: pendingKey
              ? state.entries.map((entry) => (entry.key === pendingKey ? nextEntry : entry))
              : [...state.entries, nextEntry],
            activeCompactionKey: null,
          };
        }

        case "turn_start":
        case "turn_end":
          return state;
      }
    }
    case "stats":
      return { ...state, stats: action.stats };
    case "running":
      return { ...state, running: action.running };
    case "reject_user":
      return {
        ...state,
        entries: state.entries.filter((entry) => entry.key !== action.key),
        running: false,
      };
    case "reset":
      return { ...INITIAL_CHAT_STATE };
  }
}

let fallbackId = 0;

export function createEntryKey(prefix: string): string {
  const uuid = globalThis.crypto?.randomUUID?.();
  if (uuid) return `${prefix}-${uuid}`;
  fallbackId += 1;
  return `${prefix}-${fallbackId}`;
}
