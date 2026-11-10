"""Desktop builds must not reuse another checkout's local Rust artifacts."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("desktop_launcher", Path(__file__).resolve().parents[1] / "scripts/desktop-launcher.py")
launcher = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(launcher)


class DesktopBuildTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="pipi-desktop-build-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.root = self.make_root("checkout")
        self.calls = []
        self.fail_build = False
        self.mutate_during_build = False

    def make_root(self, name):
        root = self.base / name
        (root / "crates/pipi-session/src").mkdir(parents=True)
        (root / "node_modules/.bin").mkdir(parents=True)
        (root / "node_modules/.bin/tauri").touch()
        (root / "Cargo.toml").write_text("workspace")
        (root / "Cargo.lock").write_text("lock")
        (root / "crates/pipi-session/Cargo.toml").write_text("package")
        (root / "crates/pipi-session/src/lib.rs").write_text("old source")
        return root

    def run_command(self, args, *, env, **kwargs):
        self.calls.append((list(args), env["CARGO_TARGET_DIR"]))
        if args[:2] == ["cargo", "metadata"]:
            return SimpleNamespace(returncode=0, stdout=json.dumps({
                "target_directory": env["CARGO_TARGET_DIR"],
                "workspace_members": ["member"],
                "packages": [{"id": "member", "name": "pipi-session"}],
            }))
        if args[0] == "npm":
            if self.fail_build:
                return SimpleNamespace(returncode=1)
            if self.mutate_during_build:
                (self.root / "crates/pipi-session/src/lib.rs").write_text("changed during build")
            binary = Path(env["CARGO_TARGET_DIR"]) / "debug/pipi"
            binary.parent.mkdir(parents=True, exist_ok=True)
            binary.write_bytes(b"test binary")
        return SimpleNamespace(returncode=0)

    def build(self, root=None):
        with patch.object(launcher, "ROOT", root or self.root), \
             patch.object(launcher, "host_running", return_value=False), \
             patch.object(launcher, "progress_start", return_value=None), \
             patch.object(launcher.shutil, "which", return_value="/bin/test"), \
             patch.object(launcher.subprocess, "run", side_effect=self.run_command), \
             patch.dict(os.environ, {"XDG_CACHE_HOME": str(self.base / "cache"), "CARGO_TARGET_DIR": str(self.base / "foreign")}, clear=False):
            return launcher.launch(True)

    def cleans(self):
        return [call for call in self.calls if call[0][:2] == ["cargo", "clean"]]

    def test_source_change_with_old_mtime_cleans_local_crates(self):
        self.assertEqual(self.build(), 0)
        self.assertEqual(len(self.cleans()), 1)
        self.assertEqual(self.build(), 0)
        self.assertEqual(len(self.cleans()), 1)
        source = self.root / "crates/pipi-session/src/lib.rs"
        original = source.stat()
        source.write_text("changed source")
        os.utime(source, ns=(original.st_atime_ns, original.st_mtime_ns))
        self.assertEqual(self.build(), 0)
        self.assertEqual(len(self.cleans()), 2)
        self.assertIn("pipi-session", self.cleans()[-1][0])

    def test_distinct_checkouts_ignore_inherited_target(self):
        self.build()
        self.build(self.make_root("another-checkout"))
        targets = [target for args, target in self.calls if args[0] == "npm"]
        self.assertNotEqual(targets[0], targets[1])
        self.assertTrue(all(target != str(self.base / "foreign") for target in targets))

    def test_failed_build_invalidates_stamp(self):
        self.build()
        self.fail_build = True
        with self.assertRaisesRegex(RuntimeError, "编译失败"):
            self.build()
        self.fail_build = False
        self.build()
        self.assertEqual(len(self.cleans()), 2)

    def test_changed_source_during_build_never_marks_it_trusted(self):
        self.mutate_during_build = True
        with self.assertRaisesRegex(RuntimeError, "编译期间源码发生变化"):
            self.build()
        self.mutate_during_build = False
        self.build()
        self.assertEqual(len(self.cleans()), 2)

    @unittest.skipUnless(shutil.which("cargo"), "Cargo is required")
    def test_actual_cargo_rebuilds_when_source_mtime_is_restored(self):
        root = self.root
        crate = root / "crates/pipi"
        (crate / "src").mkdir(parents=True)
        (root / "Cargo.toml").write_text('[workspace]\nmembers=["crates/pipi"]\nresolver="3"\n')
        (crate / "Cargo.toml").write_text('[package]\nname="pipi"\nversion="0.1.0"\nedition="2024"\n')
        source = crate / "src/main.rs"
        source.write_text('fn main() { println!("old"); }\n')
        real_run = subprocess.run
        real_run(["cargo", "generate-lockfile", "--offline"], cwd=root, check=True, capture_output=True)
        original = source.stat()
        built_target = []

        def actual_run(args, **kwargs):
            if args[0] == "npm":
                built_target.append(Path(kwargs["env"]["CARGO_TARGET_DIR"]))
                return real_run(["cargo", "build", "--offline", "--locked"], **kwargs)
            return real_run(args, **kwargs)

        with patch.object(launcher, "ROOT", root), \
             patch.object(launcher, "host_running", return_value=False), \
             patch.object(launcher, "progress_start", return_value=None), \
             patch.object(launcher.subprocess, "run", side_effect=actual_run), \
             patch.dict(os.environ, {"XDG_CACHE_HOME": str(self.base / "real-cache")}, clear=False):
            self.assertEqual(launcher.launch(True), 0)
            binary = built_target[-1] / "debug/pipi"
            self.assertEqual(subprocess.check_output([binary], text=True).strip(), "old")
            source.write_text('fn main() { println!("new"); }\n')
            os.utime(source, ns=(original.st_atime_ns, original.st_mtime_ns))
            self.assertEqual(launcher.launch(True), 0)
            self.assertEqual(subprocess.check_output([binary], text=True).strip(), "new")


if __name__ == "__main__":
    unittest.main()
