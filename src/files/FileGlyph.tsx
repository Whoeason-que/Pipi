import { useEffect, useSyncExternalStore } from "react";
import { IconFolder } from "../icons";
import { initializeIconTheme, iconThemeRevision, resolveIcon, subscribeIconTheme } from "./icon-theme";
import { fileGlyph } from "./model";

/** 文件树、标签和预览标题共用的文件类型图标。 */
export function FileGlyph({ path, size = "md" }: { path: string; size?: "sm" | "md" }) {
  const icon = useIcon(path);
  const glyph = fileGlyph(path);
  if (icon) return <span className={`file-glyph themed ${size}`} aria-hidden="true"><IconContent icon={icon} /></span>;
  return <span className={`file-glyph ${glyph.tone} ${size} file-glyph-${glyph.label.toLowerCase().replace(/[^a-z0-9]/g, "")}`} aria-hidden="true">
    <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
      <path d="M5.5 2.75h8.8l4.2 4.2v12.3a2 2 0 0 1-2 2h-11a2 2 0 0 1-2-2V4.75a2 2 0 0 1 2-2Z" fill="currentColor" fillOpacity=".12" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round" />
      <path d="M14.25 2.85v3.8c0 .45.35.8.8.8h3.3" stroke="currentColor" strokeWidth="1.1" strokeLinejoin="round" />
      <text x="11.1" y="17" textAnchor="middle" fill="currentColor" fontFamily="system-ui, sans-serif" fontSize={glyph.label.length > 2 ? "6.7" : "8"} fontWeight="750">{glyph.label}</text>
    </svg>
  </span>;
}

function useIcon(path: string, folder = false, expanded = false, root = false) {
  useSyncExternalStore(subscribeIconTheme, iconThemeRevision);
  useEffect(() => { void initializeIconTheme(); }, []);
  const highContrast = typeof window.matchMedia === "function" && window.matchMedia("(forced-colors: active)").matches;
  return resolveIcon(path, folder, expanded, highContrast ? "highContrast" : document.documentElement.dataset.theme === "light" ? "light" : "dark", root);
}
function IconContent({ icon }: { icon: NonNullable<ReturnType<typeof resolveIcon>> }) {
  return "image" in icon ? <img src={icon.image} alt="" draggable={false} />
    : <span style={{ fontFamily: icon.family, color: icon.color, fontSize: icon.size ?? "100%" }}>{icon.character}</span>;
}
export function FolderGlyph({ path, expanded, root = false }: { path: string; expanded: boolean; root?: boolean }) {
  const icon = useIcon(path, true, expanded, root);
  return icon ? <span className="file-glyph themed sm" aria-hidden="true"><IconContent icon={icon} /></span> : <IconFolder />;
}
