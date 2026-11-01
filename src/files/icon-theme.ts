import { invoke } from "../platform.ts";
import { languageFor } from "./model.ts";

import type { Associations, IconTheme } from "./icon-theme-types.ts";
export type { IconThemeList } from "./icon-theme-types.ts";
type RenderIcon = { image: string } | { character: string; family: string; color?: string; size?: string };

let current: IconTheme | null = null;
let lightTheme: IconTheme | null = null;
let contrastTheme: IconTheme | null = null;
let contrastMedia: MediaQueryList | null = null;
let families = new Map<string, string>();
let loadedFonts: FontFace[] = [];
let loadPromise: Promise<void> | null = null;
let generation = 0;
let requestVersion = 0;
let observer: MutationObserver | null = null;
let revision = 0;
const listeners = new Set<() => void>();
export const subscribeIconTheme = (callback: () => void) => {
  listeners.add(callback);
  if (!observer && typeof MutationObserver !== "undefined" && typeof document !== "undefined") {
    observer = new MutationObserver(publish);
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  }
  if (!contrastMedia && typeof window !== "undefined" && typeof window.matchMedia === "function") {
    contrastMedia = window.matchMedia("(forced-colors: active)");
    contrastMedia.addEventListener("change", publish);
  }
  return () => { listeners.delete(callback); if (!listeners.size) { observer?.disconnect(); observer = null; contrastMedia?.removeEventListener("change", publish); contrastMedia = null; } };
};
export const iconThemeRevision = () => revision;
function publish() { revision++; listeners.forEach(listener => listener()); }

export function applyIconTheme(theme: IconTheme | null) {
  generation++;
  if (typeof document !== "undefined") loadedFonts.forEach(face => document.fonts.delete(face));
  loadedFonts = [];
  families = new Map();
  const normalize = (map: Associations | undefined) => {
    if (!map) return undefined;
    const result = { ...map };
    for (const key of ["fileNames", "fileExtensions", "folderNames", "folderNamesExpanded", "rootFolderNames", "rootFolderNamesExpanded", "languageIds"] as const) {
      const values = result[key];
      if (values) result[key] = Object.fromEntries(Object.entries(values).map(([name, id]) => [name.toLowerCase(), id]));
    }
    return result;
  };
  current = theme ? { ...theme, ...normalize(theme), light: normalize(theme.light), highContrast: normalize(theme.highContrast) } : null;
  const merge = (base: IconTheme | null, variant: Associations | undefined): IconTheme | null => base && variant ? { ...base, ...variant,
    fileNames: { ...base.fileNames, ...variant.fileNames }, fileExtensions: { ...base.fileExtensions, ...variant.fileExtensions },
    folderNames: { ...base.folderNames, ...variant.folderNames }, folderNamesExpanded: { ...base.folderNamesExpanded, ...variant.folderNamesExpanded },
    rootFolderNames: { ...base.rootFolderNames, ...variant.rootFolderNames }, rootFolderNamesExpanded: { ...base.rootFolderNamesExpanded, ...variant.rootFolderNamesExpanded },
    languageIds: { ...base.languageIds, ...variant.languageIds } } : base;
  lightTheme = merge(current, current?.light);
  contrastTheme = merge(current, current?.highContrast);
  if (typeof document !== "undefined" && document.fonts && typeof FontFace !== "undefined") {
    for (const [index, font] of (theme?.fonts ?? []).entries()) {
      const source = font.src?.find(item => item.path?.startsWith("data:font/"));
      if (!source?.path) continue;
      const family = `pipi-file-icons-${generation}-${index}`;
      try {
        const face = new FontFace(family, `url(${JSON.stringify(source.path)})`);
        families.set(font.id, family);
        document.fonts.add(face);
        loadedFonts.push(face);
        void face.load().catch(() => {
          if (families.get(font.id) === family) { families.delete(font.id); publish(); }
        });
      } catch { /* Unsupported font: use the built-in icon. */ }
    }
  }
  publish();
}
export function initializeIconTheme(): Promise<void> {
  if (!loadPromise) {
    const version = requestVersion;
    loadPromise = invoke("get_icon_theme").then(theme => { if (version === requestVersion) applyIconTheme(theme); }).catch(() => { if (version === requestVersion) applyIconTheme(null); });
  }
  return loadPromise;
}
export async function selectIconTheme(id: string | null) {
  const version = ++requestVersion;
  const theme = await invoke("set_icon_theme", { id });
  if (version === requestVersion) applyIconTheme(theme);
}
function mapped(map: Record<string, string> | undefined, parent: string, key: string): string | undefined {
  if (!map) return undefined;
  const scoped = `${parent}/${key}`;
  return (Object.hasOwn(map, scoped) ? map[scoped] : undefined) ?? (Object.hasOwn(map, key) ? map[key] : undefined);
}
function iconId(map: Associations, path: string, folder: boolean, expanded: boolean, root: boolean): string | undefined {
  const pieces = path.replace(/\\/g, "/").split("/").filter(Boolean);
  const name = (pieces.pop() ?? "").toLowerCase();
  const parent = (pieces.pop() ?? "").toLowerCase();
  if (folder) {
    if (root) return (expanded && map.rootFolderNamesExpanded?.[name]) || map.rootFolderNames?.[name]
      || (expanded && map.rootFolderExpanded) || map.rootFolder
      || (expanded && mapped(map.folderNamesExpanded, parent, name)) || mapped(map.folderNames, parent, name)
      || (expanded ? map.folderExpanded || map.folder : map.folder);
    return (expanded && mapped(map.folderNamesExpanded, parent, name)) || mapped(map.folderNames, parent, name) || (expanded ? map.folderExpanded || map.folder : map.folder);
  }
  const direct = mapped(map.fileNames, parent, name);
  if (direct) return direct;
  const parts = name.split(".");
  for (let i = 1; i < parts.length; i++) {
    const match = mapped(map.fileExtensions, parent, parts.slice(i).join("."));
    if (match) return match;
  }
  const language = languageFor(path);
  const languageId = language === "bash" ? "shellscript" : language === "xml" && name.endsWith(".html") ? "html" : language;
  return map.languageIds?.[languageId] || map.file;
}
export function resolveIcon(path: string, folder = false, expanded = false, mode: "dark" | "light" | "highContrast" = "dark", root = false): RenderIcon | null {
  if (!current) return null;
  const override = mode === "highContrast" ? contrastTheme : mode === "light" ? lightTheme : current;
  if (!override) return null;
  const id = iconId(override, path, folder, expanded, root);
  const def = id ? current.iconDefinitions?.[id] : undefined;
  if (def?.iconPath?.startsWith("data:image/")) return { image: def.iconPath };
  if (!def?.fontCharacter) return null;
  const font = current.fonts?.find(item => item.id === def.fontId) ?? current.fonts?.[0];
  const family = font ? families.get(font.id) : undefined;
  if (!family) return null;
  const code = /^\\([0-9a-f]{1,6})\s?$/i.exec(def.fontCharacter);
  const number = code ? parseInt(code[1], 16) : 0;
  const character = code && number <= 0x10ffff ? String.fromCodePoint(number) : def.fontCharacter;
  return { character, family, color: def.fontColor, size: def.fontSize ?? font?.size };
}
