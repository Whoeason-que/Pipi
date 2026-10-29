import { useRef, type CSSProperties, type PointerEvent } from "react";
import { IconClock } from "../icons";

interface Props {
  minutes: number;
  maxMinutes: number;
  onMaxChange: (minutes: number) => void;
  label: string;
  hint: string;
  onChange: (minutes: number) => void;
}

/** Dragging around the edge selects a delay up to the configured maximum. */
export default function DelayDial({ minutes, maxMinutes, onMaxChange, label, hint, onChange }: Props) {
  const gesture = useRef<{ id: number; x: number; y: number; start: number; changed: boolean; angle: number; limit: number } | null>(null);
  const finish = (event: PointerEvent<HTMLButtonElement>, canceled: boolean) => {
    const current = gesture.current;
    if (!current || current.id !== event.pointerId) return;
    gesture.current = null;
    if (canceled && current.changed) onChange(current.start);
  };
  return (
    <button
      type="button"
      className={`btn send composer-send${minutes ? " has-delay" : ""}`}
      style={{ "--dial-progress": `${Math.min(minutes, maxMinutes) / maxMinutes * 100}%` } as CSSProperties}
      title={`${label} · ${hint}`}
      aria-label={`${label}. ${hint}`}
      onPointerDown={event => {
        if (!event.isPrimary || event.button !== 0) return;
        gesture.current = { id: event.pointerId, x: event.clientX, y: event.clientY, start: minutes, changed: false, angle: 0, limit: 0 };
        event.currentTarget.setPointerCapture(event.pointerId);
      }}
      onPointerMove={event => {
        const current = gesture.current;
        if (!current || current.id !== event.pointerId) return;
        if (!current.changed && Math.hypot(event.clientX - current.x, event.clientY - current.y) < 4) return;
        const rect = event.currentTarget.getBoundingClientRect();
        const x = event.clientX - rect.left - rect.width / 2;
        const y = event.clientY - rect.top - rect.height / 2;
        if (Math.hypot(x, y) < rect.width * 0.2) return;
        const turn = Math.PI * 2;
        let angle = (Math.atan2(x, -y) + turn) % turn;
        if (current.changed) {
          const delta = angle - current.angle;
          if (current.limit === 1) { if (angle > Math.PI && angle < turn * 0.985) current.limit = 0; else angle = turn; }
          else if (current.limit === -1) { if (angle < Math.PI && angle > turn * 0.015) current.limit = 0; else angle = 0; }
          else if (delta < -Math.PI) { current.limit = 1; angle = turn; }
          else if (delta > Math.PI) { current.limit = -1; angle = 0; }
        }
        current.changed = true;
        current.angle = angle;
        onChange(Math.max(1, Math.min(maxMinutes, Math.round(angle / turn * maxMinutes))));
      }}
      onPointerUp={event => finish(event, false)}
      onPointerCancel={event => finish(event, true)}
      onLostPointerCapture={event => finish(event, true)}
      onKeyDown={event => {
        if (event.key === "PageUp" || event.key === "PageDown") {
          event.preventDefault();
          onMaxChange(maxMinutes + (event.key === "PageUp" ? 1 : -1));
          return;
        }
        if (event.key !== "ArrowUp" && event.key !== "ArrowRight" && event.key !== "ArrowDown" && event.key !== "ArrowLeft" && event.key !== "Escape") return;
        event.preventDefault();
        onChange(event.key === "Escape" ? 0 : Math.max(0, Math.min(maxMinutes, minutes + (event.key === "ArrowUp" || event.key === "ArrowRight" ? 1 : -1))));
      }}
    >
      <IconClock size={16} />
    </button>
  );
}
