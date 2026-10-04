import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

// 渲染前应用缓存的主题，避免首帧闪烁（真实设置由 get_settings 校准）
const saved = localStorage.getItem("pipi-theme");
document.documentElement.dataset.theme = saved === "light" ? "light" : "dark";

// 开发环境下若没有 Tauri 后端（纯浏览器调试），安装最小 invoke 桩。
// 动态 import：dev-mock 只服务浏览器调试，不进生产 bundle（含演示数据）。
if (import.meta.env.DEV) {
  void import("./dev-mock").then(({ installDevMock }) => installDevMock());
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
