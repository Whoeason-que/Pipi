import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { Markdown } from "./Markdown";
import type { AgentDefinition, SessionStatsView } from "./types";

// ---- 事件负载类型（与 Rust AgentEvent 的 serde 序列化对齐）----

interface UsageView {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  totalTokens: number;
}

export interface MessageView {
  role: "user" | "assistant" | "toolResult";
  content:
    | string
    | { type: "text"; text: string }[]
    | { type: "toolCall"; id: string; name: string }[]
    | { type: "toolResultText"; text: string }[];
  usage?: UsageView;
  stopReason?: string;
  errorMessage?: string | null;
  durationMs?: number | null;
  toolName?: string;
  toolCallId?: string;
  isError?: boolean;
}

type AgentEvent =
  | { type: "agent_start" }
  | { type: "agent_end" }
  | { type: "turn_start" }
  | { type: "turn_end" }
  | { type: "message_start"; message: MessageView }
  | { type: "message_update"; message: MessageView }
  | { type: "message_end"; message: MessageView }
  | { type: "tool_execution_start"; toolCallId: string; toolName: string }
  | { type: "tool_execution_end"; toolCallId: string; toolName: string; isError: boolean };

interface Entry {
  key: string;
  role: MessageView["role"];
  text: string;
  toolName?: string;
  isError?: boolean;
  usage?: UsageView;
  durationMs?: number;
  streaming?: boolean;
}

function messageText(m: MessageView): string {
  if (typeof m.content === "string") return m.content;
  if (Array.isArray(m.content)) {
    return m.content
      .map((c) => {
        if ("text" in c) return c.text;
        if (c.type === "toolCall") return `[工具调用 ${c.name}]`;
        return "";
      })
      .join("");
  }
  return "";
}

function assistantFooter(m: MessageView): string | null {
  if (m.role !== "assistant" || !m.usage) return null;
  const parts: string[] = [];
  if (m.durationMs && m.durationMs > 0 && m.usage.output > 0) {
    const tps = m.usage.output / (m.durationMs / 1000);
    parts.push(`${tps.toFixed(1)} tok/s`);
  }
  const promptTotal = m.usage.input + m.usage.cacheRead + m.usage.cacheWrite;
  if (promptTotal > 0 && m.usage.cacheRead > 0) {
    parts.push(`缓存命中 ${((m.usage.cacheRead / promptTotal) * 100).toFixed(0)}%`);
  }
  parts.push(`${m.usage.output} tok`);
  return parts.length ? parts.join(" · ") : null;
}

interface ChatViewProps {
  agent: AgentDefinition;
  onBack: () => void;
  onError: (msg: string) => void;
}

export default function ChatView({ agent, onBack, onError }: ChatViewProps) {
  const [entries, setEntries] = useState<Entry[]>([]);
  const [stats, setStats] = useState<SessionStatsView | null>(null);
  const [input, setInput] = useState("");
  const [running, setRunning] = useState(false);
  const streamKey = useRef<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const pushEntry = useCallback((entry: Entry) => {
    setEntries((prev) => [...prev, entry]);
  }, []);

  useEffect(() => {
    // 恢复已有会话
    invoke<MessageView[]>("session_messages")
      .then((msgs) => {
        setEntries(
          msgs.map((m, i) => ({
            key: `restored-${i}`,
            role: m.role,
            text: messageText(m),
            toolName: m.toolName,
            isError: m.isError,
            usage: m.usage,
            durationMs: m.durationMs ?? undefined,
          })),
        );
      })
      .catch(() => {});
    invoke<SessionStatsView>("session_stats").then(setStats).catch(() => {});
    invoke<boolean>("session_running").then(setRunning).catch(() => {});

    const unlisten = listen<AgentEvent>("agent-event", (event) => {
      const ev = event.payload;
      switch (ev.type) {
        case "message_start": {
          if (ev.message.role === "assistant") {
            const key = `stream-${Date.now()}`;
            streamKey.current = key;
            setEntries((prev) => [
              ...prev,
              { key, role: "assistant", text: "", streaming: true },
            ]);
          }
          break;
        }
        case "message_update": {
          if (ev.message.role === "assistant" && streamKey.current) {
            const key = streamKey.current;
            setEntries((prev) =>
              prev.map((e) =>
                e.key === key ? { ...e, text: messageText(ev.message) } : e,
              ),
            );
          }
          break;
        }
        case "message_end": {
          const m = ev.message;
          if (m.role === "assistant" && streamKey.current) {
            const key = streamKey.current;
            streamKey.current = null;
            setEntries((prev) =>
              prev.map((e) =>
                e.key === key
                  ? {
                      ...e,
                      text: messageText(m),
                      usage: m.usage,
                      durationMs: m.durationMs ?? undefined,
                      streaming: false,
                    }
                  : e,
              ),
            );
          } else {
            pushEntry({
              key: `msg-${Date.now()}-${Math.random()}`,
              role: m.role,
              text: messageText(m),
              toolName: m.toolName,
              isError: m.isError,
            });
          }
          break;
        }
        case "tool_execution_start": {
          pushEntry({
            key: `tool-${ev.toolCallId}`,
            role: "toolResult",
            text: `⚙ ${ev.toolName} 运行中…`,
            toolName: ev.toolName,
          });
          break;
        }
        case "tool_execution_end": {
          setEntries((prev) =>
            prev.map((e) =>
              e.key === `tool-${ev.toolCallId}`
                ? { ...e, text: `⚙ ${ev.toolName}`, isError: ev.isError }
                : e,
            ),
          );
          break;
        }
        case "agent_end": {
          setRunning(false);
          break;
        }
      }
    });
    const unlistenStats = listen<SessionStatsView>("session-stats", (event) => {
      setStats(event.payload);
    });

    return () => {
      unlisten.then((fn) => fn());
      unlistenStats.then((fn) => fn());
    };
  }, [pushEntry]);

  // 自动滚到底
  useEffect(() => {
    listRef.current?.scrollTo({ top: listRef.current.scrollHeight });
  }, [entries]);

  const send = async () => {
    const text = input.trim();
    if (!text || running) return;
    setInput("");
    setRunning(true);
    pushEntry({ key: `user-${Date.now()}`, role: "user", text });
    try {
      await invoke("send_prompt", { agentName: agent.name, prompt: text });
    } catch (e) {
      onError(String(e));
      setRunning(false);
    }
  };

  const stop = async () => {
    try {
      await invoke("stop_run");
    } catch (e) {
      onError(String(e));
    }
  };

  const newSession = async () => {
    try {
      await invoke("new_session");
      setEntries([]);
      setStats(null);
      streamKey.current = null;
    } catch (e) {
      onError(String(e));
    }
  };

  const statsBits: string[] = [];
  if (stats) {
    if (stats.avgTps != null) statsBits.push(`${stats.avgTps.toFixed(1)} tok/s`);
    if (stats.cacheHitPct != null) statsBits.push(`缓存命中 ${stats.cacheHitPct.toFixed(0)}%`);
    if (stats.contextPercent != null)
      statsBits.push(`上下文 ${stats.contextUsed}/${stats.contextMax}（${stats.contextPercent}%）`);
    statsBits.push(`${stats.calls} 次调用`);
  }

  return (
    <div className="chat">
      <header className="chat-header">
        <button className="ghost" onClick={onBack}>
          ← 返回
        </button>
        <div className="chat-title">
          <span className="mono">{agent.name}</span>
          {statsBits.length > 0 && (
            <span className="badge neutral chat-stats" title="最近 10 次调用的滚动统计">
              {statsBits.join(" · ")}
            </span>
          )}
        </div>
        <button className="ghost" onClick={newSession} disabled={running}>
          新会话
        </button>
      </header>

      <div className="chat-list" ref={listRef}>
        {entries.length === 0 && (
          <div className="chat-welcome">
            <p>
              与 <span className="mono">{agent.name}</span> 对话。会话记录将写入
              <span className="mono"> sessions/*.jsonl</span>。
            </p>
          </div>
        )}
        {entries.map((e) => (
          <div key={e.key} className={`chat-entry role-${e.role}`}>
            <div className="chat-role">
              {e.role === "user" ? "你" : e.role === "assistant" ? agent.name : "工具"}
            </div>
            <div className={`chat-bubble ${e.isError ? "is-error" : ""}`}>
              {e.role === "assistant" ? (
                <Markdown text={e.text || (e.streaming ? "…" : "")} />
              ) : (
                <pre className="chat-text">{e.text || (e.streaming ? "…" : "")}</pre>
              )}
              {e.role === "assistant" && !e.streaming && (
                <div className="chat-foot">{assistantFooter({
                  role: "assistant",
                  content: e.text,
                  usage: e.usage,
                  durationMs: e.durationMs,
                })}</div>
              )}
              {e.streaming && <div className="chat-cursor">▍</div>}
            </div>
          </div>
        ))}
      </div>

      <footer className="chat-input">
        <textarea
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder={running ? "Agent 正在运行…" : "输入消息，Enter 发送（Shift+Enter 换行）"}
          disabled={running}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
        />
        {running ? (
          <button className="ghost stop" onClick={stop}>
            ■ 停止
          </button>
        ) : (
          <button className="primary" disabled={!input.trim()} onClick={send}>
            发送
          </button>
        )}
      </footer>
    </div>
  );
}
