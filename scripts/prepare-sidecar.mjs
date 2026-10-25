// Tauri 的 externalBin 需要 `binaries/<name>-<target-triple>` 在配置目录里。
// 这个脚本把 cargo 刚构建好的 helper 复制成 Tauri 期望的名字。
//
// 用法：node scripts/prepare-sidecar.mjs [debug|release]

import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { join } from "node:path";

const profile = process.argv[2] === "release" ? "release" : "debug";
const isWindows = process.platform === "win32";
const exe = isWindows ? "pipi-sandbox.exe" : "pipi-sandbox";
const targetDir = process.env.CARGO_TARGET_DIR ?? "target";
const source = join(targetDir, profile, exe);

if (!existsSync(source)) {
  console.error(`sidecar: 找不到 ${source}，请先 cargo build -p pipi-sandbox`);
  process.exit(1);
}

const version = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
const host = /^host: (.+)$/m.exec(version)?.[1];
if (!host) {
  console.error("sidecar: 无法从 rustc -vV 读取 host target");
  process.exit(1);
}

const destinationDir = join("crates", "pipi", "binaries");
mkdirSync(destinationDir, { recursive: true });
const destination = join(
  destinationDir,
  isWindows ? `pipi-sandbox-${host}.exe` : `pipi-sandbox-${host}`,
);
copyFileSync(source, destination);
console.log(`sidecar: ${source} -> ${destination}`);
