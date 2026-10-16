import assert from "node:assert/strict";
import test from "node:test";
import {
  BUILTIN_THEMES,
  resolveTheme,
  compileThemeTokens,
  toCssVarName,
  applyTheme,
  getInitialThemeId,
  STORAGE_THEME_KEY,
} from "../src/theme/index.ts";

test("builtin themes are registered with complete metadata and previews", () => {
  assert.ok(BUILTIN_THEMES.length >= 4);

  const ids = new Set<string>();
  for (const theme of BUILTIN_THEMES) {
    assert.ok(theme.id.length > 0);
    assert.ok(!ids.has(theme.id), `Duplicate theme id: ${theme.id}`);
    ids.add(theme.id);

    assert.ok(theme.label.length > 0);
    assert.ok(theme.mode === "dark" || theme.mode === "light");

    // 确保预览色盘完整
    assert.ok(theme.preview.bg.startsWith("#") || theme.preview.bg.startsWith("rgb"));
    assert.ok(theme.preview.nav.startsWith("#") || theme.preview.nav.startsWith("rgb"));
    assert.ok(theme.preview.rail.startsWith("#") || theme.preview.rail.startsWith("rgb"));
    assert.ok(theme.preview.card.startsWith("#") || theme.preview.card.startsWith("rgb"));
    assert.ok(theme.preview.accent.startsWith("#") || theme.preview.accent.startsWith("rgb"));
  }
});

test("resolveTheme correctly resolves IDs and maintains backward compatibility", () => {
  // 1. 显式 ID 命中
  assert.equal(resolveTheme("nord").id, "nord");
  assert.equal(resolveTheme("catppuccin").id, "catppuccin");
  assert.equal(resolveTheme("charcoal").id, "charcoal");
  assert.equal(resolveTheme("solarized-light").id, "solarized-light");

  // 2. 兼容历史遗留枚举值 "dark" 与 "light"
  assert.equal(resolveTheme("dark").id, "ink-blue");
  assert.equal(resolveTheme("light").id, "light");

  // 3. 空值与未知值优雅降级回退到默认墨蓝工作台
  assert.equal(resolveTheme(null).id, "ink-blue");
  assert.equal(resolveTheme(undefined).id, "ink-blue");
  assert.equal(resolveTheme("non-existent-theme-xyz").id, "ink-blue");
});

test("compileThemeTokens merges base tokens and overrides without omissions", () => {
  const nord = resolveTheme("nord");
  const compiledNord = compileThemeTokens(nord);

  assert.equal(compiledNord.mode, "dark");
  // 覆盖字段
  assert.equal(compiledNord.tokens.bg, "#242933");
  assert.equal(compiledNord.tokens.accent, "#88c0d0");
  // 未显式覆盖的字段从 BASE_DARK_TOKENS 继承
  assert.ok(compiledNord.tokens.shadow !== undefined);
  assert.ok(compiledNord.tokens.cardShadow !== undefined);

  const solarized = resolveTheme("solarized-light");
  const compiledSolarized = compileThemeTokens(solarized);
  assert.equal(compiledSolarized.mode, "light");
  assert.equal(compiledSolarized.tokens.bg, "#faf6ed");
  assert.equal(compiledSolarized.tokens.accent, "#b45309");
});

test("toCssVarName converts camelCase and digit suffixes to CSS custom properties", () => {
  assert.equal(toCssVarName("bg"), "--bg");
  assert.equal(toCssVarName("panel2"), "--panel-2");
  assert.equal(toCssVarName("line2"), "--line-2");
  assert.equal(toCssVarName("hover2"), "--hover-2");
  assert.equal(toCssVarName("navBg"), "--nav-bg");
  assert.equal(toCssVarName("chatSurface"), "--chat-surface");
  assert.equal(toCssVarName("inlineCode"), "--inline-code");
  assert.equal(toCssVarName("avatarBlueFg"), "--avatar-blue-fg");
  assert.equal(toCssVarName("composerShadow"), "--composer-shadow");
});

test("applyTheme injects style tag and updates root attributes", () => {
  // 简易 DOM mock 环境
  const dataset: Record<string, string> = {};
  const styleObj: Record<string, string> = {};
  const headChildren: Array<{ id: string; textContent: string }> = [];

  const fakeElement = {
    dataset,
    style: styleObj,
  };

  const fakeDoc = {
    documentElement: fakeElement,
    head: {
      appendChild: (el: { id: string; textContent: string }) => {
        headChildren.push(el);
      },
    },
    getElementById: (id: string) => headChildren.find((el) => el.id === id) || null,
    createElement: (tag: string) => ({ id: "", textContent: "", tagName: tag }),
  };

  const storage: Record<string, string> = {};
  const fakeLocalStorage = {
    getItem: (k: string) => storage[k] ?? null,
    setItem: (k: string, v: string) => {
      storage[k] = v;
    },
  };

  // 临时注入全局 document 与 localStorage
  const originalDoc = (globalThis as unknown as { document: unknown }).document;
  const originalStorage = (globalThis as unknown as { localStorage: unknown }).localStorage;

  (globalThis as unknown as { document: unknown }).document = fakeDoc;
  (globalThis as unknown as { localStorage: unknown }).localStorage = fakeLocalStorage;

  try {
    const applied = applyTheme("nord");
    assert.equal(applied.id, "nord");
    assert.equal(dataset.theme, "dark");
    assert.equal(dataset.palette, "nord");
    assert.equal(styleObj.colorScheme, "dark");
    assert.equal(fakeLocalStorage.getItem(STORAGE_THEME_KEY), "nord");

    const styleTag = fakeDoc.getElementById("pipi-theme-tokens");
    assert.ok(styleTag);
    assert.ok(styleTag.textContent.includes("--bg: #242933;"));
    assert.ok(styleTag.textContent.includes("--accent: #88c0d0;"));

    // 测试读取初始主题 ID
    assert.equal(getInitialThemeId(), "nord");
  } finally {
    (globalThis as unknown as { document: unknown }).document = originalDoc;
    (globalThis as unknown as { localStorage: unknown }).localStorage = originalStorage;
  }
});

test("switching themes dynamically updates tokens and handles repeated transitions", () => {
  const dataset: Record<string, string> = {};
  const styleObj: Record<string, string> = {};
  const headChildren: Array<{ id: string; textContent: string }> = [];

  const fakeElement = { dataset, style: styleObj };
  const fakeDoc = {
    documentElement: fakeElement,
    head: {
      appendChild: (el: { id: string; textContent: string }) => {
        headChildren.push(el);
      },
    },
    getElementById: (id: string) => headChildren.find((el) => el.id === id) || null,
    createElement: (tag: string) => ({ id: "", textContent: "", tagName: tag }),
  };

  const storage: Record<string, string> = {};
  const fakeLocalStorage = {
    getItem: (k: string) => storage[k] ?? null,
    setItem: (k: string, v: string) => {
      storage[k] = v;
    },
  };

  const originalDoc = (globalThis as unknown as { document: unknown }).document;
  const originalStorage = (globalThis as unknown as { localStorage: unknown }).localStorage;

  (globalThis as unknown as { document: unknown }).document = fakeDoc;
  (globalThis as unknown as { localStorage: unknown }).localStorage = fakeLocalStorage;

  try {
    // 1. 初始应用默认墨蓝
    applyTheme("ink-blue");
    let styleTag = fakeDoc.getElementById("pipi-theme-tokens");
    assert.ok(styleTag);
    assert.equal(dataset.theme, "dark");
    assert.equal(dataset.palette, "ink-blue");
    assert.ok(styleTag.textContent.includes("--bg: #141a24;"));

    // 2. 切换到暖阳素白
    applyTheme("solarized-light");
    styleTag = fakeDoc.getElementById("pipi-theme-tokens");
    assert.ok(styleTag);
    assert.equal(dataset.theme, "light");
    assert.equal(dataset.palette, "solarized-light");
    assert.equal(styleObj.colorScheme, "light");
    assert.ok(styleTag.textContent.includes("--bg: #faf6ed;"));
    assert.ok(styleTag.textContent.includes("--accent: #b45309;"));

    // 3. 切换到 Catppuccin 莫卡
    applyTheme("catppuccin");
    styleTag = fakeDoc.getElementById("pipi-theme-tokens");
    assert.ok(styleTag);
    assert.equal(dataset.theme, "dark");
    assert.equal(dataset.palette, "catppuccin");
    assert.equal(styleObj.colorScheme, "dark");
    assert.ok(styleTag.textContent.includes("--bg: #1e1e2e;"));
    assert.ok(styleTag.textContent.includes("--accent: #cba6f7;"));

    // 4. 连续切换回默认浅色
    applyTheme("light");
    assert.equal(dataset.theme, "light");
    assert.equal(dataset.palette, "light");

    // 5. 传入历史遗留 "dark"，平滑映射至 ink-blue
    applyTheme("dark");
    assert.equal(dataset.theme, "dark");
    assert.equal(dataset.palette, "ink-blue");
  } finally {
    (globalThis as unknown as { document: unknown }).document = originalDoc;
    (globalThis as unknown as { localStorage: unknown }).localStorage = originalStorage;
  }
});
