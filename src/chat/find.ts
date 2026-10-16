import type { ChatBlock } from "../chat-runtime.ts";

/** 以消息为单位检索正文，不把思考、工具或系统通知混进结果。 */
export function findMessages(blocks: ChatBlock[], query: string): string[] {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return [];
  return blocks.flatMap(block => (block.kind === "user" || block.kind === "body")
    && block.entry.text.toLocaleLowerCase().includes(needle) ? [block.key] : []);
}
