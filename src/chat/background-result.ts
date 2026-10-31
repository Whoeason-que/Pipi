import type { MessageView } from "../chat-runtime";

export const BACKGROUND_RESULT_OPEN = "<background task result>";
export const BACKGROUND_RESULT_CLOSE = "</background task result>";

/** 后台任务通知的结构化视图（由宿主注入的带标签 User 消息解析而来）。 */
export interface BackgroundResultView {
  status: string;
  jobId: string;
  kind: string;
  command?: string;
  targetAgent?: string;
  exitCode?: string;
  error?: string;
  output: string;
}

/**
 * 识别并解析一条后台任务通知。
 *
 * 与压缩摘要同一套约定：回放与实时都通过消息标签识别，卡片不依赖
 * 额外的条目元数据（`open_session` 只返回消息，条目不进快照）。
 */
export function parseBackgroundResult(message: MessageView): BackgroundResultView | null {
  if (message.role !== "user" || typeof message.content !== "string") return null;
  const text = message.content.trim();
  if (!text.startsWith(BACKGROUND_RESULT_OPEN) || !text.endsWith(BACKGROUND_RESULT_CLOSE)) {
    return null;
  }
  const body = text
    .slice(BACKGROUND_RESULT_OPEN.length, text.length - BACKGROUND_RESULT_CLOSE.length)
    .replace(/^\n/, "")
    .replace(/\n$/, "");

  const fields = new Map<string, string>();
  const lines = body.split("\n");
  let index = 0;
  for (; index < lines.length; index += 1) {
    const line = lines[index];
    if (line === "output:") {
      index += 1;
      break;
    }
    const at = line.indexOf(": ");
    if (at < 0) break;
    fields.set(line.slice(0, at), line.slice(at + 2));
  }

  const status = fields.get("status");
  if (!status) return null;
  return {
    status,
    jobId: fields.get("job") ?? "",
    kind: fields.get("kind") ?? "shell",
    command: fields.get("command"),
    targetAgent: fields.get("target agent"),
    exitCode: fields.get("exit code"),
    error: fields.get("error"),
    output: lines.slice(index).join("\n").trim(),
  };
}

/** 卡片的标题行：优先命令 / 目标 Agent，兜底 job id。 */
export function backgroundResultLabel(result: BackgroundResultView): string {
  return (
    result.command ??
    (result.targetAgent ? `Agent ${result.targetAgent}` : result.jobId)
  );
}
