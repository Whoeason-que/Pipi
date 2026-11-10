// Install a desktop shortcut whose actual launcher is npm/Tauri, not this script.
import { chmodSync, mkdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const destination = resolve(process.argv[2] ?? join(process.env.XDG_DATA_HOME || join(homedir(), ".local/share"), "applications/pipi.desktop"));
const escapeValue = value => value.replaceAll("\\", "\\\\").replaceAll("\n", "\\n");
// Desktop files are unescaped twice: first as key-file text, then as Exec args.
const quoteExec = value => `"${value.replaceAll("%", "%%").replace(/[\\"`$]/g, "\\\\$&")}"`;
const shell = 'PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"; export PATH; mkdir -p "${XDG_CACHE_HOME:-$HOME/.cache}/pipi"; exec npm run tauri:desktop >>"${XDG_CACHE_HOME:-$HOME/.cache}/pipi/desktop-dev.log" 2>&1';
const entry = `[Desktop Entry]\nType=Application\nVersion=1.0\nName=Pipi\nComment=Run Pipi from source\nExec=/bin/sh -c ${quoteExec(shell)}\nPath=${escapeValue(root)}\nIcon=${escapeValue(join(root, "crates/pipi/icons/128x128.png"))}\nTerminal=false\nStartupNotify=false\nStartupWMClass=pipi\nCategories=Development;\n`;
mkdirSync(dirname(destination), { recursive: true });
const temporary = `${destination}.${process.pid}.tmp`;
try {
  writeFileSync(temporary, entry, { mode: 0o755 });
  chmodSync(temporary, 0o755);
  renameSync(temporary, destination);
} finally {
  // A failed rename leaves the pre-existing launcher untouched.
  rmSync(temporary, { force: true });
}
console.log(`Installed: ${destination}`);
