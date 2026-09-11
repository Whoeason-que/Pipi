import assert from "node:assert/strict";
import test from "node:test";
import {
  PROVIDER_GROUPS,
  PROVIDER_PRESETS,
  findProviderPreset,
  findProviderPresetByEndpoint,
  presetToProviderFields,
} from "../src/providers.ts";

const LOCAL_HOSTS = ["http://localhost", "http://127.0.0.1", "http://[::1]"];

test("预设 id 唯一且都归属已知分组", () => {
  const ids = new Set<string>();
  for (const preset of PROVIDER_PRESETS) {
    assert.ok(preset.id, "预设 id 不能为空");
    assert.ok(!ids.has(preset.id), `预设 id 重复：${preset.id}`);
    ids.add(preset.id);
    assert.ok(
      PROVIDER_GROUPS.includes(preset.group),
      `${preset.id} 的分组 ${preset.group} 不在 PROVIDER_GROUPS 里`,
    );
  }
  assert.ok(PROVIDER_PRESETS.length >= 20, "预设数量过少，疑似目录生成失败");
});

test("协议只取 Pipi 支持的两种，且两类都有", () => {
  for (const preset of PROVIDER_PRESETS) {
    assert.ok(
      preset.api === "anthropic-messages" || preset.api === "openai-completions",
      `${preset.id} 的协议 ${preset.api} 不受支持`,
    );
  }
  assert.ok(PROVIDER_PRESETS.some((p) => p.api === "anthropic-messages"));
  assert.ok(PROVIDER_PRESETS.filter((p) => p.api === "openai-completions").length >= 10);
});

test("baseUrl 形态合法：本地用 http://localhost，其余必须 https", () => {
  for (const preset of PROVIDER_PRESETS) {
    const isLocal = LOCAL_HOSTS.some((host) => preset.baseUrl.startsWith(host));
    if (isLocal) continue;
    assert.ok(
      preset.baseUrl.startsWith("https://"),
      `${preset.id} 的端点既不是本地地址也不是 https：${preset.baseUrl}`,
    );
    assert.ok(!preset.baseUrl.endsWith("/"), `${preset.id} 的端点不应以 / 结尾`);
  }
  const localPresets = PROVIDER_PRESETS.filter((p) => LOCAL_HOSTS.some((h) => p.baseUrl.startsWith(h)));
  assert.ok(localPresets.length >= 3, "本地运行时预设应有多个（ollama / LM Studio / llama.cpp / vLLM）");
});

test("环境变量名与模型清单可用", () => {
  for (const preset of PROVIDER_PRESETS) {
    assert.match(
      preset.envKey,
      /^[A-Za-z0-9][A-Za-z0-9_]*$/,
      `${preset.id} 的 envKey 形态异常：${preset.envKey}`,
    );
    assert.ok(preset.name.trim().length > 0, `${preset.id} 缺少名称`);
    const modelIds = new Set<string>();
    for (const model of preset.models) {
      assert.ok(model.id.trim().length > 0, `${preset.id} 存在空的模型 id`);
      assert.ok(!modelIds.has(model.id), `${preset.id} 模型 id 重复：${model.id}`);
      modelIds.add(model.id);
      if (model.context != null) assert.ok(model.context > 0, `${preset.id}/${model.id} 上下文非正数`);
      if (model.output != null) assert.ok(model.output > 0, `${preset.id}/${model.id} 输出上限非正数`);
    }
  }
  const withModels = PROVIDER_PRESETS.filter((p) => p.models.length > 0);
  assert.ok(withModels.length >= 20, "绝大多数预设应带模型清单（本地运行时可例外）");
  // 目录里来的预设必须带模型：空的说明 CURATION 收了一个没有工具调用模型的厂商
  for (const preset of PROVIDER_PRESETS) {
    if (preset.group === "本地") continue;
    assert.ok(
      preset.models.length > 0,
      `${preset.id} 没有任何支持工具调用的模型，应移出 CURATION 或改为手动条目`,
    );
  }
});

test("按 id 与按端点查找都能命中（端点忽略大小写与尾斜杠）", () => {
  const deepseek = findProviderPreset("deepseek");
  assert.ok(deepseek);
  assert.equal(deepseek.api, "openai-completions");
  assert.match(deepseek.baseUrl, /\/v1$/);
  assert.equal(findProviderPreset("不存在的提供商"), undefined);

  const byEndpoint = findProviderPresetByEndpoint("openai-completions", "https://api.deepseek.com/v1/");
  assert.equal(byEndpoint?.id, "deepseek");
  assert.equal(
    findProviderPresetByEndpoint("openai-completions", "HTTPS://API.DEEPSEEK.COM/v1")?.id,
    "deepseek",
  );
  // 协议不同不应误命中
  assert.equal(findProviderPresetByEndpoint("anthropic-messages", "https://api.deepseek.com/v1"), undefined);
  assert.equal(findProviderPresetByEndpoint("openai-completions", ""), undefined);
});

test("Anthropic 协议预设使用各家 Anthropic 兼容端点", () => {
  const anthropic = findProviderPreset("anthropic");
  assert.ok(anthropic);
  assert.equal(anthropic.api, "anthropic-messages");
  assert.equal(anthropic.baseUrl, "https://api.anthropic.com");
  // rig 会剥掉尾部 /v1，所以厂商端点原样保留即可
  assert.equal(findProviderPreset("minimax-cn")?.api, "anthropic-messages");
  assert.equal(findProviderPreset("deepseek-anthropic")?.baseUrl, "https://api.deepseek.com/anthropic");
});

test("presetToProviderFields 只带非密钥字段", () => {
  const preset = findProviderPreset("groq");
  assert.ok(preset);
  const fields = presetToProviderFields(preset);
  assert.equal(fields.id, "groq");
  assert.equal(fields.api, preset.api);
  assert.equal(fields.baseUrl, preset.baseUrl);
  assert.equal(fields.envKey, preset.envKey);
  assert.ok(!("apiKey" in fields), "预设不得携带密钥字段");
});

test("(协议, 端点) 组合唯一，避免模型列表错配", () => {
  const seen = new Map<string, string>();
  for (const preset of PROVIDER_PRESETS) {
    const key = `${preset.api}|${preset.baseUrl.trim().replace(/\/+$/, "").toLowerCase()}`;
    const previous = seen.get(key);
    assert.equal(
      previous,
      undefined,
      `${preset.id} 与 ${previous} 的 (协议, 端点) 完全相同，模型列表会错配`,
    );
    seen.set(key, preset.id);
  }
});

test("本地运行时预设指向本机端口且给出密钥占位说明", () => {
  const ollama = findProviderPreset("ollama");
  assert.ok(ollama);
  assert.equal(ollama.baseUrl, "http://localhost:11434/v1");
  assert.match(ollama.note ?? "", /密钥/);
  assert.equal(findProviderPreset("vllm")?.baseUrl, "http://localhost:8000/v1");
});
