import type { UserImage } from "../types";

const IMAGE_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const escape = (text: string) => text.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");

/** Convert selected local files before submitting. Binary formats are rejected explicitly. */
export async function prepareAttachments(text: string, files: File[]): Promise<{ text: string; images: UserImage[] }> {
  const images: UserImage[] = [];
  let imageBytes = 0;
  const blocks: string[] = [];
  for (const file of files) {
    const suffix = file.name.toLowerCase().split(".").pop() ?? "";
    const inferred = ({ png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp" } as Record<string, string>)[suffix];
    const imageType = IMAGE_TYPES.has(file.type) ? file.type : (!file.type || file.type === "application/octet-stream") ? inferred : undefined;
    if (imageType) {
      imageBytes += file.size;
      if (images.length >= 4 || imageBytes > 1024 * 1024) throw new Error("Upload up to 4 images with a total size of 1 MiB or less");
      const bytes = new Uint8Array(await file.arrayBuffer());
      let binary = "";
      for (let offset = 0; offset < bytes.length; offset += 8192) binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
      images.push({ mimeType: imageType, data: btoa(binary) });
      blocks.push(`[图片：${file.name.replace(/[\r\n]/g, " ")}]`);
    } else {
      if (file.size > 48 * 1024) throw new Error(`Text attachment too large: ${file.name} (limit 48 KiB)`);
      let content: string;
      try { content = new TextDecoder("utf-8", { fatal: true }).decode(await file.arrayBuffer()); }
      catch { throw new Error(`Unsupported file: ${file.name}. Choose a UTF-8 text file or a PNG, JPEG, GIF, or WebP image`); }
      if (/\x00/.test(content) || content.startsWith("%PDF-") || content.startsWith("PK\x03\x04")) throw new Error(`Unsupported binary file: ${file.name}`);
      blocks.push(`<file name="${escape(file.name)}">\n${escape(content)}\n</file>`);
    }
  }
  const result = [text.trim(), ...blocks].filter(Boolean).join("\n\n");
  if (new TextEncoder().encode(result).length > 64 * 1024) throw new Error("Message and text attachments cannot exceed 64 KiB in total");
  return { text: result, images };
}
