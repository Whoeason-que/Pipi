#!/usr/bin/env python3
"""Linux 桌面入口：点击时打包当前源码，成功后直接启动，不注册后台服务。"""

import argparse
import datetime
import fcntl
import hashlib
import html
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import time


ROOT = Path(__file__).resolve().parent.parent
ICON = ROOT / "crates/pipi/icons/128x128.png"
TITLE = "Pipi 源码版"


def environment():
    env = os.environ.copy()
    env["PATH"] = os.pathsep.join([
        str(Path.home() / ".cargo/bin"), str(Path.home() / ".local/bin"),
        "/usr/local/bin", "/usr/bin", "/bin", env.get("PATH", ""),
    ])
    env["CARGO_TERM_COLOR"] = "never"
    return env


def dialog(text, error=False):
    print(text, file=sys.stderr, flush=True)
    if (os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")) and shutil.which("zenity"):
        subprocess.Popen([
            "zenity", "--error" if error else "--info", "--title", TITLE,
            "--text", html.escape(text), "--width", "480",
        ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)


def host_running():
    home = Path(os.environ.get("PIPI_HOME", str(Path.home() / ".pipi"))).expanduser()
    if not home.is_absolute():
        home = ROOT / home
    try:
        with (home / "host.lock").open("r") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return True
    except FileNotFoundError:
        pass
    return False


def progress_start(log_path):
    if not (os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")) or not shutil.which("zenity"):
        return None
    progress = subprocess.Popen([
        "zenity", "--progress", "--pulsate", "--no-cancel", "--auto-close",
        "--title", TITLE, "--width", "480", "--text",
        html.escape(f"正在编译前端和 Rust，首次可能需要较长时间。\n\n日志：{log_path}"),
    ], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, text=True)
    try:
        progress.stdin.write("0\n")
        progress.stdin.flush()
    except OSError:
        pass
    return progress


def progress_stop(progress):
    if progress is None:
        return
    try:
        progress.stdin.write("100\n")
        progress.stdin.close()
    except OSError:
        pass
    try:
        progress.wait(timeout=2)
    except subprocess.TimeoutExpired:
        progress.terminate()
        progress.wait(timeout=2)


def launch(build_only):
    env = environment()
    for name in ["cargo", "npm", "node"]:
        if not shutil.which(name, path=env["PATH"]):
            raise RuntimeError(f"未找到 {name}，请先安装 Rust 和 Node.js。")
    if not (ROOT / "node_modules/.bin/tauri").exists():
        raise RuntimeError(f"请先在源码目录安装前端依赖：\ncd {ROOT}\nnpm install")

    cache = Path(os.environ.get("XDG_CACHE_HOME", str(Path.home() / ".cache"))) / "pipi/source-launcher"
    cache.mkdir(parents=True, exist_ok=True)
    identity = hashlib.sha256(str(ROOT).encode()).hexdigest()[:16]
    with (cache / f"{identity}.lock").open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            dialog("已有一次源码编译或启动正在进行，请稍候。")
            return 0
        if host_running():
            dialog("Pipi 已经运行。请先从托盘退出 Pipi，再点击此图标编译并启动最新源码。")
            return 0

        stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S-%f")
        log_path = cache / f"launch-{stamp}.log"
        command = ["npm", "run", "tauri:build", "--", "--debug", "--no-bundle", "--", "--locked"]
        print(f"编译日志：{log_path}", flush=True)
        with log_path.open("w") as log:
            log.write(f"源码：{ROOT}\n命令：{shlex.join(command)}\n\n")
            log.flush()
            progress = progress_start(log_path)
            try:
                result = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
            finally:
                progress_stop(progress)
            if result.returncode:
                raise RuntimeError(f"编译失败，本次没有启动应用。\n\n请查看日志：\n{log_path}")

            metadata = subprocess.run([
                "cargo", "metadata", "--format-version", "1", "--no-deps", "--offline",
            ], cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=log, text=True, check=True)
            binary = Path(json.loads(metadata.stdout)["target_directory"]) / "debug/pipi"
            if not binary.is_file():
                raise RuntimeError(f"编译后未找到本机应用：{binary}\n日志：{log_path}")
            if build_only:
                print(f"编译完成：{binary}", flush=True)
                return 0
            if host_running():
                dialog("编译已完成，但另一个 Pipi 已经启动。请先退出它，再点击此图标。")
                return 0
            log.write(f"\n启动：{binary}\n")
            log.flush()
            app = subprocess.Popen(
                [str(binary)], cwd=ROOT, env=env, stdin=subprocess.DEVNULL,
                stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
            )
            # 等待宿主拿到所有权后再释放点击锁，避免连续点击产生重复实例。
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                code = app.poll()
                if code is not None:
                    if code:
                        raise RuntimeError(f"Pipi 启动失败（退出码 {code}）。\n日志：{log_path}")
                    return 0
                if host_running():
                    print(f"Pipi 已启动，PID：{app.pid}", flush=True)
                    return 0
                time.sleep(0.1)
            raise RuntimeError(f"应用进程已启动，但尚未确认宿主就绪。请勿重复启动。\n日志：{log_path}")


def install():
    script = str(Path(__file__).resolve())
    escaped = script.replace("\\", "\\\\").replace('"', '\\"').replace("$", "\\$").replace("`", "\\`").replace("%", "%%")
    entry = (
        f"[Desktop Entry]\nType=Application\nVersion=1.0\nName={TITLE}\n"
        "Comment=编译当前本地源码并启动 Pipi\n"
        f'Exec=/usr/bin/python3 "{escaped}"\nPath={ROOT}\nIcon={ICON}\n'
        "Terminal=false\nStartupNotify=false\nStartupWMClass=pipi\nCategories=Development;\n"
    )
    applications = Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share"))) / "applications"
    desktop = Path.home() / "Desktop"
    if shutil.which("xdg-user-dir"):
        result = subprocess.run(["xdg-user-dir", "DESKTOP"], capture_output=True, text=True)
        if result.returncode == 0 and result.stdout.strip():
            desktop = Path(result.stdout.strip())
    for directory, name in [(applications, "pipi-source.desktop"), (desktop, "Pipi 源码版.desktop")]:
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / name
        temporary = directory / f".{name}.{os.getpid()}.tmp"
        temporary.write_text(entry)
        temporary.chmod(0o755)
        temporary.replace(path)
        print(f"已安装：{path}")
        if directory == desktop and shutil.which("gio"):
            trusted = subprocess.run(["gio", "set", str(path), "metadata::trusted", "true"], capture_output=True, text=True)
            if trusted.returncode:
                print("桌面首次使用时，请右键图标选择“允许启动”。")
    if shutil.which("update-desktop-database"):
        subprocess.run(["update-desktop-database", str(applications)], check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--install", action="store_true", help="安装应用菜单和桌面图标")
    parser.add_argument("--build-only", action="store_true", help="只打包当前源码，不启动应用")
    args = parser.parse_args()
    os.umask(0o077)
    try:
        if args.install:
            install()
        else:
            sys.exit(launch(args.build_only))
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        dialog(str(error), error=True)
        sys.exit(1)
