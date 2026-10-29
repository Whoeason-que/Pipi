// 仅供浏览器开发 mock 使用：全部内容保存在内存中，不读取真实工作区。
import type { WorkspaceChange, WorkspaceDiff, WorkspaceEntry, WorkspaceFile, WorkspaceListing } from "./files/types";

export const DEMO_WORKSPACE = "/demo/pipi-workspace";

const files: Record<string, string> = {
  "README.md": "# Pipi Workspace\n\n这是浏览器开发模式的示例工作区。\n\n- 浏览文件和目录\n- 在 Git 页面查看示例改动\n- 选择代码行并引用到会话\n\n所有内容都是模拟数据。\n",
  "package.json": '{\n  "name": "pipi-workspace-demo",\n  "private": true,\n  "scripts": { "dev": "vite" }\n}\n',
  ".gitignore": "node_modules/\ndist/\n.env\n",
  "src/App.tsx": 'import { Status } from "./components/Status";\nimport "./styles.css";\n\nexport default function App() {\n  return <main className="app"><h1>Hello, Pipi</h1><Status /></main>;\n}\n',
  "src/styles.css": '.app {\n  max-width: 48rem;\n  margin: 0 auto;\n  padding: 2rem;\n  color: #26354a;\n}\n',
  "src/components/Status.tsx": 'export function Status() {\n  return <p>工作区已准备就绪。</p>;\n}\n',
  "crates/core/src/lib.rs": 'pub fn greeting(name: &str) -> String {\n    format!("Hello, {name}!")\n}\n',
  "docs/guide.md": "# 开发指南\n\n从 `src/App.tsx` 开始探索这个示例项目。\n\n[返回 README](../README.md)\n",
};

const changes: WorkspaceChange[] = [
  { path: "src/App.tsx", status: "M ", repository: "pipi-workspace" },
  { path: "src/styles.css", status: " M", repository: "pipi-workspace" },
  { path: "src/components/Status.tsx", status: "??", repository: "pipi-workspace" },
  { path: "docs/old-guide.md", status: " D", repository: "pipi-workspace" },
];
const patches: Record<string, string> = {
  "src/App.tsx": 'diff --git a/src/App.tsx b/src/App.tsx\n--- a/src/App.tsx\n+++ b/src/App.tsx\n@@ -1,5 +1,6 @@\n+import { Status } from "./components/Status";\n import "./styles.css";\n\n export default function App() {\n-  return <main className="app"><h1>Hello, Pipi</h1></main>;\n+  return <main className="app"><h1>Hello, Pipi</h1><Status /></main>;\n }\n',
  "src/styles.css": 'diff --git a/src/styles.css b/src/styles.css\n--- a/src/styles.css\n+++ b/src/styles.css\n@@ -1,5 +1,6 @@\n .app {\n   max-width: 48rem;\n   margin: 0 auto;\n   padding: 2rem;\n+  color: #26354a;\n }\n',
  "src/components/Status.tsx": 'diff --git a/src/components/Status.tsx b/src/components/Status.tsx\nnew file mode 100644\n--- /dev/null\n+++ b/src/components/Status.tsx\n@@ -0,0 +1,3 @@\n+export function Status() {\n+  return <p>工作区已准备就绪。</p>;\n+}\n',
  "docs/old-guide.md": 'diff --git a/docs/old-guide.md b/docs/old-guide.md\ndeleted file mode 100644\n--- a/docs/old-guide.md\n+++ /dev/null\n@@ -1 +0,0 @@\n-旧版开发指南\n',
};

function validateTarget(target: string) {
  if (target !== DEMO_WORKSPACE && target !== "demo-assistant") throw new Error("Workspace is not available in the demo");
}

export function demoWorkspaceListing(target: string, path: string): WorkspaceListing {
  validateTarget(target);
  const prefix = path ? `${path}/` : "";
  const entries = new Map<string, WorkspaceEntry>();
  for (const [filePath, content] of Object.entries(files)) {
    if (!filePath.startsWith(prefix)) continue;
    const [name, ...rest] = filePath.slice(prefix.length).split("/");
    if (!name) continue;
    const entryPath = `${prefix}${name}`;
    if (!entries.has(name)) entries.set(name, { path: entryPath, name, kind: rest.length ? "directory" : "file", size: rest.length ? 0 : new TextEncoder().encode(content).length });
  }
  if (path && entries.size === 0) throw new Error("Directory not found in the demo workspace");
  return { workspace: DEMO_WORKSPACE, path, entries: [...entries.values()].sort((a, b) => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "directory" ? -1 : 1)), truncated: false };
}

export function demoWorkspaceFile(target: string, path: string): WorkspaceFile {
  validateTarget(target);
  if (!Object.hasOwn(files, path)) throw new Error("File not found in the demo workspace");
  const content = files[path];
  return { workspace: DEMO_WORKSPACE, path, size: new TextEncoder().encode(content).length, version: `demo-${path}`, kind: path.endsWith(".md") ? "markdown" : "text", mime: "text/plain", content, base64: null, truncated: false };
}

export function demoWorkspaceChanges(target: string) {
  validateTarget(target);
  return { workspace: DEMO_WORKSPACE, isRepo: true, repositories: [{ name: "pipi-workspace", path: ".", branch: "demo/workspace" }], files: structuredClone(changes) };
}

export function demoWorkspaceDiff(target: string, path: string): WorkspaceDiff {
  validateTarget(target);
  if (!Object.hasOwn(patches, path)) throw new Error("Change not found in the demo workspace");
  const patch = patches[path];
  return { path, base: "HEAD", patch, truncated: false };
}
