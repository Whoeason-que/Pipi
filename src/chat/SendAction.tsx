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

/** The available actions follow the current run state. */
export default function SendAction({ running, stopping, busy, stopLabel, sendLabel, steerDisabled, sendDisabled, onStop, onSteer, onSend }: Props) {
  const { t } = useI18n();
  const steerLabel = t("chat.steer");
  return <div className="send-action" role="group" aria-label={t("chat.messageActions")}>
    {busy && <span className="send-waiting" role="status">{t("chat.waiting")}</span>}
    {running && <button type="button" className="traffic-button traffic-stop" disabled={stopping} title={stopLabel} aria-label={stopLabel} onClick={onStop}><IconStop /><span>{t("chat.stop")}</span></button>}
    {running && <button type="button" className="traffic-button traffic-steer" disabled={steerDisabled} title={steerLabel} aria-label={steerLabel} onClick={onSteer}><IconArrowUpRight /><span>{t("chat.guideShort")}</span></button>}
    <button type="button" className="traffic-button traffic-send" disabled={sendDisabled} title={sendLabel} aria-label={sendLabel} onClick={onSend}><span>{sendLabel}</span><IconSend /></button>
  </div>;
}
