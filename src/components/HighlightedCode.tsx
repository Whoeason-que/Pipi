import {
  Children,
  Fragment,
  isValidElement,
  type CSSProperties,
  type ReactNode,
} from "react";
import type { ThemedToken } from "shiki";
import { useShikiTokens } from "../highlight";

const FONT_ITALIC = 1;
const FONT_BOLD = 2;
const FONT_UNDERLINE = 4;

/** 从 ReactMarkdown 的 `<code>` 子节点里取围栏语言（`language-*` class）。 */
export function extractLanguage(children: ReactNode): string | null {
  for (const child of Children.toArray(children)) {
    if (!isValidElement(child)) continue;
    const className = (child.props as { className?: string }).className ?? "";
    const match = className.match(/language-([\w+#.-]+)/);
    if (match) return match[1];
  }
  return null;
}

/** 递归展开 code 子节点，得到原始代码文本。 */
export function extractCodeText(children: ReactNode): string {
  if (typeof children === "string" || typeof children === "number") return String(children);
  if (Array.isArray(children)) return children.map(extractCodeText).join("");
  if (isValidElement(children)) {
    return extractCodeText((children.props as { children?: ReactNode }).children);
  }
  return "";
}

export function useHighlightedChildren(children: ReactNode): {
  language: string | null;
  code: string;
  tokens: ThemedToken[][] | null;
} {
  const language = extractLanguage(children);
  const code = extractCodeText(children);
  const tokens = useShikiTokens(code, language);
  return { language, code, tokens };
}

/** Shiki token 的 React 渲染；不注入 HTML，模型输出仍按文本处理。 */
export function TokenSpans({ tokens }: { tokens: ThemedToken[][] }) {
  return (
    <>
      {tokens.map((line, lineIndex) => (
        <Fragment key={lineIndex}>
          {line.map((token, tokenIndex) => {
            const style: CSSProperties = {};
            if (token.color) style.color = token.color;
            if (token.bgColor) style.backgroundColor = token.bgColor;
            if (token.fontStyle) {
              if (token.fontStyle & FONT_ITALIC) style.fontStyle = "italic";
              if (token.fontStyle & FONT_BOLD) style.fontWeight = 700;
              if (token.fontStyle & FONT_UNDERLINE) style.textDecoration = "underline";
            }
            return (
              <span key={tokenIndex} style={style}>
                {token.content}
              </span>
            );
          })}
          {lineIndex < tokens.length - 1 ? "\n" : null}
        </Fragment>
      ))}
    </>
  );
}

/**
 * 文件预览用的代码块：只高亮，不加复制头（文件区有自己的工具栏）。
 * 语言未加载/不支持时先回退原始 `<code>`。
 */
export function HighlightedPre({ children }: { children?: ReactNode }) {
  const { tokens } = useHighlightedChildren(children);
  return <pre>{tokens ? <code><TokenSpans tokens={tokens} /></code> : children}</pre>;
}
