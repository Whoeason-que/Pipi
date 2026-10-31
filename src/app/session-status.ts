/**
 * 会话运行状态。界面不画状态点：用会话行的背景色表达
 * （蓝 = 运行、绿 = 完成、红 = 出错/中断、黄 = 重试中），
 * 文案只给无障碍标签与 title 用。
 */
export type SessionRunStatus = "running" | "retrying" | "completed" | "error";

export const SESSION_STATUS_LABEL: Record<SessionRunStatus, string> = {
  running: "正在运行",
  retrying: "正在重试",
  completed: "已完成",
  error: "出错或已中断",
};
