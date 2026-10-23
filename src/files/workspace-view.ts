import type { AgentDefinition } from "../types";

export type WorkspaceMode = "files" | "git";
export interface WorkspaceView {
  tabs: string[];
  selected: string | null;
  expanded: string[];
  query: string;
  showHidden: boolean;
}
interface Storage { getItem(key: string): string | null; setItem(key: string, value: string): void }
function windowStorage(): Storage | undefined {
  try { return typeof window === "undefined" ? undefined : window.sessionStorage; } catch { return undefined; }
}
const empty = (): WorkspaceView => ({ tabs: [], selected: null, expanded: [], query: "", showHidden: false });
const paths = (value: unknown): string[] => Array.isArray(value)
  ? [...new Set(value.filter((path): path is string => typeof path === "string" && path.length > 0 && path.length <= 4096))] : [];

/** 文件视图属于 Agent 工作区，File/Git 分别保存；会话 id 不参与身份。内容不落浏览器存储。 */
export class WorkspaceViewStore {
  readonly key: string;
  private snapshot = empty();
  private listeners = new Set<() => void>();
  private scrolls: Record<string, number> = {};
  private storage: Storage | undefined;
  constructor(agent: Pick<AgentDefinition, "name" | "workspace">, mode: WorkspaceMode, storage: Storage | undefined = windowStorage()) {
    this.storage = storage;
    this.key = `pipi-workspace-view-v2:${JSON.stringify([agent.name, agent.workspace, mode])}`;
    try {
      const saved = JSON.parse(storage?.getItem(this.key) ?? "null");
      if (!saved) return;
      const tabs = paths(saved.tabs).slice(-8);
      this.snapshot = { tabs, selected: tabs.includes(saved.selected) ? saved.selected : null,
        expanded: paths(saved.expanded).slice(-200), query: typeof saved.query === "string" ? saved.query : "", showHidden: saved.showHidden === true };
      this.scrolls = Object.fromEntries(tabs.map(path => [path, Number.isFinite(saved.scrolls?.[path]) ? Math.max(0, saved.scrolls[path]) : 0]));
    } catch { /* 缓存损坏时从空工作区视图开始。 */ }
  }
  getSnapshot = () => this.snapshot;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private update(patch: Partial<WorkspaceView>) {
    this.snapshot = { ...this.snapshot, ...patch };
    this.scrolls = Object.fromEntries(this.snapshot.tabs.map(path => [path, this.scroll(path)]));
    this.persist();
    this.listeners.forEach(listener => listener());
  }
  open(path: string) {
    if (!path) return;
    const parents = path.split("/").slice(0, -1).map((_, index, parts) => parts.slice(0, index + 1).join("/")).filter(Boolean);
    this.update({ selected: path, tabs: [...this.snapshot.tabs.filter(item => item !== path), path].slice(-8),
      expanded: [...new Set([...this.snapshot.expanded, ...parents])].slice(-200) });
  }
  select(path: string) { if (this.snapshot.tabs.includes(path)) this.update({ selected: path }); }
  close(path: string) {
    const tabs = this.snapshot.tabs.filter(item => item !== path);
    this.update({ tabs, selected: this.snapshot.selected === path ? tabs.at(-1) ?? null : this.snapshot.selected });
  }
  toggleFolder(path: string) { this.update({ expanded: this.snapshot.expanded.includes(path) ? this.snapshot.expanded.filter(item => item !== path) : [...this.snapshot.expanded, path].slice(-200) }); }
  setQuery(query: string) { this.update({ query }); }
  setHidden(showHidden: boolean) { this.update({ showHidden }); }
  scroll(path: string): number { return Number.isFinite(this.scrolls[path]) ? this.scrolls[path] : 0; }
  /** 滚动不通知 React，避免长代码每次滚动都重新渲染。 */
  rememberScroll(path: string, top: number) { this.scrolls[path] = top; this.persist(); }
  private persist() {
    try { this.storage?.setItem(this.key, JSON.stringify({ ...this.snapshot, scrolls: this.scrolls })); } catch { /* 存储不可用时保留内存状态。 */ }
  }
}
