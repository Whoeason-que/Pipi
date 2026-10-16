import type { ChatBlock } from "../chat-runtime";

export interface SearchHit {
  key: string;
  role: "user" | "assistant";
  text: string;
  timestamp?: number;
}

export interface SnippetResult {
  prefix: string;
  match: string;
  suffix: string;
}

/** 以消息为单位检索正文并返回富信息列表，不把思考、工具或系统通知混进结果。 */
export function findSearchHits(blocks: ChatBlock[], query: string): SearchHit[] {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return [];
  return blocks.flatMap(block => {
    if ((block.kind === "user" || block.kind === "body") && block.entry.text.toLocaleLowerCase().includes(needle)) {
      return [{
        key: block.key,
        role: block.kind === "user" ? "user" : "assistant",
        text: block.entry.text,
        timestamp: block.entry.timestamp,
      }];
    }
    return [];
  });
}

/** 以消息为单位检索正文，返回匹配消息 key 列表。 */
export function findMessages(blocks: ChatBlock[], query: string): string[] {
  return findSearchHits(blocks, query).map(h => h.key);
}

/** 提取高亮摘要：截取关键字周围文字并保留原匹配字符。 */
export function extractSnippet(text: string, query: string, maxLen = 140): SnippetResult {
  const needle = query.trim();
  const clean = text.replace(/\s+/g, " ");
  if (!needle) {
    const slice = clean.slice(0, maxLen);
    return { prefix: slice + (clean.length > maxLen ? "…" : ""), match: "", suffix: "" };
  }
  const lowerText = clean.toLowerCase();
  const lowerNeedle = needle.toLowerCase();
  const idx = lowerText.indexOf(lowerNeedle);
  if (idx === -1) {
    const slice = clean.slice(0, maxLen);
    return { prefix: slice + (clean.length > maxLen ? "…" : ""), match: "", suffix: "" };
  }
  const start = Math.max(0, idx - 28);
  const end = Math.min(clean.length, idx + needle.length + 65);
  const prefix = (start > 0 ? "…" : "") + clean.slice(start, idx);
  const match = clean.slice(idx, idx + needle.length);
  const suffix = clean.slice(idx + needle.length, end) + (end < clean.length ? "…" : "");
  return { prefix, match, suffix };
}
