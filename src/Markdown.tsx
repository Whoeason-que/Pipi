import { memo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeHighlight from "rehype-highlight";

/**
 * 流式安全：生成中的消息常以未闭合的 ``` 代码块结尾，
 * 补一个闭合围栏让 rehype-highlight 能按代码块渲染，流结束后自然还原。
 */
function balanceFences(text: string): string {
  const fences = (text.match(/```/g) ?? []).length;
  return fences % 2 === 1 ? `${text}\n\`\`\`` : text;
}

/**
 * Agent 输出的 Markdown 渲染。
 * - HTML 默认转义（不启用 rehype-raw）：模型输出不可信，防注入
 * - remark-gfm：表格 / 删除线 / 任务列表
 * - rehype-highlight：代码高亮（配色见 styles.css 的 .hljs-*）
 */
export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeHighlight]}>
        {balanceFences(text)}
      </ReactMarkdown>
    </div>
  );
});
