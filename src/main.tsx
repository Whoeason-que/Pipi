import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { I18nProvider, getDefaultLocale } from "./i18n";
import "./styles.css";

import { applyTheme, getInitialThemeId } from "./theme";

// 渲染前应用缓存的主题与语言，避免首帧闪烁（真实设置由 get_settings 校准）
applyTheme(getInitialThemeId());
document.documentElement.lang = getDefaultLocale();

// 开发环境下若没有 Tauri 后端（纯浏览器调试），安装最小 invoke 桩。
// 动态 import：dev-mock 只服务浏览器调试，不进生产 bundle（含演示数据）。
async function mount() {
  // 开发桩先于 App 的鉴权/事件订阅安装，避免误连真实 Web 端点并显示断开。
  if (import.meta.env.DEV) {
    const { installDevMock } = await import("./dev-mock");
    installDevMock();
  }
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <I18nProvider>
        <App />
      </I18nProvider>
    </React.StrictMode>,
  );
}
void mount();

