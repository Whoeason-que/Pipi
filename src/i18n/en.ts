import { agents } from "./locales/en/agents.ts";
import { app } from "./locales/en/app.ts";
import { chat } from "./locales/en/chat.ts";
import { common } from "./locales/en/common.ts";
import { components } from "./locales/en/components.ts";
import { errors } from "./locales/en/errors.ts";
import { files } from "./locales/en/files.ts";
import { forum } from "./locales/en/forum.ts";
import { inspector } from "./locales/en/inspector.ts";
import { login } from "./locales/en/login.ts";
import { nav } from "./locales/en/nav.ts";
import { notifications } from "./locales/en/notifications.ts";
import { remoteAccess } from "./locales/en/remote-access.ts";
import { settings } from "./locales/en/settings.ts";
import { skills } from "./locales/en/skills.ts";
import { stats } from "./locales/en/stats.ts";
import { themes } from "./locales/en/themes.ts";
import { workspace } from "./locales/en/workspace.ts";

/** English is the authoritative dictionary: default locale and fallback for every other locale. */
export const en = {
  common,
  nav,
  workspace,
  chat,
  stats,
  inspector,
  settings,
  notifications,
  remoteAccess,
  errors,
  agents,
  files,
  skills,
  forum,
  app,
  components,
  login,
  themes,
};

export type I18nSchema = typeof en;

/** Other locales may translate any subset; missing keys fall back to English. */
export type DeepPartial<T> = {
  [K in keyof T]?: T[K] extends string ? string : DeepPartial<T[K]>;
};
