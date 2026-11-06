import type { ThemeRegistration } from "shiki";

export const PIPI_CODE_THEME_NAME = "pipi-code";

/**
 * 代码块恒为深底，沿用原来的 ChatGPT 深色调色板。
 * scope 映射按 TextMate 语义归类，颜色值与原 highlight.js 规则一一对应。
 */
export const pipiCodeTheme: ThemeRegistration = {
  name: PIPI_CODE_THEME_NAME,
  type: "dark",
  colors: {
    "editor.background": "#0b1017",
    "editor.foreground": "#ececec",
  },
  settings: [
    { settings: { foreground: "#ececec" } },
    {
      scope: ["comment", "punctuation.definition.comment"],
      settings: { foreground: "#8b949e", fontStyle: "italic" },
    },
    {
      scope: [
        "keyword",
        "keyword.control",
        "keyword.operator.new",
        "storage",
        "storage.type",
        "storage.modifier",
        "constant.language",
        "constant.language.boolean",
        "constant.language.null",
        "variable.language",
      ],
      settings: { foreground: "#2e95d3" },
    },
    {
      scope: [
        "string",
        "string.quoted",
        "string.template",
        "constant.character",
        "constant.character.escape",
        "constant.other.symbol",
        "markup.inserted",
      ],
      settings: { foreground: "#00a67d" },
    },
    {
      scope: [
        "constant.numeric",
        "constant.numeric.integer",
        "constant.numeric.float",
        "variable",
        "variable.other",
        "variable.parameter",
        "support.variable",
        "punctuation.definition.variable",
      ],
      settings: { foreground: "#df3079" },
    },
    {
      scope: [
        "entity.name.function",
        "support.function",
        "entity.name.tag",
        "entity.name.section",
        "markup.heading",
      ],
      settings: { foreground: "#f22c3d" },
    },
    {
      scope: [
        "entity.other.attribute-name",
        "entity.name.type",
        "support.type",
        "support.class",
        "variable.other.property",
        "meta.object-literal.key",
        "support.type.property-name",
      ],
      settings: { foreground: "#e9950c" },
    },
    {
      scope: ["meta.preprocessor", "meta.annotation", "meta.decorator", "markup.deleted", "invalid", "invalid.illegal"],
      settings: { foreground: "#f2939f" },
    },
  ],
};
