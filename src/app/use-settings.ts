import { useRef, useState, type Dispatch, type SetStateAction } from "react";
import { formatRuntimeError } from "../chat-runtime";
import { invoke } from "../platform";
import { applyTheme } from "../theme";
import type { Settings } from "../types";

export interface SettingsState {
  settings: Settings | null;
  /** 初始加载用（get_settings 回调）；保存走 updateSettings。 */
  setSettings: Dispatch<SetStateAction<Settings | null>>;
  updateSettings: (next: Settings) => Promise<void>;
}

/**
 * 全局设置：乐观更新 + 串行保存队列。
 *
 * 每次更新带自增编号：保存失败时若已有更新的保存发起，就不再回滚
 * （否则会把用户后来的修改覆盖掉）。写入 settingsRef 是同步的，连续
 * 调用以最后一次为准。
 */
export function useSettings(
  setError: (message: string | null) => void,
): SettingsState {
  const [settings, setSettings] = useState<Settings | null>(null);
  const settingsRef = useRef<Settings | null>(null);
  const settingsSaveRequestRef = useRef(0);
  const settingsSaveQueueRef = useRef<Promise<void>>(Promise.resolve());
  settingsRef.current = settings;

  const updateSettings = (next: Settings): Promise<void> => {
    const previous = settingsRef.current;
    const requestId = ++settingsSaveRequestRef.current;
    settingsRef.current = next;
    setSettings(next);
    applyTheme(next.theme);

    const save = settingsSaveQueueRef.current.then(async () => {
      try {
        await invoke("save_settings", { settings: next });
      } catch (errorValue) {
        if (requestId === settingsSaveRequestRef.current) {
          settingsRef.current = previous;
          setSettings(previous);
          if (previous) applyTheme(previous.theme);
          setError(formatRuntimeError(errorValue));
        }
      }
    });
    settingsSaveQueueRef.current = save.catch(() => {});
    return save;
  };

  return { settings, setSettings, updateSettings };
}
