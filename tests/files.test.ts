import assert from "node:assert/strict";
import test from "node:test";
import { gitStatusMeta, parsePatch } from "../src/files/diff.ts";
import { FileResourceCache } from "../src/files/resource-cache.ts";
import { codeFence, formatReference, linkedPath, makeReference, markdownImagePath, toolFiles } from "../src/files/model.ts";
import type { WorkspaceFile } from "../src/files/types.ts";
import { WorkspaceViewStore } from "../src/files/workspace-view.ts";
import { chatReducer, INITIAL_CHAT_STATE, type MessageView } from "../src/chat-runtime.ts";

const file: WorkspaceFile = { workspace: "/project", path: "src/你好.ts", size: 30, version: "sha256-test", kind: "text", mime: "text/plain", content: "one\ntwo\nthree", base64: null, truncated: false };

test("读取缓存按完整身份隔离，更新只替换相同的请求", () => {
  const cache = new FileResourceCache();
  const a = JSON.stringify(["preview", "/one:files", "a.ts"]);
  cache.set(a, file);
  assert.equal(cache.get(a), file);
  for (const key of [["preview", "/two:files", "a.ts"], ["preview", "/one:git", "a.ts"], ["listing", "/one:files", "a.ts"], ["preview", "/one:files", "b.ts"]]) {
    assert.equal(cache.get(JSON.stringify(key)), null);
  }
  cache.set(a, { ...file, content: "updated" });
  assert.equal(cache.get<WorkspaceFile>(a)?.content, "updated");
});

test("读取缓存限制数量和内容大小，更新会回收旧条目占用", () => {
  const cache = new FileResourceCache(2, 24);
  cache.set("a", "one"); // 10 bytes
  cache.set("b", "two");
  cache.set("a", "new");
  cache.set("c", "end");
  assert.equal(cache.get("b"), null);
  assert.equal(cache.get("a"), "new");
  cache.set("large", "x".repeat(20));
  assert.equal(cache.get("large"), null);
  assert.equal(cache.get("c"), "end");
  cache.set("a", "123456789"); // 22 bytes; evicts c by byte budget
  assert.equal(cache.get("c"), null);
  assert.equal(cache.get("a"), "123456789");
});

test("引用保留工作区和版本，反向选区按实际行号保存，不修改源内容", () => {
  const reference = makeReference(file, [3, 2], "  修改这里  ");
  assert.deepEqual(reference, { workspace: "/project", path: "src/你好.ts", version: "sha256-test", startLine: 2, endLine: 3, excerpt: "two\nthree", comment: "修改这里" });
  assert.match(formatReference(reference), /sha256-test/);
  assert.match(formatReference(reference), /L2–3/);
  assert.equal(file.content, "one\ntwo\nthree");
  assert.throws(() => makeReference({ ...file, content: "x".repeat(16_001) }, [1, 1], ""), /选区过大/);
});

test("截断预览不伪造完整版本和选区，代码围栏不会被源文件中的围栏截断", () => {
  const reference = makeReference({ ...file, truncated: true, version: "" }, [1, 2], "");
  assert.equal(reference.excerpt, undefined);
  assert.equal(reference.startLine, undefined);
  assert.match(formatReference(reference), /"sha256":null/);
  assert.equal(codeFence("```\nsource\n```"), "````\n```\nsource\n```\n````");
  assert.doesNotThrow(() => codeFence("` ".repeat(130_000)));
});

test("Markdown 本地链接正确解析，网络和越界链接不作为工作区读取", () => {
  assert.equal(linkedPath("docs/readme.md", "../src/%E4%BD%A0%E5%A5%BD.ts#L2"), "src/你好.ts");
  assert.equal(linkedPath("docs/readme.md", "../../secret"), null);
  for (const url of ["https://example.com/a", "javascript:alert(1)", "//example.com/a", "data:text/html,a", "%ZZ", "#section"]) assert.equal(linkedPath("docs/readme.md", url), null);
});

test("聊天图片支持工作区相对路径、绝对路径和本地 file URL，不把网络地址当成本地文件", () => {
  assert.equal(markdownImagePath("./.agents/report%20one.png"), ".agents/report one.png");
  assert.equal(markdownImagePath("/project/%E5%9B%BE%E7%89%87.png#preview"), "/project/图片.png");
  assert.equal(markdownImagePath("file:///project/report%20one.png"), "/project/report one.png");
  assert.equal(markdownImagePath("file://localhost/project/chart.png"), "/project/chart.png");
  assert.equal(markdownImagePath("file:///C:/project/chart.png"), "C:/project/chart.png");
  assert.equal(markdownImagePath("C:\\project\\chart.png"), "C:/project/chart.png");
  for (const path of ["https://example.com/a.png", "//example.com/a.png", "file://remote/project/a.png", "data:image/svg+xml,test", "javascript:alert(1)", "blob:other", "../outside.png", "%ZZ", ""]) {
    assert.equal(markdownImagePath(path), null, path);
  }
});

test("会话文件保留多次操作与失败标记，不把它们合成为工作区历史 diff", () => {
  const result = toolFiles([
    { key: "write-1", role: "toolResult", text: "", toolName: "write", toolArgs: { path: "a.txt" } },
    { key: "edit-1", role: "toolResult", text: "", toolName: "edit", toolArgs: { path: "a.txt" }, toolDetails: { diff: "+new" }, isError: true },
    { key: "bash-1", role: "toolResult", text: "", toolName: "bash", toolArgs: { path: "ignored" } },
  ]);
  assert.equal(result.length, 1);
  assert.equal(result[0].calls, 2);
  assert.equal(result[0].errors, 1);
  assert.equal(result[0].operations[0].patch, null);
  assert.equal(result[0].operations[1].patch, "+new");
});

test("File 与 Git 的状态按工作区隔离，刷新保留目录、标签和阅读位置", () => {
  const values = new Map<string, string>();
  const storage = { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); } };
  const agent = { name: "coder", workspace: "/one" };
  const files = new WorkspaceViewStore(agent, "files", storage);
  let changes = 0;
  files.subscribe(() => { changes++; });
  files.open("src/nested/a.ts");
  files.rememberScroll("src/nested/a.ts", 200);
  assert.equal(changes, 1, "滚动不触发整页重新渲染");
  files.setQuery("a.ts");
  files.setHidden(true);
  const restored = new WorkspaceViewStore(agent, "files", storage);
  assert.deepEqual(restored.getSnapshot().expanded, ["src", "src/nested"]);
  assert.equal(restored.getSnapshot().selected, "src/nested/a.ts");
  assert.equal(restored.scroll("src/nested/a.ts"), 200);
  assert.equal(restored.getSnapshot().query, "a.ts");
  assert.equal(restored.getSnapshot().showHidden, true);
  for (const other of [new WorkspaceViewStore(agent, "git", storage), new WorkspaceViewStore({ ...agent, workspace: "/two" }, "files", storage), new WorkspaceViewStore({ ...agent, name: "reviewer" }, "files", storage)]) {
    assert.equal(other.getSnapshot().selected, null);
    assert.equal(other.scroll("src/nested/a.ts"), 0);
  }
  for (let index = 0; index < 12; index++) restored.open(`file-${index}.ts`);
  assert.equal(restored.getSnapshot().tabs.length, 8);
  assert.equal(restored.scroll("src/nested/a.ts"), 0, "移出的标签不保留内容和滚动缓存");
  restored.close("file-11.ts");
  assert.equal(restored.getSnapshot().selected, "file-10.ts");
});

test("历史会话按工具调用 id 和名称恢复文件参数与差异，多次水合不丢失", () => {
  const messages: MessageView[] = [
    { role: "assistant", content: [{ type: "toolCall", id: "edit-one", name: "edit", arguments: { path: "src/a.ts" } }, { type: "toolCall", id: "edit-two", name: "edit", arguments: { path: "src/b.ts" } }] },
    { role: "toolResult", toolCallId: "edit-two", toolName: "edit", content: "edited", details: { diff: "-old b\n+new b" } },
    { role: "toolResult", toolCallId: "edit-one", toolName: "edit", content: "edited", details: { diff: "-old a\n+new a" } },
    { role: "toolResult", toolCallId: "edit-one", toolName: "read", content: "mismatch" },
  ];
  const hydrate = { type: "hydrate" as const, messages, stats: null, running: false };
  const first = chatReducer(INITIAL_CHAT_STATE, hydrate);
  const files = toolFiles(first.entries);
  assert.deepEqual(files.map(file => file.path), ["src/a.ts", "src/b.ts"]);
  assert.equal(files[0].operations[0].patch, "-old a\n+new a");
  const again = chatReducer(first, hydrate);
  assert.deepEqual(toolFiles(again.entries), files);
  assert.equal(again.entries.at(-1)?.toolArgs, undefined, "工具名不匹配时不能套用其他调用的参数");
});

test("diff 解析保留双行号与增删统计，末尾空行不算新增内容", () => {
  const patch = "diff --git a/a.ts b/a.ts\n--- a/a.ts\n+++ b/a.ts\n@@ -2,3 +2,4 @@\n context\n-old\n+new\n+extra\n tail\n";
  const parsed = parsePatch(patch);
  assert.equal(parsed.added, 2);
  assert.equal(parsed.removed, 1);
  assert.equal(parsed.hunks, 1);
  assert.deepEqual(
    parsed.rows.filter(row => row.kind !== "meta" && row.kind !== "hunk").map(row => [row.kind, row.oldLine, row.newLine, row.text]),
    [["context", 2, 2, "context"], ["del", 3, null, "old"], ["add", null, 3, "new"], ["add", null, 4, "extra"], ["context", 4, 5, "tail"]],
  );
  assert.equal(parsePatch("@@ new file @@\n+a\n+b\n").added, 2);
});

test("git 状态码按暂存区与工作树拆分，未跟踪文件不伪造编辑状态", () => {
  assert.deepEqual(gitStatusMeta("M "), { code: "M ", label: "修改", tone: "modified", staged: true, unstaged: false });
  assert.deepEqual(gitStatusMeta(" M"), { code: " M", label: "修改", tone: "modified", staged: false, unstaged: true });
  assert.deepEqual(gitStatusMeta("??"), { code: "??", label: "未跟踪", tone: "untracked", staged: false, unstaged: true });
  assert.equal(gitStatusMeta("A ").tone, "added");
  assert.equal(gitStatusMeta(" D").tone, "deleted");
  assert.equal(gitStatusMeta("UU").tone, "conflict");
});

test("多仓库与分支元数据兼容 DTO 契约并正确支持状态归属", () => {
  const changesWithRepos: import("../src/files/types.ts").WorkspaceChanges = {
    workspace: "/workspace",
    isRepo: true,
    repositories: [
      { name: "Root", path: "", branch: "main" },
      { name: "Frontend", path: "packages/frontend", branch: "feat-ui" }
    ],
    files: [
      { path: "README.md", status: " M", repository: "Root" },
      { path: "packages/frontend/src/App.tsx", status: "M ", repository: "Frontend" }
    ]
  };
  assert.equal(changesWithRepos.repositories?.length, 2);
  assert.equal(changesWithRepos.files[1].repository, "Frontend");
  assert.equal(changesWithRepos.repositories?.[1].branch, "feat-ui");
});
