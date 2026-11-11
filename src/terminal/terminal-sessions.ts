import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import type { TerminalSnapshot } from "../ipc";
import { invoke } from "../platform";
import { base64ToBytes, bytesToBase64 } from "./base64";
import { terminalTheme } from "./terminal-theme";

/** Canonical workspace target for one terminal. */
export interface TerminalTarget {
  agentName: string;
  workspace: string;
}

const sessions = new Map<string, TerminalSession>();
const everOpened = new Set<string>();
const listeners = new Set<() => void>();
let revision = 0;

function notify(): void {
  revision += 1;
  for (const listener of listeners) listener();
}

/** Lets tabs re-render when a title changes or a process exits. */
export function subscribeTerminalSessions(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function terminalSessionRevision(): number {
  return revision;
}

/** One shell per workspace is auto-created on first visit, but never again after the user closes all tabs. */
export function terminalOpenedBefore(key: string): boolean {
  return everOpened.has(key);
}

export function markTerminalOpened(key: string): void {
  everOpened.add(key);
}

function fontFamily(): string {
  if (typeof document === "undefined") return "ui-monospace, monospace";
  const family = getComputedStyle(document.documentElement)
    .getPropertyValue("--font-mono")
    .trim();
  return family || "ui-monospace, monospace";
}

let themeObserver: MutationObserver | null = null;
function ensureThemeObserver(): void {
  if (themeObserver || typeof document === "undefined") return;
  themeObserver = new MutationObserver(() => {
    for (const session of sessions.values()) session.refreshTheme();
  });
  themeObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["data-theme", "data-palette"],
  });
}

/**
 * One xterm instance and its PTY reader. Lives outside React so remounting the
 * chat inspector or navigating to another page never loses the screen buffer
 * or the shell. Only the active tab runs a read loop; hidden tabs replay from
 * their cursor when they are shown again.
 */
export class TerminalSession {
  readonly terminalId: string;
  readonly terminal: Terminal;
  readonly fit = new FitAddon();
  target: TerminalTarget;
  running: boolean;
  exitCode: number | null;
  error = "";
  title = "";
  private cursor = 0;
  private generation = 0;
  private reader: AbortController | null = null;
  private container: HTMLElement | null = null;
  private observer: ResizeObserver | null = null;
  private resizeTimer = 0;
  private writes: Promise<void> = Promise.resolve();
  private disposed = false;

  constructor(snapshot: TerminalSnapshot, target: TerminalTarget) {
    this.terminalId = snapshot.terminalId;
    this.target = target;
    this.running = snapshot.running;
    this.exitCode = snapshot.exitCode ?? null;
    this.terminal = new Terminal({
      cursorBlink: true,
      fontFamily: fontFamily(),
      fontSize: 12,
      lineHeight: 1.2,
      scrollback: 5000,
      allowTransparency: false,
      macOptionIsMeta: true,
      theme: terminalTheme(),
    });
    this.terminal.loadAddon(this.fit);
    this.terminal.onData((data) => this.send(data));
    this.terminal.onTitleChange((title) => {
      if (this.title === title) return;
      this.title = title;
      notify();
    });
    ensureThemeObserver();
  }

  get workspace(): string {
    return this.target.workspace;
  }

  private get inputContext(): { agentName: string; workspace: string } {
    return this.target;
  }

  attach(container: HTMLElement, target: TerminalTarget): void {
    if (this.disposed) return;
    this.target = target;
    this.container = container;
    if (!this.terminal.element) {
      this.terminal.open(container);
    } else if (this.terminal.element.parentElement !== container) {
      container.appendChild(this.terminal.element);
    }
    if (typeof ResizeObserver !== "undefined") {
      this.observer?.disconnect();
      this.observer = new ResizeObserver(() => this.scheduleResize());
      this.observer.observe(container);
    }
    this.error = "";
    this.scheduleResize();
    this.startReader();
  }

  /** Keep the xterm instance and PTY alive; only stop network polling. */
  detach(): void {
    // The selection effect can unmount after a tab close already disposed the
    // session; touching a disposed xterm is not safe.
    if (this.disposed) return;
    this.stopReader();
    this.observer?.disconnect();
    this.observer = null;
    this.container = null;
    window.clearTimeout(this.resizeTimer);
    this.terminal.blur();
    // Keeping the element attached would leave the previous tab's xterm in the
    // shared surface after a tab switch (two overlapping screens and two
    // helper textareas). The instance and its buffer survive; reattach appends
    // the same element to the next host.
    this.terminal.element?.remove();
  }

  refreshTheme(): void {
    if (this.disposed) return;
    this.terminal.options.theme = terminalTheme();
  }

  clear(): void {
    this.terminal.clear();
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.stopReader();
    this.observer?.disconnect();
    this.observer = null;
    window.clearTimeout(this.resizeTimer);
    const element = this.terminal.element;
    this.fit.dispose();
    this.terminal.dispose();
    element?.remove();
    notify();
  }

  private scheduleResize(): void {
    window.clearTimeout(this.resizeTimer);
    this.resizeTimer = window.setTimeout(() => {
      requestAnimationFrame(() => this.resize());
    }, 80);
  }

  private resize(): void {
    const container = this.container;
    if (!container || container.clientWidth < 2 || container.clientHeight < 2) return;
    try {
      this.fit.fit();
    } catch {
      return;
    }
    const { cols, rows } = this.terminal;
    if (cols < 1 || rows < 1) return;
    void invoke("resize_terminal", {
      ...this.inputContext,
      terminalId: this.terminalId,
      cols,
      rows,
    }).catch(() => {
      // The shell may be exiting; a failed resize is not user-visible.
    });
  }

  private startReader(): void {
    if (this.reader || this.disposed) return;
    const controller = new AbortController();
    this.reader = controller;
    const generation = this.generation;
    void this.readLoop(controller, generation);
  }

  private stopReader(): void {
    this.generation += 1;
    this.reader?.abort();
    this.reader = null;
  }

  private async readLoop(
    controller: AbortController,
    generation: number,
  ): Promise<void> {
    while (!controller.signal.aborted && generation === this.generation) {
      let read;
      try {
        read = await invoke("read_terminal", {
          ...this.inputContext,
          terminalId: this.terminalId,
          afterSeq: this.cursor,
          waitMs: 500,
        });
      } catch (error) {
        if (!controller.signal.aborted && generation === this.generation) {
          this.error = String(error);
          notify();
        }
        break;
      }
      if (controller.signal.aborted || generation !== this.generation) return;
      if (read.truncated) {
        this.terminal.write(
          "\r\n\x1b[33m[earlier output truncated]\x1b[0m\r\n",
        );
        // A fresh xterm cannot rebuild an alternate screen from a mid-stream
        // tail, so ask the shell for one redraw. A live reader that merely
        // fell behind only gets the marker; injecting Ctrl-L repeatedly into
        // a fast producer would be worse than the gap.
        if (this.cursor === 0) this.send("\x0c");
      }
      for (const chunk of read.chunks) {
        this.terminal.write(base64ToBytes(chunk.data));
      }
      this.cursor = read.nextSeq;
      if (!read.running) {
        this.running = false;
        this.exitCode = read.exitCode ?? null;
        this.reader = null;
        notify();
        return;
      }
    }
    if (this.reader === controller) this.reader = null;
  }

  private send(data: string): void {
    if (!data || this.disposed) return;
    const payload = bytesToBase64(new TextEncoder().encode(data));
    // Serialize writes so fast typing and paste cannot reorder at the PTY.
    this.writes = this.writes
      .then(() =>
        invoke("write_terminal", {
          ...this.inputContext,
          terminalId: this.terminalId,
          data: payload,
        }),
      )
      .then(() => undefined)
      .catch((error) => {
        this.error = String(error);
        notify();
      });
  }
}

export function getTerminalSession(
  terminalId: string,
): TerminalSession | undefined {
  return sessions.get(terminalId);
}

export function terminalSession(
  snapshot: TerminalSnapshot,
  target: TerminalTarget,
): TerminalSession {
  const existing = sessions.get(snapshot.terminalId);
  if (existing) {
    existing.target = target;
    return existing;
  }
  const session = new TerminalSession(snapshot, target);
  sessions.set(snapshot.terminalId, session);
  // The panel reads status through the registry; a new session must trigger a
  // render so the toolbar picks it up.
  notify();
  return session;
}

/** Local teardown only; used when the host list says the terminal is gone. */
export function disposeTerminalSession(terminalId: string): void {
  const session = sessions.get(terminalId);
  if (!session) return;
  sessions.delete(terminalId);
  session.dispose();
}

/** Stop the shell and drop the local xterm. */
export async function closeTerminalSession(
  terminalId: string,
  fallback: TerminalTarget,
): Promise<void> {
  const session = sessions.get(terminalId);
  const target = session?.target ?? fallback;
  disposeTerminalSession(terminalId);
  try {
    await invoke("close_terminal", { ...target, terminalId });
  } catch {
    // Already closed by another client or by host shutdown.
  }
}

export function reconcileTerminalSessions(
  workspace: string,
  ids: Set<string>,
): void {
  for (const [terminalId, session] of [...sessions]) {
    if (session.workspace === workspace && !ids.has(terminalId)) {
      disposeTerminalSession(terminalId);
    }
  }
}
