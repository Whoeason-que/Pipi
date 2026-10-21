export interface WorkspaceEntry { path: string; name: string; kind: string; size: number }
export interface WorkspaceListing { workspace: string; path: string; entries: WorkspaceEntry[]; truncated: boolean }
export interface WorkspaceFile {
  workspace: string; path: string; size: number; version: string; kind: string; mime: string;
  content: string | null; base64: string | null; truncated: boolean;
}
export interface WorkspaceChange { path: string; status: string }
export interface WorkspaceChanges { workspace: string; isRepo: boolean; files: WorkspaceChange[] }
export interface WorkspaceDiff { path: string; base: string; patch: string; truncated: boolean }

export interface FileReference {
  workspace: string;
  path: string;
  version: string;
  startLine?: number;
  endLine?: number;
  excerpt?: string;
  comment?: string;
}

export interface ToolFile {
  path: string;
  calls: number;
  errors: number;
  operations: Array<{ id: string; name: string; patch: string | null; error: boolean }>;
}
