import { NavigationToggle } from "../app/AppNavigation";
import { useI18n } from "../i18n";
import { IconGear, IconPlus } from "../icons";
import {
  INSPECTOR_TABS,
  InspectorTabIcon,
  getInspectorTabLabel,
  type InspectorTab,
} from "./inspector-tabs";

interface ChatToolbarProps {
  agentName: string;
  title: string;
  running: boolean;
  temporary: boolean;
  ready: boolean;
  clearing: boolean;
  inspectorOpen: boolean;
  activeTab: InspectorTab;
  hasDetail: boolean;
  onTabClick: (tab: InspectorTab) => void;
  onInspectorToggle: () => void;
  onBack?: () => void;
  onNewSession?: () => void;
  onClear?: () => void;
}

export default function ChatToolbar({
  agentName,
  title,
  running,
  temporary,
  ready,
  clearing,
  inspectorOpen,
  activeTab,
  hasDetail,
  onTabClick,
  onInspectorToggle,
  onBack,
  onNewSession,
  onClear,
}: ChatToolbarProps) {
  const { locale, t } = useI18n();
  return (
    <header
      className="screen-bar chat-toolbar"
      aria-label={locale === "en" ? "Session toolbar" : "会话工具栏"}
    >
      <NavigationToggle />
      <span className="toolbar-divider" />
      <span className="crumb chat-title" title={`${agentName} · ${title}`}>
        <button
          type="button"
          className="agent-tag"
          onClick={onBack}
          disabled={!onBack}
          title={t("workspace.agentInfo")}
        >
          {agentName}
        </button>
        <span className="chat-title-separator" aria-hidden="true">·</span>
        <b>
          {temporary
            ? locale === "en"
              ? "Agent workbench"
              : "Agent 工作台"
            : title}
        </b>
      </span>
      {running && (
        <span className="running-pill">
          <span className="running-dot pulse" />
          {t("common.running")}
        </span>
      )}
      <span className="spacer" />
      {!temporary ? (
        <div
          className="bar-tabs"
          role="tablist"
          aria-label={
            locale === "en" ? "Session inspector" : "会话检查器选项卡"
          }
        >
          {INSPECTOR_TABS.map(({ id }) => {
            const label = getInspectorTabLabel(id, t);
            const selected = inspectorOpen && activeTab === id;
            return (
              <button
                key={id}
                type="button"
                role="tab"
                id={`bar-tab-${id}`}
                aria-selected={selected}
                aria-controls={`ipanel-${id}`}
                aria-label={label}
                className={`bar-tab${selected ? " active" : ""}`}
                onClick={() => onTabClick(id)}
                title={`${label}${selected ? (locale === "en" ? " (click to close)" : "（点击收起）") : ""}`}
              >
                <InspectorTabIcon tab={id} className="bar-tab-icon" />
                <span className="bar-tab-title">{label}</span>
                {id === "detail" && hasDetail && <span className="tab-dot" />}
              </button>
            );
          })}
        </div>
      ) : (
        <>
          <span className="test-mode-pill">
            <span className={`test-mode-dot${running ? " running" : ""}`} />
            {locale === "en" ? "Temporary test" : "临时测试"}
          </span>
          <button
            type="button"
            className={`icon-btn test-settings-toggle${inspectorOpen ? " active" : ""}`}
            onClick={onInspectorToggle}
            aria-expanded={inspectorOpen}
            aria-controls="session-inspector"
            aria-label={locale === "en" ? "Agent settings" : "Agent 设置"}
            title={locale === "en" ? "Agent settings" : "Agent 设置"}
          >
            <IconGear />
          </button>
          {onClear && (
            <button
              type="button"
              className="btn ghost test-clear"
              onClick={onClear}
              disabled={!ready || running || clearing}
              title={
                running
                  ? locale === "en"
                    ? "Stop the test first"
                    : "请先停止测试"
                  : locale === "en"
                    ? "Clear temporary context"
                    : "清空临时上下文"
              }
            >
              {clearing
                ? locale === "en"
                  ? "Clearing…"
                  : "清空中…"
                : locale === "en"
                  ? "Clear test"
                  : "清空测试"}
            </button>
          )}
        </>
      )}
      {onNewSession && (
        <button
          type="button"
          className="icon-btn chat-new-session"
          onClick={onNewSession}
          title={t("nav.newSession")}
          aria-label={t("nav.newSession")}
        >
          <IconPlus />
        </button>
      )}
    </header>
  );
}
