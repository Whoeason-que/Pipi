/** notebook 里计划清单的解析（纯函数，无依赖：前端测试直接 import）。 */

export interface PlanProgress {
  done: number;
  total: number;
}

/** 统计 Markdown 任务清单（`- [ ]` / `- [x]`），给右栏一个廉价的进度。 */
export function parsePlan(markdown: string): PlanProgress {
  const done = markdown.match(/^\s*[-*]\s+\[[xX]\]/gm)?.length ?? 0;
  const open = markdown.match(/^\s*[-*]\s+\[\s\]/gm)?.length ?? 0;
  return { done, total: done + open };
}
