/**
 * 会话运行状态。界面只用会话标题的颜色表达：
 * 绿 = 运行、黄 = 重试中、红 = 出错 / 中断、蓝 = 完成且未读。
 * 完成态被读过后不再染色；文案只给无障碍标签与 title 用。
 */
export type SessionRunStatus = "running" | "retrying" | "completed" | "error";

/** 状态文案的 i18n 键；翻译在调用方进行（本模块保持无 React 依赖）。 */
export const SESSION_STATUS_KEY: Record<SessionRunStatus, string> = {
  running: "common.running",
  retrying: "app.sessionStatus.retrying",
  completed: "app.sessionStatus.completed",
  error: "app.sessionStatus.error",
};

type Translate = (path: string) => string;

export function sessionStatusLabel(status: SessionRunStatus, unread: boolean, t: Translate): string {
  return status === "completed" && unread
    ? t("app.sessionStatus.completedUnread")
    : t(SESSION_STATUS_KEY[status]);
}
