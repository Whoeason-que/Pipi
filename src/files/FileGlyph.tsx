import { fileGlyph } from "./model";

/** 文件类型胶囊：扩展名 + 语义色，File 树、标签和标题共用。 */
export function FileGlyph({ path, size = "md" }: { path: string; size?: "sm" | "md" }) {
  const glyph = fileGlyph(path);
  return <span className={`file-glyph ${glyph.tone} ${size}`} aria-hidden="true">{glyph.label}</span>;
}
