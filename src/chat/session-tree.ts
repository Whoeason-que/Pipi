import type { SessionSummaryView } from "../types";

/** 左栏会话仓库树的一个分支节点。 */
export interface SessionTreeNode {
  session: SessionSummaryView;
  children: SessionTreeNode[];
  /** 有 forkParentId 但父分支不在当前列表（已归档/删除）时的来源 id。 */
  missingParentId?: string;
}

/**
 * 把按 lastActive 降序的分支列表折成「分支 → 分叉子分支」树。
 *
 * - 父分支在本列表里才嵌套；父已归档/删除时保留 `missingParentId`，节点升为根。
 * - 同级保持输入顺序（Rust 已按 lastActive 降序）。
 * - 数据损坏形成环时，环上的节点会作为根各渲染一次，不会递归死循环。
 */
export function buildSessionTree(
  sessions: SessionSummaryView[],
): SessionTreeNode[] {
  const byId = new Map<string, SessionSummaryView>();
  for (const session of sessions) {
    if (!byId.has(session.id)) byId.set(session.id, session);
  }
  const children = new Map<string, SessionSummaryView[]>();
  const roots: Array<{ session: SessionSummaryView; missingParentId?: string }> = [];
  const seen = new Set<string>();
  for (const session of sessions) {
    if (seen.has(session.id)) continue;
    seen.add(session.id);
    const parentId = session.forkParentId;
    if (parentId && parentId !== session.id && byId.has(parentId)) {
      const siblings = children.get(parentId) ?? [];
      siblings.push(session);
      children.set(parentId, siblings);
    } else {
      roots.push({
        session,
        missingParentId:
          parentId && parentId !== session.id && !byId.has(parentId)
            ? parentId
            : undefined,
      });
    }
  }

  const visited = new Set<string>();
  const build = (
    session: SessionSummaryView,
    missingParentId?: string,
  ): SessionTreeNode => {
    visited.add(session.id);
    const kids: SessionTreeNode[] = [];
    for (const child of children.get(session.id) ?? []) {
      if (!visited.has(child.id)) kids.push(build(child));
    }
    return { session, children: kids, missingParentId };
  };

  const tree = roots.map((root) => build(root.session, root.missingParentId));
  // 环（正常数据不会出现）：把没访问到的节点按输入顺序补成根，保证都能看见。
  for (const session of sessions) {
    if (!visited.has(session.id)) {
      tree.push(build(session, session.forkParentId));
    }
  }
  return tree;
}
