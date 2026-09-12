import assert from "node:assert/strict";
import test from "node:test";
import {
  FALLBACK_MODELS,
  PROVIDER_GROUPS,
  catalogSourceLabel,
  fallbackModelChoices,
  findCatalogModel,
  matchProvider,
  modelChoices,
  providerGroups,
  providerSeed,
  resolveContextWindow,
  resolveMaxTokens,
  type ModelCatalog,
} from "../src/catalog.ts";

function catalogFixture(): ModelCatalog {
  return {
    fetchedAt: 1_700_000_000,
    source: "models.dev",
    stale: false,
    providers: [
      {
        id: "deepseek",
        name: "DeepSeek",
        api: "openai-completions",
        baseUrl: "https://api.deepseek.com/v1",
        envKey: "DEEPSEEK_API_KEY",
        group: "国内",
        local: false,
        models: [
          { id: "deepseek-chat", name: "DeepSeek Chat", context: 128000, output: 8192 },
          { id: "deepseek-reasoner", name: "DeepSeek Reasoner", context: 128000, output: 65536, reasoning: true },
        ],
      },
      {
        id: "anthropic",
        name: "Anthropic",
        api: "anthropic-messages",
        baseUrl: "https://api.anthropic.com",
        envKey: "ANTHROPIC_API_KEY",
        group: "国际",
        local: false,
        models: [{ id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6", context: 1000000, output: 128000 }],
      },
      {
        id: "ollama",
        name: "Ollama（本地）",
        api: "openai-completions",
        baseUrl: "http://localhost:11434/v1",
        envKey: "OLLAMA_API_KEY",
        group: "本地",
        local: true,
        models: [],
      },
    ],
  };
}

test("按「协议 + 端点」反查供应商：忽略尾斜杠与大小写", () => {
  const catalog = catalogFixture();
  assert.equal(matchProvider(catalog, "openai-completions", "https://api.deepseek.com/v1")?.id, "deepseek");
  assert.equal(matchProvider(catalog, "openai-completions", "https://api.deepseek.com/v1/")?.id, "deepseek");
  assert.equal(matchProvider(catalog, "openai-completions", "HTTPS://API.DEEPSEEK.COM/v1")?.id, "deepseek");
  // 协议不同不能误命中（同一个端点可能同时有 OpenAI / Anthropic 两种协议）
  assert.equal(matchProvider(catalog, "anthropic-messages", "https://api.deepseek.com/v1"), undefined);
  assert.equal(matchProvider(catalog, "openai-completions", ""), undefined);
  assert.equal(matchProvider(null, "openai-completions", "https://api.deepseek.com/v1"), undefined);
});

test("本地主机别名归一：localhost / 127.0.0.1 / [::1] 视为同一台", () => {
  const catalog = catalogFixture();
  // 目录里 ollama 写的是 http://localhost:11434/v1，老数据可能存 127.0.0.1
  assert.equal(matchProvider(catalog, "openai-completions", "http://127.0.0.1:11434/v1")?.id, "ollama");
  assert.equal(matchProvider(catalog, "openai-completions", "http://[::1]:11434/v1")?.id, "ollama");
  assert.equal(matchProvider(catalog, "openai-completions", "http://LOCALHOST:11434/v1/")?.id, "ollama");
  // 端口不同就不是同一台
  assert.equal(matchProvider(catalog, "openai-completions", "http://127.0.0.1:1234/v1"), undefined);
  // 非本地主机不做别名折叠
  assert.equal(matchProvider(catalog, "openai-completions", "https://api.deepseek.com/v2"), undefined);
});

test("模型选项：保持目录顺序，label 带 id（同名/多代模型靠它区分）", () => {
  const catalog = catalogFixture();
  const deepseek = matchProvider(catalog, "openai-completions", "https://api.deepseek.com/v1");
  const choices = modelChoices(deepseek);
  assert.deepEqual(
    choices.map((choice) => choice.value),
    ["deepseek-chat", "deepseek-reasoner"],
  );
  for (const choice of choices) {
    assert.ok(choice.label.includes(choice.value), `${choice.label} 应包含模型 id`);
  }
  assert.equal(modelChoices(undefined).length, 0);
});

test("findCatalogModel 只认目录里的 id（含空白归一化）", () => {
  const catalog = catalogFixture();
  const deepseek = matchProvider(catalog, "openai-completions", "https://api.deepseek.com/v1");
  assert.equal(findCatalogModel(deepseek, "deepseek-reasoner")?.output, 65536);
  assert.equal(findCatalogModel(deepseek, "  deepseek-reasoner  ")?.output, 65536);
  assert.equal(findCatalogModel(deepseek, "不在目录"), undefined);
  assert.equal(findCatalogModel(undefined, "deepseek-chat"), undefined);
  assert.equal(findCatalogModel(deepseek, "   "), undefined);
});

test("限额回填：目录有数据用目录，没有则保留调用点旧值", () => {
  const model = findCatalogModel(undefined, "x") ?? { id: "x", name: "X", output: 65536, context: 128000 };
  assert.equal(resolveMaxTokens(model, 8192), 65536);
  assert.equal(resolveContextWindow(model, 0), 128000);
  // 手填的模型没有元数据 → 旧值原样保留（不能在切供应商/改 ID 时被复位成默认）
  assert.equal(resolveMaxTokens(undefined, 4096), 4096);
  assert.equal(resolveContextWindow(undefined, 200000), 200000);
  assert.equal(resolveMaxTokens({ id: "x", name: "X", output: 0 }, 1234), 1234);
  assert.equal(resolveContextWindow({ id: "x", name: "X", context: 0 }, 4321), 4321);
});

test("分组选项：顺序跟随 PROVIDER_GROUPS，且丢掉空分组", () => {
  const groups = providerGroups(catalogFixture());
  const labels = groups.map((group) => group.label);
  assert.deepEqual(labels, ["国内", "国际", "本地"]);
  assert.deepEqual(
    labels.map((label) => PROVIDER_GROUPS.indexOf(label as (typeof PROVIDER_GROUPS)[number])),
    [1, 2, 3],
  );
  assert.deepEqual(groups[0].options, [{ value: "deepseek", label: "DeepSeek" }]);
});

test("预设种子不含任何密钥字段", () => {
  const catalog = catalogFixture();
  const seed = providerSeed(catalog.providers[0]);
  assert.deepEqual(Object.keys(seed).sort(), ["api", "baseUrl", "envKey", "id", "name"]);
  assert.equal(seed.baseUrl, "https://api.deepseek.com/v1");
  assert.equal(seed.envKey, "DEEPSEEK_API_KEY");
  assert.ok(!("apiKey" in seed));
});

test("目录不可用时的兜底：常用清单非空且 label 带 id", () => {
  const choices = fallbackModelChoices();
  assert.equal(choices.length, FALLBACK_MODELS.length);
  assert.ok(choices.length >= 4);
  for (const choice of choices) {
    assert.ok(choice.label.includes(choice.value));
  }
});

test("来源文案区分「本次刷新」与「本地缓存/过期」", () => {
  const catalog = catalogFixture();
  assert.equal(catalogSourceLabel(catalog), "models.dev");
  assert.equal(catalogSourceLabel({ ...catalog, source: "cache" }), "本地缓存");
  assert.equal(catalogSourceLabel({ ...catalog, source: "cache", stale: true }), "本地缓存（刷新失败，可能过期）");
});
