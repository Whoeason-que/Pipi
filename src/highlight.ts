import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import type { HighlighterCore, LanguageRegistration, ThemedToken } from "shiki";
import { createHighlighterCore } from "shiki/core";
import { createOnigurumaEngine } from "shiki/engine/oniguruma";
import { pipiCodeTheme, PIPI_CODE_THEME_NAME } from "./highlight-theme";

/**
 * 代码高亮：Shiki 核心 + 按语言动态 import 语法。
 *
 * - highlighter 本身首次遇到代码块时才初始化，不阻塞应用启动；
 * - 语言模块只在第一次遇到该围栏语言时加载，未支持的语言回退纯文本；
 * - 主题固定为深色 `pipi-code`，与代码块恒为深底的视觉一致。
 */

const MAX_HIGHLIGHT_CHARS = 50_000;

type LanguageModule = { default: LanguageRegistration | LanguageRegistration[] };

const LANGUAGE_LOADERS: Record<string, () => Promise<LanguageModule>> = {
  typescript: () => import("shiki/langs/typescript.mjs"),
  tsx: () => import("shiki/langs/tsx.mjs"),
  javascript: () => import("shiki/langs/javascript.mjs"),
  jsx: () => import("shiki/langs/jsx.mjs"),
  json: () => import("shiki/langs/json.mjs"),
  jsonc: () => import("shiki/langs/jsonc.mjs"),
  rust: () => import("shiki/langs/rust.mjs"),
  shellscript: () => import("shiki/langs/shellscript.mjs"),
  python: () => import("shiki/langs/python.mjs"),
  go: () => import("shiki/langs/go.mjs"),
  html: () => import("shiki/langs/html.mjs"),
  css: () => import("shiki/langs/css.mjs"),
  scss: () => import("shiki/langs/scss.mjs"),
  markdown: () => import("shiki/langs/markdown.mjs"),
  yaml: () => import("shiki/langs/yaml.mjs"),
  toml: () => import("shiki/langs/toml.mjs"),
  sql: () => import("shiki/langs/sql.mjs"),
  diff: () => import("shiki/langs/diff.mjs"),
  java: () => import("shiki/langs/java.mjs"),
  c: () => import("shiki/langs/c.mjs"),
  cpp: () => import("shiki/langs/cpp.mjs"),
  csharp: () => import("shiki/langs/csharp.mjs"),
  php: () => import("shiki/langs/php.mjs"),
  ruby: () => import("shiki/langs/ruby.mjs"),
  swift: () => import("shiki/langs/swift.mjs"),
  kotlin: () => import("shiki/langs/kotlin.mjs"),
  dockerfile: () => import("shiki/langs/dockerfile.mjs"),
  xml: () => import("shiki/langs/xml.mjs"),
  ini: () => import("shiki/langs/ini.mjs"),
  makefile: () => import("shiki/langs/makefile.mjs"),
  lua: () => import("shiki/langs/lua.mjs"),
  powershell: () => import("shiki/langs/powershell.mjs"),
  vue: () => import("shiki/langs/vue.mjs"),
  svelte: () => import("shiki/langs/svelte.mjs"),
  astro: () => import("shiki/langs/astro.mjs"),
  graphql: () => import("shiki/langs/graphql.mjs"),
  prisma: () => import("shiki/langs/prisma.mjs"),
  nix: () => import("shiki/langs/nix.mjs"),
};

const LANGUAGE_ALIASES: Record<string, string> = {
  ts: "typescript",
  cts: "typescript",
  mts: "typescript",
  js: "javascript",
  cjs: "javascript",
  mjs: "javascript",
  bash: "shellscript",
  sh: "shellscript",
  shell: "shellscript",
  zsh: "shellscript",
  py: "python",
  rs: "rust",
  md: "markdown",
  yml: "yaml",
  htm: "html",
  docker: "dockerfile",
  ps1: "powershell",
  cs: "csharp",
  kt: "kotlin",
  rb: "ruby",
  "c++": "cpp",
  "c#": "csharp",
  golang: "go",
};

let highlighter: HighlighterCore | null = null;
let highlighterPromise: Promise<void> | null = null;
const listeners = new Set<() => void>();
const languagePromises = new Map<string, Promise<boolean>>();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function notify(): void {
  for (const listener of listeners) listener();
}

/** 首次调用时初始化 Shiki 核心；重复调用共享同一个 Promise。 */
export function loadHighlighter(): Promise<void> {
  if (highlighter) return Promise.resolve();
  highlighterPromise ??= createHighlighterCore({
    themes: [pipiCodeTheme],
    langs: [],
    engine: createOnigurumaEngine(import("shiki/wasm")),
  }).then((instance) => {
    highlighter = instance;
    notify();
  }).catch(() => {
    // 高亮不可用时保持纯文本，不影响消息渲染。
  });
  return highlighterPromise;
}

/** 订阅 highlighter 的加载状态；未就绪时返回 null，先渲染纯文本。 */
export function useHighlighter(): HighlighterCore | null {
  useEffect(() => {
    void loadHighlighter();
  }, []);
  return useSyncExternalStore(subscribe, () => highlighter, () => null);
}

/** 把围栏语言（含常见别名）解析为已支持的规范语言名；不支持则返回 null。 */
export function resolveLanguage(language: string | null | undefined): string | null {
  if (!language) return null;
  const normalized = language.trim().toLowerCase().replace(/^language-/, "");
  if (!normalized || normalized === "text" || normalized === "plaintext" || normalized === "plain" || normalized === "txt") {
    return null;
  }
  const canonical = LANGUAGE_ALIASES[normalized] ?? normalized;
  return LANGUAGE_LOADERS[canonical] ? canonical : null;
}

async function ensureLanguage(instance: HighlighterCore, language: string): Promise<boolean> {
  if (instance.getLoadedLanguages().includes(language)) return true;
  const loader = LANGUAGE_LOADERS[language];
  if (!loader) return false;
  let promise = languagePromises.get(language);
  if (!promise) {
    promise = loader()
      .then((module) => instance.loadLanguage(module.default))
      .then(() => true)
      .catch(() => false);
    languagePromises.set(language, promise);
  }
  return promise;
}

/**
 * 把代码块 token 化为 Shiki token；语言按需加载。
 * 在语言就绪前返回 null，调用方先渲染原始代码文本。
 */
export function useShikiTokens(code: string, language: string | null): ThemedToken[][] | null {
  const instance = useHighlighter();
  const canonical = resolveLanguage(language);
  const [ready, setReady] = useState<string | null>(null);

  useEffect(() => {
    if (!instance || !canonical) {
      setReady(null);
      return;
    }
    let active = true;
    if (instance.getLoadedLanguages().includes(canonical)) {
      setReady(canonical);
      return;
    }
    void ensureLanguage(instance, canonical).then((ok) => {
      if (active && ok) setReady(canonical);
    });
    return () => {
      active = false;
    };
  }, [instance, canonical]);

  return useMemo(() => {
    if (!instance || !canonical || ready !== canonical) return null;
    if (!code || code.length > MAX_HIGHLIGHT_CHARS) return null;
    try {
      return instance.codeToTokens(code, { lang: canonical, theme: PIPI_CODE_THEME_NAME }).tokens;
    } catch {
      return null;
    }
  }, [instance, code, canonical, ready]);
}
