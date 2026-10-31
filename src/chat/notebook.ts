import { useEffect, useState } from "react";
import { formatRuntimeError, type ChatEntry } from "../chat-runtime";
import { invoke } from "../platform";

export interface NotebookSnapshot {
  content: string;
  loading: boolean;
  error: string | null;
}

/**
 * 会话 notebook：会话切换、或本轮出现 notebook 工具调用后重新读取。
 *
 * 只有这一个读取点：文件很小，宿主读一次比把内容随事件推来推去简单。
 * `entries` 只用来数 notebook 调用次数，不进 effect 依赖（否则流式输出
 * 每来一个 token 都会重读一次文件）。
 */
export function useNotebook(
  agentName: string,
  sessionId: string | null,
  entries: ChatEntry[],
): NotebookSnapshot {
  const [snapshot, setSnapshot] = useState<NotebookSnapshot>({
    content: "",
    loading: false,
    error: null,
  });
  const calls = entries.reduce(
    (count, entry) =>
      entry.role === "toolResult" && entry.toolName === "notebook"
        ? count + 1
        : count,
    0,
  );

  useEffect(() => {
    if (!sessionId) {
      setSnapshot({ content: "", loading: false, error: null });
      return;
    }
    let cancelled = false;
    setSnapshot((previous) => ({
      content: previous.content,
      loading: true,
      error: null,
    }));
    invoke("read_notebook", { agentName, sessionId })
      .then((content) => {
        if (!cancelled) setSnapshot({ content, loading: false, error: null });
      })
      .catch((error: unknown) => {
        if (!cancelled)
          setSnapshot({
            content: "",
            loading: false,
            error: formatRuntimeError(error),
          });
      });
    return () => {
      cancelled = true;
    };
  }, [agentName, sessionId, calls]);

  return snapshot;
}
