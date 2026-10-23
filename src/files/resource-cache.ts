/** 只在当前进程保留读取结果；目录、Git 状态与预览使用不同的请求身份。 */
export class FileResourceCache {
  private entries = new Map<string, { value: unknown; size: number }>();
  private size = 0;
  private maxEntries: number;
  private maxBytes: number;

  constructor(maxEntries = 64, maxBytes = 32 * 1024 * 1024) {
    this.maxEntries = maxEntries;
    this.maxBytes = maxBytes;
  }

  get<T>(identity: string): T | null {
    return this.entries.get(identity)?.value as T ?? null;
  }

  set(identity: string, value: unknown) {
    const size = JSON.stringify(value).length * 2;
    const previous = this.entries.get(identity);
    if (previous) { this.size -= previous.size; this.entries.delete(identity); }
    if (size > this.maxBytes) return;
    this.entries.set(identity, { value, size });
    this.size += size;
    while (this.entries.size > this.maxEntries || this.size > this.maxBytes) {
      const oldest = this.entries.keys().next().value!;
      this.size -= this.entries.get(oldest)!.size;
      this.entries.delete(oldest);
    }
  }
}
