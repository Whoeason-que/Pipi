//! 模型目录：类型与纯变换（选项、反查、限额回填）。
//! 取数逻辑在 catalog-client.ts —— 那里才依赖平台层，本模块保持零运行时依赖，
//! 便于 `node --test` 直接单测。

export type ProviderGroup = "聚合网关" | "国内" | "国际" | "本地";

/** 分组顺序即选择器里的顺序（与 core 的 GROUPS 一致）。 */
export const PROVIDER_GROUPS: ProviderGroup[] = ["聚合网关", "国内", "国际", "本地"];

export interface CatalogModel {
  id: string;
  name: string;
  context?: number;
  output?: number;
  reasoning?: boolean;
}

export interface CatalogProvider {
  id: string;
  name: string;
  api: "anthropic-messages" | "openai-completions";
  baseUrl: string;
  envKey?: string | null;
  group: ProviderGroup;
  doc?: string | null;
  note?: string | null;
  /** 端点不含版本段时的人工核实出处（OpenAI 兼容协议的例外）。 */
  baseUrlNote?: string | null;
  local: boolean;
  models: CatalogModel[];
}

export interface ModelCatalog {
  fetchedAt?: number | null;
  /** `models.dev` = 本次联网刷新；`cache` = 读的本地缓存。 */
  source: string;
  /** 缓存过期且刷新失败（此时仍可用，只是可能旧）。 */
  stale: boolean;
  providers: CatalogProvider[];
}

export interface ModelChoice {
  value: string;
  label: string;
}

/** 本地主机别名：本地运行时的端点常被写成 localhost / 127.0.0.1 / [::1] 三种形态。 */
const LOCAL_HOST_ALIASES = new Set(["localhost", "127.0.0.1", "::1"]);

/**
 * 端点归一化键：忽略尾斜杠与大小写，并把本地主机别名折叠成同一种写法。
 * 不这样做的话，旧数据里的 `http://127.0.0.1:1234/v1` 匹配不上目录里的
 * `http://localhost:1234/v1`，用户会看到「该供应商不在模型目录里」。
 */
function endpointKey(api: string, baseUrl: string): string | null {
  const trimmed = baseUrl.trim().replace(/\/+$/, "");
  if (!trimmed) return null;
  try {
    const url = new URL(trimmed);
    const rawHost = url.hostname.replace(/^\[|\]$/g, "").toLowerCase();
    const host = LOCAL_HOST_ALIASES.has(rawHost) ? "local" : rawHost;
    const port = url.port || (url.protocol === "https:" ? "443" : "80");
    return `${api}|${url.protocol}//${host}:${port}${url.pathname}`.toLowerCase();
  } catch {
    // 不是合法 URL（用户可能填了半截）：退回字符串比较
    return `${api}|${trimmed}`.toLowerCase();
  }
}

/** 用 (协议, 端点) 在目录里反查 —— 口径与会话模型绑定一致，避免张冠李戴。 */
export function matchProvider(
  catalog: ModelCatalog | null,
  api: string,
  baseUrl: string,
): CatalogProvider | undefined {
  if (!catalog) return undefined;
  const target = endpointKey(api, baseUrl);
  if (!target) return undefined;
  return catalog.providers.find((provider) => endpointKey(provider.api, provider.baseUrl) === target);
}

/** 供应商的模型 → 下拉选项。保持目录顺序，label 带 id（同名/多代模型靠它区分）。 */
export function modelChoices(provider: CatalogProvider | undefined): ModelChoice[] {
  if (!provider) return [];
  return provider.models.map((model) => ({
    value: model.id,
    label: `${model.name} · ${model.id}`,
  }));
}

/** 目录里查一个模型（选中后回填 maxTokens / contextWindow 用）。 */
export function findCatalogModel(
  provider: CatalogProvider | undefined,
  modelId: string,
): CatalogModel | undefined {
  const id = modelId.trim();
  if (!provider || !id) return undefined;
  return provider.models.find((model) => model.id === id);
}

/**
 * 目录不可用时的最小清单（离线首启）：只给常用模型 id/名称。
 * 端点、上下文等一切以用户填的供应商为准 —— 这里不做任何猜测。
 */
export const FALLBACK_MODELS: Array<{ id: string; name: string }> = [
  { id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6" },
  { id: "claude-opus-4-5", name: "Claude Opus 4.5" },
  { id: "gpt-5", name: "GPT-5" },
  { id: "gpt-5-mini", name: "GPT-5 mini" },
  { id: "deepseek-chat", name: "DeepSeek Chat" },
  { id: "deepseek-reasoner", name: "DeepSeek Reasoner" },
];

/** 目录不可用时的模型选项。 */
export function fallbackModelChoices(): ModelChoice[] {
  return FALLBACK_MODELS.map((model) => ({ value: model.id, label: `${model.name} · ${model.id}` }));
}

/** 选中目录模型时回填单次最大输出；目录没数据就保留调用点的旧值。 */
export function resolveMaxTokens(model: CatalogModel | undefined, fallback: number): number {
  return model?.output && model.output > 0 ? model.output : fallback;
}

/** 选中目录模型时回填上下文窗口；目录没数据就保留调用点的旧值。 */
export function resolveContextWindow(model: CatalogModel | undefined, fallback: number): number {
  return model?.context && model.context > 0 ? model.context : fallback;
}

/** 从目录条目生成「添加提供商」表单的种子值（不含密钥）。 */
export function providerSeed(provider: CatalogProvider): {
  id: string;
  name: string;
  api: "anthropic-messages" | "openai-completions";
  baseUrl: string;
  envKey: string;
} {
  return {
    id: provider.id,
    name: provider.name,
    api: provider.api,
    baseUrl: provider.baseUrl,
    envKey: provider.envKey ?? "",
  };
}

/** 目录 → 分组选项（预设选择器用）。 */
export function providerGroups(
  catalog: ModelCatalog,
): Array<{ label: ProviderGroup; options: Array<{ value: string; label: string }> }> {
  return PROVIDER_GROUPS.map((group) => ({
    label: group,
    options: catalog.providers
      .filter((provider) => provider.group === group)
      .map((provider) => ({ value: provider.id, label: provider.name })),
  })).filter((entry) => entry.options.length > 0);
}

/** 目录来源的一句话描述（状态栏/提示用）。 */
export function catalogSourceLabel(catalog: ModelCatalog): string {
  if (catalog.source === "cache") {
    return catalog.stale ? "本地缓存（刷新失败，可能过期）" : "本地缓存";
  }
  return "models.dev";
}
