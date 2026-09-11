#!/usr/bin/env node
/**
 * 生成 src/providers.ts —— 模型提供商预设目录。
 *
 * 数据源：models.dev（MIT，https://models.dev/api.json），也就是 opencode 使用的
 * 那份模型目录。opencode 会把快照缓存在 ~/.cache/opencode/models.json，本脚本优先
 * 读取本地缓存，缺失时再联网拉取。
 *
 * 为什么是「生成」而不是手写：baseUrl / envKey / 模型清单都是易变数据，
 * 手写必然漂移。预设的取舍（收录哪些、归到哪一组、协议映射、要不要覆盖 baseUrl）
 * 全部写在下面的 CURATION 表里 —— 改预设改这张表，然后 `npm run providers:gen`。
 *
 * 协议映射（Pipi 只支持两种，见 crates/pipi-core/src/types.rs 的 Api）：
 *   @ai-sdk/anthropic        → anthropic-messages   （rig 会剥掉尾部 /v1，见 anthropic/client.rs）
 *   @ai-sdk/openai(-compat)  → openai-completions   （rig 拼 {baseUrl}/chat/completions，baseUrl 必须含版本段）
 * 其它 SDK（google/azure/bedrock 等原生协议）不支持，只能走厂商提供的 OpenAI 兼容端点。
 */
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { homedir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const LOCAL_CACHE = join(homedir(), ".cache/opencode/models.json");
const REMOTE = "https://models.dev/api.json";
const MAX_MODELS_PER_PROVIDER = 12;

/** 收录哪些 provider、归到哪组、是否覆盖字段。 */
const CURATION = [
  // ---- 聚合网关 ----
  { pid: "openrouter", group: "聚合网关", note: "聚合 300+ 模型；模型 ID 形如 anthropic/claude-sonnet-4.5" },
  { pid: "requesty", group: "聚合网关" },
  // baseUrl 目录未提供 → 已核对官方文档（vercel.com/docs/ai-gateway 的 /v1/chat/completions）
  { pid: "vercel", group: "聚合网关", baseUrl: "https://ai-gateway.vercel.sh/v1", note: "AI Gateway（OpenAI 兼容端点）" },
  { pid: "llmgateway", group: "聚合网关" },
  { pid: "poe", group: "聚合网关" },
  { pid: "opencode", presetId: "opencode-zen", name: "OpenCode Zen", group: "聚合网关" },
  { pid: "opencode-go", name: "OpenCode Go", group: "聚合网关" },
  { pid: "302ai", name: "302.AI", group: "聚合网关", note: "目录里的环境变量名以数字开头，shell 不便设置，建议直接在表单里填明文密钥" },

  // ---- 国内 ----
  { pid: "deepseek", group: "国内", baseUrl: "https://api.deepseek.com/v1" },
  {
    pid: "deepseek",
    presetId: "deepseek-anthropic",
    name: "DeepSeek（Anthropic 兼容）",
    group: "国内",
    api: "anthropic-messages",
    baseUrl: "https://api.deepseek.com/anthropic",
    note: "官方 Anthropic 兼容端点，可跑 Claude Code 类客户端",
  },
  { pid: "moonshotai-cn", name: "Moonshot / Kimi（中国）", group: "国内" },
  { pid: "moonshotai", name: "Moonshot / Kimi（国际）", group: "国内" },
  { pid: "kimi-for-coding", name: "Kimi For Coding", group: "国内" },
  { pid: "zhipuai", name: "智谱 GLM（开放平台）", group: "国内" },
  { pid: "zai", name: "Z.ai（GLM 国际）", group: "国内" },
  { pid: "minimax-cn", name: "MiniMax（中国）", group: "国内" },
  { pid: "alibaba-cn", name: "阿里云百炼 / Qwen（中国）", group: "国内" },
  { pid: "alibaba", name: "Aliyun DashScope（国际）", group: "国内" },
  { pid: "siliconflow-cn", name: "硅基流动 SiliconFlow", group: "国内" },
  { pid: "qiniu-ai", name: "七牛云 AI", group: "国内" },
  { pid: "volcengine", name: "火山方舟 Volcengine", group: "国内", note: "模型 ID 用方舟的 endpoint id（ep-…）或模型名" },
  { pid: "modelscope", name: "魔搭 ModelScope", group: "国内" },

  // ---- 国际 ----
  // 以下 baseUrl 目录未提供，均按各家官方 SDK 的默认值核实（见提交说明）：
  { pid: "anthropic", group: "国际", baseUrl: "https://api.anthropic.com" },
  { pid: "openai", group: "国际", baseUrl: "https://api.openai.com/v1" },
  { pid: "google", name: "Google Gemini", group: "国际", api: "openai-completions", baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai/", note: "走 Gemini 的 OpenAI 兼容端点（原生协议不支持）", envKey: "GEMINI_API_KEY" },
  { pid: "xai", name: "xAI Grok", group: "国际", baseUrl: "https://api.x.ai/v1" },
  { pid: "groq", group: "国际", baseUrl: "https://api.groq.com/openai/v1" },
  { pid: "mistral", group: "国际", baseUrl: "https://api.mistral.ai/v1" },
  { pid: "togetherai", group: "国际", baseUrl: "https://api.together.xyz/v1" },
  { pid: "deepinfra", group: "国际", baseUrl: "https://api.deepinfra.com/v1/openai" },
  { pid: "cerebras", group: "国际", baseUrl: "https://api.cerebras.ai/v1" },
  { pid: "fireworks-ai", name: "Fireworks AI", group: "国际" },
  { pid: "novita-ai", group: "国际" },
  { pid: "nvidia", name: "NVIDIA NIM", group: "国际" },
  { pid: "huggingface", name: "Hugging Face Router", group: "国际", envKey: "HF_TOKEN" },
  { pid: "upstage", group: "国际" },
  { pid: "inception", group: "国际" },
  { pid: "chutes", group: "国际" },

  // ---- 本地（不在 models.dev 目录里，手写） ----
  {
    manual: true,
    presetId: "ollama",
    name: "Ollama（本地）",
    api: "openai-completions",
    baseUrl: "http://localhost:11434/v1",
    envKey: "OLLAMA_API_KEY",
    group: "本地",
    note: "本地服务：密钥填任意非空值（如 ollama）；模型名以 `ollama list` 为准",
    models: [],
    doc: "https://docs.ollama.com/api/openai-compatibility",
  },
  { pid: "lmstudio", name: "LM Studio（本地）", group: "本地", note: "本地服务：密钥填任意非空值；模型名以 LM Studio 已加载模型为准" },
  {
    manual: true,
    presetId: "llamacpp",
    name: "llama.cpp server（本地）",
    api: "openai-completions",
    baseUrl: "http://localhost:8080/v1",
    envKey: "LLAMACPP_API_KEY",
    group: "本地",
    note: "本地服务：密钥填任意非空值；模型名以 --model / --alias 指定值为准",
    models: [],
    doc: "https://github.com/ggml-org/llama.cpp/tree/master/tools/server",
  },
  {
    manual: true,
    presetId: "vllm",
    name: "vLLM（本地）",
    api: "openai-completions",
    baseUrl: "http://localhost:8000/v1",
    envKey: "VLLM_API_KEY",
    group: "本地",
    note: "本地服务：密钥填任意非空值；模型名以 --served-model-name 为准",
    models: [],
    doc: "https://docs.vllm.ai/en/latest/serving/openai_compatible_server.html",
  },
];

const GROUP_ORDER = ["聚合网关", "国内", "国际", "本地"];

/**
 * SDK → 协议映射。只有明确说 OpenAI Chat Completions 形状的才映射为 openai-completions；
 * 目录里出现的其它 SDK 一律报错（宁可漏收录，也不猜协议）。
 * AnthropicMessages 走 rig 的 anthropic 客户端（会剥掉尾部 /v1 或 /messages）。
 */
const SDK_PROTOCOL = {
  "@ai-sdk/anthropic": "anthropic-messages",
  "@ai-sdk/openai": "openai-completions",
  "@ai-sdk/openai-compatible": "openai-completions",
  "@openrouter/ai-sdk-provider": "openai-completions",
  "@ai-sdk/xai": "openai-completions",
  "@ai-sdk/groq": "openai-completions",
  "@ai-sdk/mistral": "openai-completions",
  "@ai-sdk/togetherai": "openai-completions",
  "@ai-sdk/deepinfra": "openai-completions",
  "@ai-sdk/cerebras": "openai-completions",
  "@ai-sdk/gateway": "openai-completions",
};

function protocolOf(provider) {
  return SDK_PROTOCOL[provider?.npm ?? ""] ?? null;
}

function loadSource() {
  if (existsSync(LOCAL_CACHE)) {
    return { data: JSON.parse(readFileSync(LOCAL_CACHE, "utf-8")), origin: LOCAL_CACHE };
  }
  return { remote: true };
}

async function fetchRemote() {
  const response = await fetch(REMOTE);
  if (!response.ok) throw new Error(`拉取 ${REMOTE} 失败：HTTP ${response.status}`);
  return { data: await response.json(), origin: REMOTE };
}

function modelsOf(provider) {
  const all = Object.values(provider?.models ?? {}).filter((model) => model.tool_call);
  const list = all.slice(0, MAX_MODELS_PER_PROVIDER);
  return {
    modelCount: all.length,
    models: list.map((model) => ({
      id: model.id,
      name: model.name || model.id,
      context: model.limit?.context,
      output: model.limit?.output,
      reasoning: Boolean(model.reasoning) || undefined,
    })),
  };
}

function buildPreset(entry, catalog) {
  const stripSlash = (url) => url.replace(/\/+$/, "");
  if (entry.manual === true) {
    return {
      id: entry.presetId,
      name: entry.name,
      api: entry.api,
      baseUrl: stripSlash(entry.baseUrl),
      envKey: entry.envKey,
      group: entry.group,
      doc: entry.doc,
      note: entry.note,
      modelCount: 0,
      models: entry.models ?? [],
    };
  }
  const provider = catalog[entry.pid];
  if (!provider) throw new Error(`models.dev 目录里找不到 provider：${entry.pid}`);
  const api = entry.api ?? protocolOf(provider);
  if (!api) throw new Error(`${entry.pid} 的协议 ${provider.npm} 不受支持（Pipi 只支持两种），请改手动条目或换 provider`);
  const baseUrl = entry.baseUrl ?? provider.api;
  if (!baseUrl) throw new Error(`${entry.pid} 既没有 baseUrl 覆盖也没有目录 api 字段，请显式指定`);
  const modelInfo = entry.models ? { models: entry.models, modelCount: entry.models.length } : modelsOf(provider);
  return {
    id: entry.presetId ?? entry.pid,
    name: entry.name ?? provider.name ?? entry.pid,
    api,
    baseUrl: stripSlash(baseUrl),
    envKey: entry.envKey ?? provider.env?.[0] ?? "",
    group: entry.group,
    doc: entry.doc ?? provider.doc,
    note: entry.note,
    modelCount: modelInfo.modelCount,
    models: modelInfo.models,
  };
}

function ts(value) {
  return JSON.stringify(value);
}

function render(presets, origin) {
  const stamp = new Date().toISOString().slice(0, 10);
  const body = presets
    .map((preset) => {
      const lines = [
        "  {",
        `    id: ${ts(preset.id)},`,
        `    name: ${ts(preset.name)},`,
        `    api: ${ts(preset.api)},`,
        `    baseUrl: ${ts(preset.baseUrl)},`,
        `    envKey: ${ts(preset.envKey)},`,
        `    group: ${ts(preset.group)},`,
      ];
      if (preset.doc) lines.push(`    doc: ${ts(preset.doc)},`);
      if (preset.note) lines.push(`    note: ${ts(preset.note)},`);
      if (preset.modelCount > 0) lines.push(`    modelCount: ${preset.modelCount},`);
      if (preset.models.length === 0) {
        lines.push("    models: [],");
      } else {
        lines.push("    models: [");
        for (const model of preset.models) {
          const parts = [`id: ${ts(model.id)}`, `name: ${ts(model.name)}`];
          if (model.context) parts.push(`context: ${model.context}`);
          if (model.output) parts.push(`output: ${model.output}`);
          if (model.reasoning) parts.push("reasoning: true");
          lines.push(`      { ${parts.join(", ")} },`);
        }
        lines.push("    ],");
      }
      lines.push("  },");
      return lines.join("\n");
    })
    .join("\n");

  return `// 由 scripts/gen-provider-catalog.mjs 生成 —— 请勿手改。
// 改收录范围/分组/协议映射：改生成脚本里的 CURATION 表，然后跑 \`npm run providers:gen\`。
//
// 数据来源：models.dev（MIT，https://models.dev）—— opencode 使用的模型目录快照，生成于 ${stamp}
// （本地来源：${origin}）。baseUrl 已按 rig 的拼装方式核定：
//   OpenAI 兼容 → 必须含版本段（rig 拼 {baseUrl}/chat/completions）
//   Anthropic 兼容 → rig 会剥掉尾部 /v1 或 /messages，故原样使用厂商端点即可
import type { ApiKind } from "./types";

export type ProviderGroup = ${GROUP_ORDER.map(ts).join(" | ")};

export interface CatalogModel {
  id: string;
  name: string;
  /** 上下文窗口（tokens），来自目录 limit.context */
  context?: number;
  /** 单次最大输出（tokens），来自目录 limit.output */
  output?: number;
  reasoning?: boolean;
}

export interface ProviderPreset {
  /** 稳定 slug：既作为 ProviderConfig.id，也作为预设查找键 */
  id: string;
  name: string;
  api: ApiKind;
  baseUrl: string;
  envKey: string;
  group: ProviderGroup;
  doc?: string;
  note?: string;
  /** 目录里支持工具调用的模型总数（models 可能只收录前若干个） */
  modelCount?: number;
  models: CatalogModel[];
}

export const PROVIDER_GROUPS: ProviderGroup[] = ${JSON.stringify(GROUP_ORDER)};

export const PROVIDER_PRESETS: ProviderPreset[] = [
${body}
];

const BY_ID = new Map(PROVIDER_PRESETS.map((preset) => [preset.id, preset]));

export function findProviderPreset(id: string): ProviderPreset | undefined {
  return BY_ID.get(id);
}

function normalizeEndpoint(url: string): string {
  return url.trim().replace(/\\/+$/, "").toLowerCase();
}

/** 用 (协议, baseUrl) 反查预设 —— 会话模型选择器据此列出该供应商可用模型。 */
export function findProviderPresetByEndpoint(api: ApiKind, baseUrl: string): ProviderPreset | undefined {
  const target = normalizeEndpoint(baseUrl);
  if (!target) return undefined;
  return PROVIDER_PRESETS.find(
    (preset) => preset.api === api && normalizeEndpoint(preset.baseUrl) === target,
  );
}

/** 供「添加提供商」表单使用的初始值（不含密钥）。 */
export function presetToProviderFields(preset: ProviderPreset): {
  id: string;
  name: string;
  api: ApiKind;
  baseUrl: string;
  envKey: string;
} {
  return {
    id: preset.id,
    name: preset.name,
    api: preset.api,
    baseUrl: preset.baseUrl,
    envKey: preset.envKey,
  };
}
`;
}

async function main() {
  const { data, origin, remote } = loadSource();
  const catalog = remote ? await fetchRemote() : { data, origin };
  const presets = CURATION.map((entry) => buildPreset(entry, catalog.data));
  const ids = new Set();
  for (const preset of presets) {
    if (ids.has(preset.id)) throw new Error(`预设 id 重复：${preset.id}`);
    ids.add(preset.id);
  }
  const target = join(ROOT, "src/providers.ts");
  writeFileSync(target, render(presets, catalog.origin), "utf-8");
  const modelCount = presets.reduce((total, preset) => total + preset.models.length, 0);
  console.log(
    `已生成 ${target}：${presets.length} 个预设（${GROUP_ORDER.join("/")}），共 ${modelCount} 个模型；来源 ${catalog.origin}`,
  );
}

main().catch((error) => {
  console.error(String(error.message ?? error));
  process.exitCode = 1;
});
