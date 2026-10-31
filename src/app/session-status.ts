/**
 * 会话运行状态。界面只用会话标题的颜色表达：
 * 绿 = 运行、黄 = 重试中、红 = 出错 / 中断、蓝 = 完成且未读。
 * 完成态被读过后不再染色；文案只给无障碍标签与 title 用。
 */
export type SessionRunStatus = "running" | "retrying" | "completed" | "error";

export const SESSION_STATUS_LABEL: Record<SessionRunStatus, string> = {
  running: "正在运行",
  retrying: "正在重试",
  completed: "已完成",
  error: "出错或已中断",
};

export function sessionStatusLabel(status: SessionRunStatus, unread: boolean): string {
  return status === "completed" && unread ? "已完成 · 未读" : SESSION_STATUS_LABEL[status];
}
