// 设置右栏：当前分区说明、保存策略与本机数据位置。

import { useRightRail, RightRail } from "../app/right-rail";
import { IconGear } from "../icons";
import type { Settings } from "../types";
import { getSettingsSectionInfo, SettingsSectionIcon, type SettingsSection } from "./settings-sections";

export function SettingsRail({
  controller,
  section,
  settings,
  t,
  showLogout,
  onClose,
}: {
  controller: ReturnType<typeof useRightRail>;
  section: SettingsSection;
  settings: Settings;
  t: (key: string) => string;
  showLogout?: boolean;
  onClose: () => void;
}) {
  const info = getSettingsSectionInfo(section, t);
  const defaultProvider = settings.providers.find((provider) => provider.id === settings.defaultProviderId);
  return <RightRail
    controller={controller}
    id="settings-rail"
    label={info.label}
    subtitle="设置 · 自动保存"
    icon={<SettingsSectionIcon section={section} />}
    tabs={[
      {
        id: "section",
        label: "分区",
        content: <SectionInfo description={info.description} section={section} showLogout={showLogout} onClose={onClose} />,
      },
      {
        id: "storage",
        label: "存储",
        content: <StorageInfo settings={settings} defaultProvider={defaultProvider?.id ?? null} />,
      },
    ]}
  />;
}

function SectionInfo({ description, section, showLogout, onClose }: {
  description: string;
  section: SettingsSection;
  showLogout?: boolean;
  onClose: () => void;
}) {
  return <>
    <section className="right-rail-section">
      <p>{description}</p>
      <div className="right-rail-kv"><span>保存</span><span>{section === "remote" ? "手动应用" : "自动保存"}</span></div>
      <div className="right-rail-kv"><span>分区</span><span className="mono">{section}</span></div>
      {showLogout && <div className="right-rail-kv"><span>账户</span><span>Web 远程访问</span></div>}
    </section>
    <section className="right-rail-section">
      <h3>提示</h3>
      <p className="right-rail-note">
        设置修改会立即写入本机文件，不需要「保存」按钮。按 <kbd>Esc</kbd> 返回之前的界面。
      </p>
      <div className="right-rail-actions-row">
        <button type="button" className="btn small ghost" onClick={onClose}>返回</button>
      </div>
    </section>
  </>;
}

function StorageInfo({ settings, defaultProvider }: {
  settings: Settings;
  defaultProvider: string | null;
}) {
  return <>
    <section className="right-rail-section">
      <h3>文件</h3>
      <div className="right-rail-item active" style={{ cursor: "default" }}>
        <IconGear />
        <span className="right-rail-item-main mono">~/.pipi/settings.json</span>
      </div>
      <p className="right-rail-note">纯文件存储，无数据库、无遥测；修改由宿主原子写回。</p>
    </section>
    <section className="right-rail-section">
      <h3>当前值</h3>
      <div className="right-rail-kv"><span>主题</span><span className="mono">{settings.theme}</span></div>
      <div className="right-rail-kv"><span>提供商</span><span>{settings.providers.length} 个</span></div>
      <div className="right-rail-kv"><span>默认提供商</span><span className="mono">{defaultProvider ?? "未设置"}</span></div>
      <div className="right-rail-kv"><span>压缩</span><span>{settings.compaction.forkBeforeCompact ? "分叉新会话" : "就地替换"}{settings.compaction.archiveOriginal ? " · 原会话归档" : ""}</span></div>
      <div className="right-rail-kv"><span>重试</span><span>{settings.retry.maxAttempts} 次 · {settings.retry.baseDelayMs}ms → {settings.retry.maxDelayMs}ms</span></div>
    </section>
  </>;
}
