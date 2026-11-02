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
  const { t } = useI18n();
  return (
    <header
      className="screen-bar chat-toolbar"
      aria-label={t("chat.toolbar")}
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
            ? t("chat.agentWorkbench")
            : title}
        </b>
      </span>
      <span className="spacer" />
      {!temporary ? (
        <button
          type="button"
          className={`icon-btn chat-inspector-toggle${inspectorOpen ? " active" : ""}`}
          onClick={onInspectorToggle}
          aria-expanded={inspectorOpen}
          aria-controls="session-inspector"
          aria-label={t("inspector.ariaLabel")}
          title={t("inspector.ariaLabel")}
        >
          <IconPanelRight />
          {hasDetail && !inspectorOpen && <span className="tab-dot" />}
        </button>
      ) : (
        <>
          <span className="test-mode-pill">
            <span className={`test-mode-dot${running ? " running" : ""}`} />
            {t("chat.temporaryTest")}
          </span>
          <button
            type="button"
            className={`icon-btn test-settings-toggle${inspectorOpen ? " active" : ""}`}
            onClick={onInspectorToggle}
            aria-expanded={inspectorOpen}
            aria-controls="session-inspector"
            aria-label={t("chat.agentSettings")}
            title={t("chat.agentSettings")}
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
                  ? t("chat.stopTestFirst")
                  : t("chat.clearTemporaryContext")
              }
            >
              {clearing ? t("chat.clearing") : t("chat.clearTest")}
            </button>
          )}
        </>
      )}
    </header>
  );
}
