import { IconArrowUpRight, IconSend, IconStop } from "../icons";
import { useI18n } from "../i18n";

interface Props {
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
export default function SendAction({ running, stopping, busy, stopLabel, sendLabel, steerDisabled, sendDisabled, onStop, onSteer, onSend }: Props) {
  const { t } = useI18n();
  const steerLabel = t("chat.steer");
  return <div className="send-action" role="group" aria-label={t("chat.messageActions")}>
    {busy && <span className="send-waiting" role="status">{t("chat.waiting")}</span>}
    <button type="button" className="traffic-button traffic-stop" disabled={!running || stopping} title={stopLabel} aria-label={stopLabel} onClick={onStop}><IconStop /></button>
    <button type="button" className="traffic-button traffic-steer" disabled={steerDisabled} title={steerLabel} aria-label={steerLabel} onClick={onSteer}><IconArrowUpRight /></button>
    <button type="button" className="traffic-button traffic-send" disabled={sendDisabled} title={sendLabel} aria-label={sendLabel} onClick={onSend}><IconSend /></button>
  </div>;
}
