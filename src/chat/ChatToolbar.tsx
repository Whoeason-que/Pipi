import { NavigationToggle } from "../app/AppNavigation";
import { useI18n } from "../i18n";
import { IconGear, IconPanelRight } from "../icons";

interface ChatToolbarProps {
  agentName: string;
  title: string;
  running: boolean;
  temporary: boolean;
  ready: boolean;
  clearing: boolean;
  inspectorOpen: boolean;
  hasDetail: boolean;
  onInspectorToggle: () => void;
  onBack?: () => void;
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
  hasDetail,
  onInspectorToggle,
  onBack,
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
      <span className="truncate caption crumb chat-title" title={`${agentName} · ${title}`}>
        <button
          type="button"
          className="truncate agent-tag"
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
        <button
          type="button"
          className={`icon-btn chat-inspector-toggle${inspectorOpen ? " active" : ""}`}
          onClick={onInspectorToggle}
          aria-expanded={inspectorOpen}
          aria-controls="session-inspector"
          aria-label={locale === "en" ? "Session inspector" : "会话检查器"}
          title={locale === "en" ? "Session inspector" : "会话检查器"}
        >
          <IconPanelRight />
          {hasDetail && !inspectorOpen && <span className="tab-dot" />}
        </button>
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
    </header>
  );
}
