import { IconArrowUpRight, IconSend, IconStop } from "../icons";

interface Props {
  en: boolean;
  running: boolean;
  stopping: boolean;
  busy: boolean;
  stopLabel: string;
  sendLabel: string;
  steerDisabled: boolean;
  sendDisabled: boolean;
  onStop: () => void;
  onSteer: () => void;
  onSend: () => void;
}

/** Red stops, yellow guides the current run, and green sends or queues. */
export default function SendAction({ en, running, stopping, busy, stopLabel, sendLabel, steerDisabled, sendDisabled, onStop, onSteer, onSend }: Props) {
  const steerLabel = en ? "Guide the current task · Alt+Enter" : "引导当前任务 · Alt+Enter";
  return <div className="send-action" role="group" aria-label={en ? "Message actions" : "消息操作"}>
    {busy && <span className="send-waiting" role="status">{en ? "Waiting…" : "等待中…"}</span>}
    <button type="button" className="traffic-button traffic-stop" disabled={!running || stopping} title={stopLabel} aria-label={stopLabel} onClick={onStop}><IconStop /></button>
    <button type="button" className="traffic-button traffic-steer" disabled={steerDisabled} title={steerLabel} aria-label={steerLabel} onClick={onSteer}><IconArrowUpRight /></button>
    <button type="button" className="traffic-button traffic-send" disabled={sendDisabled} title={sendLabel} aria-label={sendLabel} onClick={onSend}><IconSend /></button>
  </div>;
}
