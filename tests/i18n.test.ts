import assert from "node:assert/strict";
import test from "node:test";
import { getTranslation, getDefaultLocale, zh, en } from "../src/i18n/core.ts";
import { classifyChatError } from "../src/chat/chat-errors.ts";

test("i18n dictionaries are complete and match keys", () => {
  // 确保 en 和 zh 的一级命名空间完全一致
  const zhKeys = Object.keys(zh);
  const enKeys = Object.keys(en);
  assert.deepEqual(enKeys.sort(), zhKeys.sort());

  // 确保每个命名空间下的二级字段完全一致
  for (const k of zhKeys) {
    const zhSub = Object.keys((zh as Record<string, Record<string, string>>)[k]);
    const enSub = Object.keys((en as Record<string, Record<string, string>>)[k]);
    assert.deepEqual(enSub.sort(), zhSub.sort(), `Mismatch in namespace ${k}`);
  }
});

test("getTranslation returns correct text in both languages", () => {
  assert.equal(getTranslation("zh", "common.confirm"), "确认");
  assert.equal(getTranslation("en", "common.confirm"), "Confirm");

  assert.equal(getTranslation("zh", "chat.fork"), "分叉会话");
  assert.equal(getTranslation("en", "chat.fork"), "Fork Session");
});

test("getTranslation interpolates parameters correctly", () => {
  assert.equal(
    getTranslation("zh", "inspector.searchResults", { count: 5 }),
    "5 处匹配",
  );
  assert.equal(
    getTranslation("en", "inspector.searchResults", { count: 5 }),
    "5 matches",
  );
});

test("getTranslation falls back gracefully when key is missing", () => {
  // @ts-expect-error non-existent key test
  assert.equal(getTranslation("en", "common.non_existent_key"), "common.non_existent_key");
});

test("classifyChatError supports English and Chinese locale outputs", () => {
  // 1. 后端英文错误在英文 locale 下渲染
  const enAuth = classifyChatError({ errorMessage: "401 Unauthorized: Invalid API Key" }, "en");
  assert.equal(enAuth.category, "auth");
  assert.equal(enAuth.title, "API Authentication Failed or Invalid Credentials");
  assert.equal(enAuth.badge, "Auth Error");
  assert.ok(enAuth.suggestion?.includes("Settings → Providers"));

  // 2. 后端英文错误在中文 locale 下渲染
  const zhAuth = classifyChatError({ errorMessage: "401 Unauthorized: Invalid API Key" }, "zh");
  assert.equal(zhAuth.category, "auth");
  assert.equal(zhAuth.title, "API 认证失败或凭据无效");
  assert.equal(zhAuth.badge, "认证异常");
  assert.ok(zhAuth.suggestion?.includes("设置 → 模型服务"));

  // 3. 限流错误在中英双语下的输出
  const enRate = classifyChatError({ errorMessage: "429 Rate limit reached for requests per minute" }, "en");
  assert.equal(enRate.category, "rate_limit");
  assert.equal(enRate.title, "Rate Limit Exceeded");
  assert.equal(enRate.badge, "Rate Limit");

  const zhRate = classifyChatError({ errorMessage: "429 Rate limit reached for requests per minute" }, "zh");
  assert.equal(zhRate.category, "rate_limit");
  assert.equal(zhRate.title, "请求频率超限");
  assert.equal(zhRate.badge, "频率限制");

  // 4. 上下文超限在中英双语下的输出
  const enContext = classifyChatError({ errorMessage: "maximum context length exceeded" }, "en");
  assert.equal(enContext.category, "context_overflow");
  assert.equal(enContext.title, "Context Exceeded Model Window Limit");
  assert.equal(enContext.badge, "Context Overflow");

  // 5. 用户中止在中英双语下的输出
  const enAbort = classifyChatError({ status: "aborted" }, "en");
  assert.equal(enAbort.category, "aborted");
  assert.equal(enAbort.title, "Generation Aborted by User");
  assert.equal(enAbort.badge, "Aborted");

  const zhAbort = classifyChatError({ status: "aborted" }, "zh");
  assert.equal(zhAbort.category, "aborted");
  assert.equal(zhAbort.title, "生成已由用户中止");
  assert.equal(zhAbort.badge, "已中止");
});
