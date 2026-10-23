import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { invoke, onHostReconnect } from "../platform";
import type { AgentDefinition } from "../types";
import type { WorkspaceChanges, WorkspaceDiff, WorkspaceFile, WorkspaceListing } from "./types";
import { WorkspaceViewStore, type WorkspaceMode } from "./workspace-view";
import { FileResourceCache } from "./resource-cache";

const resources = new FileResourceCache();

/** 返回值带请求身份：切工作区、文件或目录的第一帧也不能露出上一份内容。 */
export function useFileResource<T>(identity: string | null, revision: number, load: () => Promise<T>) {
  const [state, setState] = useState<{ identity: string | null; revision: number; value: T | null; error: string; loading: boolean; refreshing: boolean }>({ identity: null, revision: -1, value: null, error: "", loading: false, refreshing: false });
  const previous = identity === null ? null : state.identity === identity && state.value !== null ? state.value : resources.get<T>(identity);
  useEffect(() => {
    if (identity === null) return;
    let disposed = false;
    setState({ identity, revision, value: previous, error: "", loading: previous === null, refreshing: true });
    void load().then(value => {
      if (disposed) return;
      resources.set(identity, value);
      setState({ identity, revision, value, error: "", loading: false, refreshing: false });
    }).catch(error => {
      if (!disposed) setState({ identity, revision, value: previous, error: String(error), loading: false, refreshing: false });
    });
    return () => { disposed = true; };
    // identity 包含调用参数和工作区身份；load 是本次渲染的同一读取操作。
  }, [identity, revision]);
  if (identity === null) return { value: null, loading: false, refreshing: false, error: "" };
  return state.identity === identity && state.revision === revision ? state : { value: previous, loading: previous === null, refreshing: true, error: "" };
}

const noSubscribe = () => () => {};
const noView = { tabs: [], selected: null, expanded: [], query: "", showHidden: false };
const emptySnapshot = () => noView;

export type WorkspaceFileTarget = Pick<AgentDefinition, "name" | "workspace">;

export function useWorkspaceFiles(target: WorkspaceFileTarget | null, mode: WorkspaceMode, active: boolean, running: boolean) {
  const store = useMemo(() => target ? new WorkspaceViewStore(target, mode) : null, [target?.name, target?.workspace, mode]);
  const view = useSyncExternalStore(store?.subscribe ?? noSubscribe, store?.getSnapshot ?? emptySnapshot);
  const [revision, setRevision] = useState(0);
  const refresh = useCallback(() => setRevision(value => value + 1), []);
  useEffect(() => {
    if (!active) return;
    window.addEventListener("focus", refresh);
    const reconnect = onHostReconnect(refresh);
    return () => { window.removeEventListener("focus", refresh); reconnect(); };
  }, [active, refresh]);
  useEffect(() => { if (active && !running) refresh(); }, [active, running, refresh]);
  const key = active && target && store ? store.key : null;
  const lookupTarget = target?.workspace || target?.name || "";
  const rootListing = useFileResource<WorkspaceListing>(key && mode === "files" ? JSON.stringify(["listing", key, ""]) : null, revision,
    () => invoke("list_workspace_files", { agentName: lookupTarget, path: "" }));
  const changes = useFileResource<WorkspaceChanges>(key && mode === "git" ? JSON.stringify(["changes", key]) : null, revision,
    () => invoke("workspace_changes", { agentName: lookupTarget }));
  const preview = useFileResource<WorkspaceFile | WorkspaceDiff>(key && view.selected ? JSON.stringify(["preview", key, view.selected]) : null, revision,
    () => mode === "files" ? invoke("preview_workspace_file", { agentName: lookupTarget, path: view.selected! }) : invoke("workspace_diff", { agentName: lookupTarget, path: view.selected! }));
  return { target, agent: target, mode, active, store, view, revision, refresh, rootListing, changes, preview };
}

export type WorkspaceFilesState = ReturnType<typeof useWorkspaceFiles>;
