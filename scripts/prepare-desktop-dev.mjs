// Keep the desktop dev target private to this checkout. Cargo's mtime-based
// freshness checks can otherwise reuse crates built from different contents.
import { createHash } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, realpathSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

export function assertHostIdle(projectRoot, env = process.env) {
  const dataRoot = env.PIPI_HOME || join(env.HOME || "", ".pipi");
  const lock = resolve(projectRoot, dataRoot, "host.lock");
  if (!existsSync(lock)) return;
  const result = spawnSync("flock", ["--nonblock", "--shared", lock, "/bin/true"], { stdio: "ignore" });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error("Pipi is already running; quit it before launching another desktop instance.");
}

export function sourceDigest(projectRoot) {
  const hash = createHash("sha256");
  const files = ["Cargo.toml", "Cargo.lock", ".cargo/config.toml"];
  function visit(dir) {
    for (const entry of readdirSync(join(projectRoot, dir), { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const name = join(dir, entry.name);
      if (entry.isDirectory()) {
        if (!["target", "binaries", "gen"].includes(entry.name)) visit(name);
      } else if (entry.isFile() || (entry.isSymbolicLink() && statSync(join(projectRoot, name)).isFile())) files.push(name);
    }
  }
  visit("crates");
  hash.update(realpathSync(projectRoot));
  for (const file of files.sort()) {
    const path = join(projectRoot, file);
    if (!existsSync(path)) continue;
    const name = Buffer.from(file);
    const data = readFileSync(path);
    const size = Buffer.alloc(8);
    size.writeBigUInt64BE(BigInt(name.length)); hash.update(size); hash.update(name);
    size.writeBigUInt64BE(BigInt(data.length)); hash.update(size); hash.update(data);
  }
  return hash.digest("hex");
}

export function prepare(projectRoot, target) {
  const expected = resolve(projectRoot, "target/pipi-desktop");
  if (resolve(target) !== expected) throw new Error("Desktop target must be target/pipi-desktop");
  mkdirSync(expected, { recursive: true });
  const stamp = join(expected, ".pipi-source-sha256");
  const current = sourceDigest(projectRoot);
  const previous = existsSync(stamp) ? readFileSync(stamp, "utf8").trim() : "";
  if (current === previous) return false;
  rmSync(stamp, { force: true });
  const env = { ...process.env, CARGO_TARGET_DIR: expected };
  const meta = JSON.parse(execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps", "--offline", "--locked"], { cwd: projectRoot, env, encoding: "utf8" }));
  const members = new Set(meta.workspace_members);
  const packages = meta.packages.filter(pkg => members.has(pkg.id)).map(pkg => pkg.name).sort();
  if (!packages.length) throw new Error("Cannot determine desktop workspace packages");
  console.log("Pipi desktop: source changed; rebuilding workspace crates.");
  execFileSync("cargo", ["clean", "--locked", ...packages.flatMap(name => ["-p", name])], { cwd: projectRoot, env, stdio: "inherit" });
  if (sourceDigest(projectRoot) !== current) throw new Error("Source changed during desktop preparation; retry.");
  const tmp = `${stamp}.${process.pid}.tmp`;
  try { writeFileSync(tmp, `${current}\n`, { mode: 0o600 }); renameSync(tmp, stamp); }
  finally { rmSync(tmp, { force: true }); }
  return true;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    assertHostIdle(root);
    prepare(root, process.env.CARGO_TARGET_DIR ?? join(root, "target/pipi-desktop"));
  }
  catch (error) { console.error(`Pipi desktop preparation failed: ${error.message}`); process.exitCode = 1; }
}
