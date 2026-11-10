import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { assertHostIdle, prepare } from "../scripts/prepare-desktop-dev.mjs";

const root = resolve(import.meta.dirname, "..");

test("desktop preparation refuses a running Pipi host", () => {
  const dir = mkdtempSync(join(tmpdir(), "pipi-dev-host-"));
  try {
    const lock = join(dir, "host.lock");
    writeFileSync(lock, "");
    assert.doesNotThrow(() => assertHostIdle(root, { PIPI_HOME: dir }));
    const moduleUrl = new URL("../scripts/prepare-desktop-dev.mjs", import.meta.url).href;
    const code = `import {assertHostIdle} from ${JSON.stringify(moduleUrl)}; try {assertHostIdle(${JSON.stringify(root)}, {PIPI_HOME:${JSON.stringify(dir)}}); process.exit(2)} catch (e) {console.log(e.message)}`;
    const result = execFileSync("flock", ["--exclusive", lock, process.execPath, "--input-type=module", "-e", code], { encoding: "utf8" });
    assert.match(result, /already running/);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("desktop preparation rebuilds changed Rust contents even with restored mtime", () => {
  const dir = mkdtempSync(join(tmpdir(), "pipi-dev-cache-"));
  try {
    const crate = join(dir, "crates/pipi");
    mkdirSync(join(crate, "src"), { recursive: true });
    writeFileSync(join(dir, "Cargo.toml"), '[workspace]\nmembers=["crates/pipi"]\nresolver="3"\n');
    writeFileSync(join(crate, "Cargo.toml"), '[package]\nname="pipi"\nversion="0.1.0"\nedition="2024"\n');
    const source = join(crate, "src/main.rs");
    writeFileSync(source, 'fn main(){println!("old");}\n');
    execFileSync("cargo", ["generate-lockfile", "--offline"], { cwd: dir });
    const target = join(dir, "target/pipi-desktop");
    const env = { ...process.env, CARGO_TARGET_DIR: target };
    assert.equal(prepare(dir, target), true);
    execFileSync("cargo", ["build", "--offline", "--quiet"], { cwd: dir, env });
    const bin = join(target, "debug/pipi");
    assert.equal(execFileSync(bin, { encoding: "utf8" }).trim(), "old");
    assert.equal(prepare(dir, target), false);
    const original = statSync(source);
    writeFileSync(source, 'fn main(){println!("new");}\n');
    utimesSync(source, original.atime, original.mtime);
    assert.equal(prepare(dir, target), true);
    execFileSync("cargo", ["build", "--offline", "--quiet"], { cwd: dir, env });
    assert.equal(execFileSync(bin, { encoding: "utf8" }).trim(), "new");
    assert.throws(() => prepare(dir, join(dir, "target")), /Desktop target must/);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("desktop Vite disables watching and HMR while normal dev keeps watching", () => {
  const code = 'import {resolveConfig} from "vite"; const c=await resolveConfig({},"serve"); console.log(JSON.stringify({watch:c.server.watch,hmr:c.server.hmr}));';
  const base = { ...process.env };
  delete base.PIPI_DESKTOP_NO_WATCH;
  const resolveVite = (env: NodeJS.ProcessEnv) => JSON.parse(execFileSync(process.execPath, ["--input-type=module", "-e", code], { cwd: root, env, encoding: "utf8" }).trim());
  const normal = resolveVite(base);
  const desktop = resolveVite({ ...base, PIPI_DESKTOP_NO_WATCH: "1" });
  assert.notEqual(normal.watch, null);
  assert.equal(desktop.watch, null);
  assert.equal(desktop.hmr, false);
});

test("desktop shortcut runs npm directly and is valid", () => {
  const dir = mkdtempSync(join(tmpdir(), "pipi-dev-shortcut-"));
  try {
    const output = join(dir, "pipi.desktop");
    execFileSync(process.execPath, [join(root, "scripts/install-desktop.mjs"), output]);
    const shortcut = readFileSync(output, "utf8");
    assert.match(shortcut, /^Exec=\/bin\/sh -c .*npm run tauri:desktop/m);
    assert.doesNotMatch(shortcut, /python/i);
    assert.ok(shortcut.includes(`Path=${root}`));
    const validation = spawnSync("desktop-file-validate", [output], { encoding: "utf8" });
    if (!validation.error) assert.equal(validation.status, 0, validation.stderr);
    // Exercise GIO's real two-stage Exec parsing with a fake npm, never Pipi.
    const bin = join(dir, ".local/bin");
    mkdirSync(bin, { recursive: true });
    const fake = join(bin, "npm");
    writeFileSync(fake, '#!/bin/sh\nprintf "%s\\n" "$*" > "$HOME/shortcut-args"\n');
    chmodSync(fake, 0o755);
    const launched = spawnSync("gio", ["launch", output], {
      encoding: "utf8", env: { ...process.env, HOME: dir, XDG_CACHE_HOME: join(dir, "cache") },
    });
    if (!launched.error) {
      assert.equal(launched.status, 0, launched.stderr);
      const result = join(dir, "shortcut-args");
      for (let n = 0; n < 50 && !existsSync(result); n++) Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 20);
      assert.equal(readFileSync(result, "utf8").trim(), "run tauri:desktop");
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
