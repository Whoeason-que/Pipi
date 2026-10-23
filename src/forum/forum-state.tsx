// 论坛全局状态：在侧栏（帖子列表）与主区（正文与回复）之间共享。
// 仅在进入论坛导航或论坛视图时按需拉取数据。

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { formatRuntimeError } from "../chat-runtime";
import { invoke } from "../platform";
import type { AgentDefinition, ForumThreadView, ThreadSummaryView } from "../types";

export type ThreadFilterStatus = "all" | "open" | "closed";

export interface ForumContextValue {
  threads: ThreadSummaryView[];
  filteredThreads: ThreadSummaryView[];
  selectedId: string | null;
  thread: ForumThreadView | null;
  loading: boolean;
  threadLoading: boolean;
  busy: boolean;
  error: string | null;
  composing: boolean;
  filterText: string;
  statusFilter: ThreadFilterStatus;
  agents: AgentDefinition[];
  mentionableAgents: AgentDefinition[];
  setFilterText: (text: string) => void;
  setStatusFilter: (status: ThreadFilterStatus) => void;
  selectThread: (id: string | null) => void;
  startComposing: () => void;
  cancelComposing: () => void;
  loadThreads: (preferId?: string) => Promise<void>;
  loadThread: (id: string) => Promise<void>;
  submitPost: (title: string, body: string, tags?: string[]) => Promise<boolean>;
  submitReply: (body: string, replyTo?: string | null) => Promise<boolean>;
  clearError: () => void;
}

const ForumContext = createContext<ForumContextValue | null>(null);

export function ForumProvider({
  children,
  active = false,
  agents = [],
}: {
  children: ReactNode;
  active?: boolean;
  agents?: AgentDefinition[];
}) {
  const [threads, setThreads] = useState<ThreadSummaryView[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [thread, setThread] = useState<ForumThreadView | null>(null);
  const [loading, setLoading] = useState(false);
  const [threadLoading, setThreadLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [composing, setComposing] = useState(false);
  const [filterText, setFilterText] = useState("");
  const [statusFilter, setStatusFilter] = useState<ThreadFilterStatus>("all");
  const hasLoadedRef = useRef(false);

  const loadThreads = useCallback(async (preferId?: string) => {
    setLoading(true);
    try {
      const list = await invoke("forum_threads", { archived: false, limit: 100 });
      setThreads(list);
      setError(null);
      setSelectedId((prev) => {
        const wanted = preferId ?? prev;
        if (wanted && list.some((s) => s.thread.id === wanted)) return wanted;
        return list[0]?.thread.id ?? null;
      });
    } catch (err) {
      setError(formatRuntimeError(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const loadThread = useCallback(async (id: string) => {
    setThreadLoading(true);
    try {
      const detail = await invoke("forum_read", { threadId: id });
      setThread(detail);
      setError(null);
    } catch (err) {
      setError(formatRuntimeError(err));
    } finally {
      setThreadLoading(false);
    }
  }, []);

  useEffect(() => {
    if (active) {
      void loadThreads();
      hasLoadedRef.current = true;
    }
  }, [active, loadThreads]);

  useEffect(() => {
    if (selectedId) {
      void loadThread(selectedId);
    } else {
      setThread(null);
    }
  }, [selectedId, loadThread]);

  const selectThread = useCallback((id: string | null) => {
    setComposing(false);
    setSelectedId(id);
  }, []);

  const startComposing = useCallback(() => {
    setComposing(true);
  }, []);

  const cancelComposing = useCallback(() => {
    setComposing(false);
  }, []);

  const clearError = useCallback(() => {
    setError(null);
  }, []);

  const submitPost = useCallback(
    async (title: string, body: string, tags?: string[]): Promise<boolean> => {
      if (!title.trim() || !body.trim()) return false;
      setBusy(true);
      try {
        const created = await invoke("forum_post", {
          title: title.trim(),
          body: body.trim(),
          tags: tags?.length ? tags : null,
          mentions: null,
          author: null,
        });
        setComposing(false);
        setError(null);
        await loadThreads(created.id);
        await loadThread(created.id);
        return true;
      } catch (err) {
        setError(formatRuntimeError(err));
        return false;
      } finally {
        setBusy(false);
      }
    },
    [loadThreads, loadThread],
  );

  const submitReply = useCallback(
    async (body: string, replyTo?: string | null): Promise<boolean> => {
      if (!selectedId || !body.trim()) return false;
      setBusy(true);
      try {
        await invoke("forum_reply", {
          threadId: selectedId,
          body: body.trim(),
          replyTo: replyTo ?? null,
          mentions: null,
          author: null,
        });
        setError(null);
        await loadThread(selectedId);
        await loadThreads(selectedId);
        return true;
      } catch (err) {
        setError(formatRuntimeError(err));
        return false;
      } finally {
        setBusy(false);
      }
    },
    [selectedId, loadThread, loadThreads],
  );

  const filteredThreads = useMemo(() => {
    let list = threads;
    if (statusFilter !== "all") {
      list = list.filter((t) => t.thread.status === statusFilter);
    }
    const q = filterText.trim().toLowerCase();
    if (!q) return list;
    return list.filter((t) => {
      const matchTitle = t.thread.title.toLowerCase().includes(q);
      const matchAuthor = t.thread.author.toLowerCase().includes(q);
      const matchTags = t.thread.tags.some((tag) => tag.toLowerCase().includes(q));
      return matchTitle || matchAuthor || matchTags;
    });
  }, [threads, filterText, statusFilter]);

  const mentionableAgents = useMemo(() => agents.filter((a) => !a.subagent), [agents]);

  const value = useMemo<ForumContextValue>(
    () => ({
      threads,
      filteredThreads,
      selectedId,
      thread,
      loading,
      threadLoading,
      busy,
      error,
      composing,
      filterText,
      statusFilter,
      agents,
      mentionableAgents,
      setFilterText,
      setStatusFilter,
      selectThread,
      startComposing,
      cancelComposing,
      loadThreads,
      loadThread,
      submitPost,
      submitReply,
      clearError,
    }),
    [
      threads,
      filteredThreads,
      selectedId,
      thread,
      loading,
      threadLoading,
      busy,
      error,
      composing,
      filterText,
      statusFilter,
      agents,
      mentionableAgents,
      selectThread,
      startComposing,
      cancelComposing,
      loadThreads,
      loadThread,
      submitPost,
      submitReply,
      clearError,
    ],
  );

  return <ForumContext.Provider value={value}>{children}</ForumContext.Provider>;
}

export function useForum(): ForumContextValue {
  const context = useContext(ForumContext);
  if (!context) {
    throw new Error("useForum must be used within ForumProvider");
  }
  return context;
}
