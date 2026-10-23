// 论坛时间格式化助手：提供相对时间（列表）与绝对完整时间（详情/提示）。

export function formatFullTime(millis: number): string {
  if (!millis) return "";
  const d = new Date(millis);
  const pad = (n: number) => n.toString().padStart(2, "0");
  const year = d.getFullYear();
  const month = pad(d.getMonth() + 1);
  const day = pad(d.getDate());
  const hours = pad(d.getHours());
  const mins = pad(d.getMinutes());
  const secs = pad(d.getSeconds());
  return `${year}-${month}-${day} ${hours}:${mins}:${secs}`;
}

export function formatRelativeTime(millis: number): string {
  if (!millis) return "";
  const now = Date.now();
  const diffSec = Math.floor((now - millis) / 1000);
  if (diffSec < 0 || diffSec < 45) return "刚刚";
  if (diffSec < 3600) return `${Math.floor(diffSec / 60)} 分钟前`;
  if (diffSec < 86400) return `${Math.floor(diffSec / 3600)} 小时前`;
  if (diffSec < 86400 * 7) return `${Math.floor(diffSec / 86400)} 天前`;
  const d = new Date(millis);
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getMonth() + 1}月${d.getDate()}日 ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
