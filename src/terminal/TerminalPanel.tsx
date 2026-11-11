import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { useI18n } from "../i18n";
import type { TerminalSnapshot } from "../ipc";
import { invoke, onHostReconnect } from "../platform";
import { readStored, writeStored } from "../storage";
import {
  closeTerminalSession,
  getTerminalSession,
  markTerminalOpened,
  reconcileTerminalSessions,
  subscribeTerminalSessions,
  terminalOpenedBefore,
  terminalSession,
  terminalSessionRevision,
} from "./terminal-sessions";

const MAX_TABS = 8;

function tabStorageKey(workspace: string): string {
  return `pipi-terminal-tab:${workspace}`;
}

function shellName(shell: string): string {
  return shell.split(/[\\/]/).pop() || shell || "shell";
}

/**
 * One workspace's terminals. `variant="page"` is the navigation page,
 * `variant="rail"` is the chat inspector tab; both share the module-level
 * registry, so they show the same shells.
 */
export function TerminalPanel({
  agentName,
  workspace,
  active,
  variant,
}: {
  agentName: string;
  /** Explicit workspace for the navigation page; omitted in the chat rail. */
  workspace?: string | null;
  active: boolean;
  variant: "page" | "rail";
}) {
  const { t } = useI18n();
  const hostRef = useRef<HTMLDivElement>(null);
  const terminalsRef = useRef<TerminalSnapshot[]>([]);
  const [terminals, setTerminals] = useState<TerminalSnapshot[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const requestRef = useRef(0);
  const openingRef = useRef(false);
  terminalsRef.current = terminals;

  // Re-render tab titles/status when the registry notices a change.
  useSyncExternalStore(subscribeTerminalSessions, terminalSessionRevision);

  const create = useCallback(
    async (cols = 120, rows = 32): Promise<TerminalSnapshot | null> => {
      if (openingRef.current) return null;
      openingRef.current = true;
      // Invalidate any list request that started before this open; it would
      // otherwise overwrite the new tab with a pre-open snapshot.
      requestRef.current += 1;
      setBusy(true);
      try {
        const snapshot = await invoke("open_terminal", {
          agentName,
          workspace: workspace ?? null,
          cols,
          rows,
        });
        markTerminalOpened(snapshot.workspace);
        requestRef.current += 1;
        setTerminals((previous) =>
          previous.some((item) => item.terminalId === snapshot.terminalId)
            ? previous
            : [...previous, snapshot],
        );
        setSelected(snapshot.terminalId);
        setError("");
        return snapshot;
      } catch (reason) {
        setError(String(reason));
        return null;
      } finally {
        openingRef.current = false;
        setBusy(false);
      }
    },
    [agentName, workspace],
  );

  const refresh = useCallback(
    async (openIfEmpty: boolean) => {
      const request = ++requestRef.current;
      try {
        const list = await invoke("list_terminals", {
          agentName,
          workspace: workspace ?? null,
        });
        if (request !== requestRef.current) return;
        setTerminals(list);
        const canonical = list[0]?.workspace ?? workspace ?? null;
        if (canonical) {
          reconcileTerminalSessions(
            canonical,
            new Set(list.map((item) => item.terminalId)),
          );
        }
        setSelected((previous) => {
          if (previous && list.some((item) => item.terminalId === previous)) {
            return previous;
          }
          const stored = canonical ? readStored(tabStorageKey(canonical), "") : "";
          if (stored && list.some((item) => item.terminalId === stored)) {
            return stored;
          }
          return list[0]?.terminalId ?? null;
        });
        if (openIfEmpty && list.length === 0) {
          const key =
            canonical ?? (workspace ? `ws:${workspace}` : `agent:${agentName}`);
          // First visit creates one shell; after the user closes every tab the
          // key is already marked, so an empty panel stays empty until "+".
          if (!terminalOpenedBefore(key)) {
            markTerminalOpened(key);
            void create();
          }
        }
      } catch (reason) {
        if (request === requestRef.current) setError(String(reason));
      }
    },
    [agentName, create, workspace],
  );

  useEffect(() => {
    if (!active) return;
    void refresh(true);
    const refreshOnFocus = () => void refresh(false);
    window.addEventListener("focus", refreshOnFocus);
    const dispose = onHostReconnect(refreshOnFocus);
    return () => {
      window.removeEventListener("focus", refreshOnFocus);
      dispose();
    };
  }, [active, refresh]);

  const selectedSnapshot =
    terminals.find((item) => item.terminalId === selected) ?? null;
  const selectedId = selectedSnapshot?.terminalId ?? null;
  const selectedWorkspace = selectedSnapshot?.workspace ?? null;

  useEffect(() => {
    const host = hostRef.current;
    if (!active || !host || !selectedId || !selectedWorkspace) return;
    const snapshot = terminalsRef.current.find(
      (item) => item.terminalId === selectedId,
    );
    if (!snapshot) return;
    const target = { agentName, workspace: selectedWorkspace };
    const session = terminalSession(snapshot, target);
    session.attach(host, target);
    return () => session.detach();
  }, [active, agentName, selectedId, selectedWorkspace]);

  useEffect(() => {
    if (!selected) return;
    const snapshot = terminals.find((item) => item.terminalId === selected);
    if (snapshot) writeStored(tabStorageKey(snapshot.workspace), selected);
  }, [selected, terminals]);

  const closeTab = useCallback(
    async (terminalId: string) => {
      const snapshot = terminals.find((item) => item.terminalId === terminalId);
      if (!snapshot) return;
      requestRef.current += 1;
      const index = terminals.findIndex((item) => item.terminalId === terminalId);
      const next = terminals[index + 1] ?? terminals[index - 1] ?? null;
      setTerminals((previous) =>
        previous.filter((item) => item.terminalId !== terminalId),
      );
      setSelected((previous) =>
        previous === terminalId ? (next?.terminalId ?? null) : previous,
      );
      await closeTerminalSession(terminalId, {
        agentName,
        workspace: snapshot.workspace,
      });
    },
    [agentName, terminals],
  );

  const selectedSession = selectedId ? getTerminalSession(selectedId) : undefined;
  const running =
    selectedSession?.running ?? selectedSnapshot?.running ?? false;
  const exitCode =
    selectedSession?.exitCode ?? selectedSnapshot?.exitCode ?? null;
  const displayError = error || selectedSession?.error || "";

  const add = () => {
    if (terminals.length >= MAX_TABS) return;
    void create(
      selectedSnapshot?.cols ?? 120,
      selectedSnapshot?.rows ?? 32,
    );
  };

  const restart = () => {
    if (!selectedSnapshot) return;
    const cols = selectedSession?.terminal.cols || selectedSnapshot.cols;
    const rows = selectedSession?.terminal.rows || selectedSnapshot.rows;
    void (async () => {
      await closeTab(selectedSnapshot.terminalId);
      await create(cols, rows);
    })();
  };

  return (
    <div className={`terminal-panel variant-${variant}`}>
      <div className="terminal-tabs" role="tablist" aria-label={t("terminal.title")}>
        {terminals.map((snapshot, index) => {
          const session = getTerminalSession(snapshot.terminalId);
          const title =
            session?.title?.trim() || `${shellName(snapshot.shell)} ${index + 1}`;
          const alive = session?.running ?? snapshot.running;
          return (
            <div
              key={snapshot.terminalId}
              className={`terminal-tab${snapshot.terminalId === selectedId ? " active" : ""}`}
            >
              <button
                type="button"
                role="tab"
                aria-selected={snapshot.terminalId === selectedId}
                title={snapshot.workspace}
                onClick={() => setSelected(snapshot.terminalId)}
              >
                <span
                  className={`terminal-tab-dot${alive ? " running" : ""}`}
                  aria-hidden="true"
                />
                <span className="terminal-tab-title">{title}</span>
              </button>
              <button
                type="button"
                className="terminal-tab-close"
                aria-label={t("terminal.close")}
                title={t("terminal.close")}
                onClick={() => void closeTab(snapshot.terminalId)}
              >
                ×
              </button>
            </div>
          );
        })}
        <button
          type="button"
          className="terminal-new"
          disabled={terminals.length >= MAX_TABS || busy}
          aria-label={t("terminal.new")}
          title={
            terminals.length >= MAX_TABS
              ? t("terminal.maxTabs")
              : t("terminal.new")
          }
          onClick={add}
        >
          +
        </button>
      </div>

      {selectedSnapshot ? (
        <>
          <div className="terminal-toolbar">
            <span
              className={`terminal-status${running ? " running" : ""}`}
              role="status"
            >
              {running
                ? t("terminal.running")
                : exitCode !== null
                  ? t("terminal.exitedWithCode", { code: exitCode })
                  : t("terminal.exited")}
            </span>
            <span
              className="terminal-path"
              title={selectedSnapshot.workspace}
            >
              {selectedSnapshot.workspace}
            </span>
            {selectedSession && (
              <button
                type="button"
                className="btn small ghost"
                onClick={() => selectedSession.clear()}
              >
                {t("terminal.clear")}
              </button>
            )}
            <button
              type="button"
              className="btn small ghost"
              onClick={restart}
              disabled={busy}
            >
              {t("terminal.restart")}
            </button>
          </div>
          {displayError && (
            <p className="file-notice" role="alert">
              {displayError}
            </p>
          )}
          <div
            className="terminal-surface"
            ref={hostRef}
            aria-label={t("terminal.title")}
            onMouseDown={() => selectedSession?.terminal.focus()}
          />
        </>
      ) : (
        <div className="empty-state terminal-empty">
          <strong>{t("terminal.emptyTitle")}</strong>
          <p>{t("terminal.emptyHint")}</p>
          <button
            type="button"
            className="btn"
            onClick={add}
            disabled={busy}
          >
            {t("terminal.new")}
          </button>
        </div>
      )}
    </div>
  );
}
