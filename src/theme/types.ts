export type ThemeMode = "dark" | "light";

/**
 * 完整的设计令牌契约（对齐 tokens.css 中的 CSS 变量）
 */
export interface ThemeTokens {
  // 1. 结构与容器底色
  bg: string;
  panel: string;
  panel2: string;
  navBg: string;
  railBg: string;
  navDivider: string;
  card: string;
  chatSurface: string;
  codeBg: string;
  inlineCode: string;

  // 2. 文字排版与边框线条
  fg: string;
  text: string;
  muted: string;
  faint: string;
  line: string;
  line2: string;
  lineSoft: string;
  stripe: string;
  hover: string;
  hover2: string;

  // 3. 品牌与强调色
  accent: string;
  accentText: string;
  onAccent: string;
  onDanger: string;
  accentSoft: string;
  accentBorder: string;
  primaryBg: string;
  primaryFg: string;

  // 4. 状态与通知色
  ok: string;
  okSoft: string;
  okBorder: string;
  warn: string;
  warnSoft: string;
  warnBorder: string;
  danger: string;
  dangerSoft: string;
  dangerBorder: string;

  // 5. 头像调色板
  avatarBlue?: string;
  avatarBlueFg?: string;
  avatarTeal?: string;
  avatarTealFg?: string;
  avatarAmber?: string;
  avatarAmberFg?: string;
  avatarRose?: string;
  avatarRoseFg?: string;

  // 6. 轨道、辉光与阴影
  track?: string;
  bar?: string;
  glowOk?: string;
  glowWarn?: string;
  shadow?: string;
  cardShadow?: string;
  composerShadow?: string;
  bgFade?: string;
}

/**
 * 设置面板中微缩工作区预览专用的配色提取
 */
export interface ThemePreview {
  bg: string;
  nav: string;
  rail: string;
  card: string;
  line: string;
  accent: string;
}

/**
 * 主题包元数据与定义
 */
export interface ThemeDefinition {
  /** 唯一标识，如 "ink-blue", "light", "nord", "catppuccin" */
  id: string;
  /** UI 显示名称，如 "墨蓝工作台", "Nord 极光" */
  label: string;
  /** 基础模式：决定原生滚动条、WebView input、color-scheme 的明暗属性 */
  mode: ThemeMode;
  /** 设置卡片预览专用的色块提取 */
  preview: ThemePreview;
  /** 具体的令牌键值。未声明的令牌自动继承对应 mode 的基底默认值 */
  tokens: Partial<ThemeTokens>;
  /** 简要描述（可选） */
  description?: string;
}
