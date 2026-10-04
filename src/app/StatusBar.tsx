import { getRuntimeEndpoint, type ConnectionState } from "../platform";

const CONNECTION_LABELS: Record<ConnectionState, string> = {
  online: "已连接",
  connecting: "连接中",
  offline: "已断开",
  dev: "演示模式",
};

export interface StatusBarProps {
  connection: ConnectionState;
  agentCount: number;
  sessionCount: number;
  runningCount: number;
  /** package.json 注入的版本号；未注入时留空（不渲染版本段）。 */
  version: string;
}

/** 底部状态栏：连接状态 / 运行位置 / 数量统计 / 版本。 */
export function StatusBar({
  connection,
  agentCount,
  sessionCount,
  runningCount,
  version,
}: StatusBarProps) {
  return (
    <footer className="status-bar" aria-label="运行状态">
      <span className="conn">
        <i className={`status-dot ${connection}`} aria-hidden="true" />
        {CONNECTION_LABELS[connection]}
      </span>
      <span className="sep">│</span>
      <span className="opt">{getRuntimeEndpoint()}</span>
      <span className="sep opt">│</span>
      <span className="opt">
        数据根目录 <b>~/.pipi/agents</b>
      </span>
      <span className="sep">│</span>
      <span>
        <b>{agentCount}</b> 个 Agent · <b>{sessionCount}</b> 个会话
      </span>
      <span className="spacer" />
      <span>
        运行中 <b>{runningCount}</b>
      </span>
      {version && (
        <>
          <span className="sep">│</span>
          <span>
            Pipi <b>v{version}</b>
          </span>
        </>
      )}
    </footer>
  );
}
