import {
  Children,
  isValidElement,
  memo,
  useRef,
  useState,
  type ClassAttributes,
  type HTMLAttributes,
  type ReactNode,
} from "react";
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

/** 从 <code> 的 class 里取语言名（rehype-highlight 会写 language-*）。 */
function extractLang(children: ReactNode): string | null {
  for (const child of Children.toArray(children)) {
    if (isValidElement(child)) {
      const className = (child.props as { className?: string }).className ?? "";
      const match = className.match(/language-([\w+#.-]+)/);
      if (match) return match[1];
    }
  }
  return null;
}

type PreProps = ClassAttributes<HTMLPreElement> & HTMLAttributes<HTMLPreElement>;

/**
 * 代码块外壳：头部行（语言名 + 复制按钮）+ 代码主体，
 * 形态对齐 ChatGPT；配色见 styles/markdown.css 的 .codeblock-*。
 */
function CodeBlockPre({ children }: PreProps) {
  const preRef = useRef<HTMLPreElement>(null);
  const [copied, setCopied] = useState(false);
  const lang = extractLang(children);

  const handleCopy = () => {
    const text = preRef.current?.innerText ?? "";
    navigator.clipboard
      ?.writeText(text)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      })
      .catch(() => {
        // 剪贴板不可用时静默失败：复制是锦上添花，不该打断阅读
      });
  };

  return (
    <div className="codeblock">
      <div className="codeblock-head">
        <span className="codeblock-lang">{lang ?? "text"}</span>
        <button type="button" className="codeblock-copy" onClick={handleCopy}>
          {copied ? "已复制" : "复制"}
        </button>
      </div>
      <pre ref={preRef}>{children}</pre>
    </div>
  );
}

/**
 * Agent 输出的 Markdown 渲染。
 * - HTML 默认转义（不启用 rehype-raw）：模型输出不可信，防注入
 * - remark-gfm：表格 / 删除线 / 任务列表
 * - rehype-highlight：代码高亮（配色见 styles/markdown.css 的 .hljs-*）
 */
export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeHighlight]}
        components={{ pre: CodeBlockPre }}
      >
        {balanceFences(text)}
      </ReactMarkdown>
    </div>
  );
});
