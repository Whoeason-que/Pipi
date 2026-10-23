// npm run dev 后打开 /tests/images.browser.html。只使用本地假宿主，不访问真实 Agent。
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import { Markdown } from "../src/Markdown";
import { I18nProvider } from "../src/i18n";
import type { WorkspaceFile } from "../src/files/types";
import "../src/styles.css";

const image = fetch("/crates/pipi/icons/128x128.png").then(response => response.arrayBuffer())
  .then(buffer => btoa(Array.from(new Uint8Array(buffer), byte => String.fromCharCode(byte)).join("")));
const calls: string[] = [];
window.__PIPI_DEV_PLATFORM__ = {
  invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
    if (command !== "preview_workspace_file") throw new Error(`Unexpected command: ${command}`);
    const agent = String(args?.agentName);
    const path = String(args?.path);
    calls.push(`${agent}: ${path}`);
    document.getElementById("reads")!.textContent = calls.join("\n");
    if (path.includes("slow")) await new Promise(resolve => setTimeout(resolve, 800));
    const root = `/fixture/${agent}/`;
    if (path.startsWith("/") && !path.startsWith(root)) throw new Error("文件不在此 Agent 的工作目录内");
    if (path.includes("missing")) throw new Error("文件不存在");
    const result: WorkspaceFile = {
      workspace: root, path: path.replace(root, ""), kind: path.endsWith("svg") ? "text" : "image",
      mime: path.endsWith("svg") ? "text/plain" : "image/png", size: 1000, version: "fixture",
      content: null, base64: path.includes("corrupt") ? "bm90IGEgcG5n" : await image,
      truncated: path.includes("large"),
    };
    return result as T;
  },
  listen: async () => () => {},
};

function Fixture() {
  const [agent, setAgent] = useState("one");
  const [src, setSrc] = useState("/fixture/one/report.png");
  const [suffix, setSuffix] = useState("");
  return <main style={{ maxWidth: 760, margin: "32px auto", padding: 20 }}>
    <h1>聊天图片链路回归</h1>
    <div style={{ display: "flex", flexWrap: "wrap", gap: 8, marginBlock: 16 }}>
      <button className="btn" onClick={() => setSrc("/fixture/one/report.png")}>绝对路径</button>
      <button className="btn" onClick={() => setSrc("file:///fixture/one/report%20one.png")}>file URL</button>
      <button className="btn" onClick={() => setSrc("./report.png")}>相对路径</button>
      <button className="btn" onClick={() => setSrc("/fixture/one/missing.png")}>文件缺失</button>
      <button className="btn" onClick={() => setSrc("/fixture/one/large.png")}>超大文件</button>
      <button className="btn" onClick={() => setSrc("/fixture/one/corrupt.png")}>图片损坏</button>
      <button className="btn" onClick={() => setSrc("/fixture/one/test.svg")}>SVG 源码</button>
      <button className="btn" onClick={() => setSrc("https://example.invalid/external.png")}>外部图片</button>
      <button className="btn" onClick={() => setSrc("/fixture/one/slow.png")}>慢请求</button>
      <button className="btn" onClick={() => setAgent(value => value === "one" ? "two" : "one")}>切换 Agent</button>
      <button className="btn" onClick={() => setSuffix(value => value + "\n\n新增一段流式正文。")}>追加正文</button>
    </div>
    <p>当前 Agent：{agent}</p>
    <Markdown text={`![项目报告](${src})${suffix}`} imageAgent={{ name: agent, workspace: `/fixture/${agent}` }} />
    <h2>宿主读取记录</h2><pre id="reads" style={{ whiteSpace: "pre-wrap" }} />
  </main>;
}

const root = createRoot(document.getElementById("root")!);
root.render(<StrictMode><I18nProvider><Fixture /></I18nProvider></StrictMode>);
import.meta.hot?.dispose(() => root.unmount());
