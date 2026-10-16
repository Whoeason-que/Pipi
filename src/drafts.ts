/** 窗口级输入缓存。Buffer 在会话换 id 时保持身份，迟到的发送确认只清理提交版本。 */
export interface DraftSnapshot { text: string; revision: number }
export interface DraftStorage { getItem(key: string): string | null; setItem(key: string, value: string): void }
const STORAGE_KEY = "pipi-composer-drafts-v1";

function windowStorage(): DraftStorage | undefined {
  try { return typeof window === "undefined" ? undefined : window.sessionStorage; }
  catch { return undefined; }
}

export class DraftBuffer {
  private snapshot: DraftSnapshot;
  private listeners = new Set<() => void>();
  private changed: () => void;
  constructor(text: string, changed: () => void) { this.snapshot = { text, revision: 0 }; this.changed = changed; }
  getSnapshot = (): DraftSnapshot => this.snapshot;
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  write(text: string): void {
    if (text === this.snapshot.text) return;
    this.snapshot = { text, revision: this.snapshot.revision + 1 };
    this.changed();
    this.listeners.forEach(listener => listener());
  }
  acknowledge(revision: number): void {
    if (this.snapshot.revision === revision) this.write("");
  }
}

interface RecordValue { agent: string; session: string | null; temporary: boolean; buffer: DraftBuffer }
const keyOf = (agent: string, session: string | null, temporary: boolean) => JSON.stringify([agent, session, temporary]);

export class DraftStore {
  private records = new Map<string, RecordValue>();
  private storage: DraftStorage | undefined;
  constructor(storage: DraftStorage | undefined = windowStorage()) {
    this.storage = storage;
    try {
      const saved: unknown = JSON.parse(storage?.getItem(STORAGE_KEY) ?? "[]");
      if (!Array.isArray(saved)) return;
      for (const row of saved) {
        if (!Array.isArray(row) || row.length !== 3 || typeof row[0] !== "string"
          || !(row[1] === null || typeof row[1] === "string") || typeof row[2] !== "string") continue;
        this.get(row[0], row[1]).write(row[2]);
      }
    } catch { /* 无缓存 / 损坏 / 禁用存储时继续使用内存 */ }
  }
  get(agent: string, session: string | null, temporary = false): DraftBuffer {
    const key = keyOf(agent, session, temporary);
    let record = this.records.get(key);
    if (!record) {
      record = { agent, session, temporary, buffer: new DraftBuffer("", () => this.persist()) };
      this.records.set(key, record);
    }
    return record.buffer;
  }
  move(agent: string, previous: string | null, next: string, temporary: boolean): void {
    if (previous === next) return;
    const oldKey = keyOf(agent, previous, temporary);
    const record = this.records.get(oldKey);
    if (!record) return;
    this.records.delete(oldKey);
    record.session = next;
    this.records.set(keyOf(agent, next, temporary), record);
    this.persist();
  }
  forget(agent: string, session: string | null): void {
    for (const [key, record] of this.records) {
      if (record.agent === agent && record.session === session) {
        this.records.delete(key);
        record.buffer.write("");
      }
    }
    this.persist();
  }
  forgetAgent(agent: string): void {
    for (const [key, record] of this.records) {
      if (record.agent === agent) { this.records.delete(key); record.buffer.write(""); }
    }
    this.persist();
  }
  private persist(): void {
    try {
      this.storage?.setItem(STORAGE_KEY, JSON.stringify([...this.records.values()]
        .filter(record => !record.temporary && record.buffer.getSnapshot().text)
        .map(record => [record.agent, record.session, record.buffer.getSnapshot().text])));
    } catch { /* 配额或访问失败不影响发送，也不丢失内存中的草稿 */ }
  }
}

export async function submitDraft(buffer: DraftBuffer, send: (text: string) => Promise<boolean>): Promise<boolean> {
  const { text, revision } = buffer.getSnapshot();
  if (!text.trim()) return false;
  const accepted = await send(text.trim());
  if (accepted) buffer.acknowledge(revision);
  return accepted;
}
