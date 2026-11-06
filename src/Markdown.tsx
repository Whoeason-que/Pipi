import {
  memo,
  useMemo,
  useRef,
  type ClassAttributes,
  type HTMLAttributes,
} from "react";
import ReactMarkdown, { defaultUrlTransform, type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { useCopyFeedback } from "./components/use-copy";
import { TokenSpans, useHighlightedChildren } from "./components/HighlightedCode";
import { MarkdownImage, type ImageAgent } from "./files/MarkdownImage";
import { markdownImagePath } from "./files/model";
import { useI18n } from "./i18n";

/**
 * 流式安全：生成中的消息常以未闭合的 ``` 代码块结尾，
 * 补一个闭合围栏让 Markdown 解析器能按代码块渲染，流结束后自然还原。
 */
function balanceFences(text: string): string {
  const fences = (text.match(/```/g) ?? []).length;
  return fences % 2 === 1 ? `${text}\n\`\`\`` : text;
}

type PreProps = ClassAttributes<HTMLPreElement> & HTMLAttributes<HTMLPreElement>;

/**
 * 代码块外壳：头部行（语言名 + 复制按钮）+ 代码主体，
 * 形态对齐 ChatGPT；配色见 styles/markdown.css 的 .codeblock-*。
 * 高亮由 Shiki 按需加载，语言未就绪时先渲染原始代码文本。
 */
function CodeBlockPre({ children }: PreProps) {
  const preRef = useRef<HTMLPreElement>(null);
  const { copy, state } = useCopyFeedback();
  const { t } = useI18n();
  const { language, tokens } = useHighlightedChildren(children);
  const copyLabel = state === "copied" ? t("common.copied") : state === "failed" ? t("chat.copyFailed") : t("common.copy");

  const handleCopy = () => {
    const text = preRef.current?.innerText ?? "";
    void copy(text);
  };

  return (
    <div className="codeblock">
      <div className="codeblock-head">
        <span className="codeblock-lang">{language ?? "text"}</span>
        <button type="button" className="codeblock-copy" onClick={handleCopy}>
          {copyLabel}
        </button>
      </div>
      <pre ref={preRef}>
        {tokens ? <code><TokenSpans tokens={tokens} /></code> : children}
      </pre>
    </div>
  );
}

/**
 * Agent 输出的 Markdown 渲染。
 * - HTML 默认转义（不启用 rehype-raw）：模型输出不可信，防注入
 * - remark-gfm：表格 / 删除线 / 任务列表
 * - Shiki：代码高亮（见 src/highlight.ts 的按需加载与 pipi-code 主题）
 */
export const Markdown = memo(function Markdown({ text, imageAgent }: { text: string; imageAgent?: ImageAgent }) {
  const components = useMemo<Components>(() => ({
    pre: CodeBlockPre,
    img: ({ src, alt }) => <MarkdownImage src={typeof src === "string" ? src : ""} alt={alt} agent={imageAgent} />,
  }), [imageAgent?.name, imageAgent?.workspace]);
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={components}
        urlTransform={(url, key) => key === "src" && markdownImagePath(url) !== null ? url : defaultUrlTransform(url)}
      >
        {balanceFences(text)}
      </ReactMarkdown>
    </div>
  );
});
