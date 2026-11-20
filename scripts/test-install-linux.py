#!/usr/bin/env python3
"""Exercise Linux installation with verified local assets and no real user data."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

INSTALLER = Path(__file__).resolve().with_name("install.sh")


class LinuxInstaller(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="rustcode-install-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "home with spaces"
        self.home.mkdir()
        self.assets = self.root / "assets"
        self.assets.mkdir()
        self.commands = self.root / "commands"
        self.commands.mkdir()
        self.command("uname", 'case "$1" in -s) echo Linux;; -m) echo x86_64;; esac')
        self.command("ldconfig", 'echo "libwebkit2gtk-4.1.so.0 (libc6,x86-64) => /usr/lib/libwebkit2gtk-4.1.so.0"')
        self.command("ldd", 'echo "libc.so.6 => /usr/lib/libc.so.6"')
        self.command("update-desktop-database", "exit 0")
        self.archive("RustCode-1.2.3-linux-x86_64.tar.gz", {
            "./usr/bin/rc": b"#!/bin/sh\necho shared-engine\n",
            "./usr/bin/rustcode-desktop": b"#!/bin/sh\nexit 0\n",
        })
        self.appimage = self.assets / "RustCode-1.2.3-linux-x86_64.AppImage"
        self.appimage.write_text("#!/bin/sh\nexit 1\n")
        # Deliberately no standalone CLI archive: shared installs must not fetch it.
        self.manifest()

    def command(self, name, body):
        path = self.commands / name
        path.write_text("#!/bin/sh\n" + body + "\n")
        path.chmod(0o755)

    def archive(self, name, files):
        with tarfile.open(self.assets / name, "w:gz") as archive:
            for path, content in files.items():
                entry = tarfile.TarInfo(path)
                entry.mode = 0o755
                entry.size = len(content)
                archive.addfile(entry, io.BytesIO(content))

    def manifest(self):
        entries = []
        for path in self.assets.iterdir():
            if path.name != "SHA256SUMS":
                entries.append(hashlib.sha256(path.read_bytes()).hexdigest() + "  " + path.name)
        (self.assets / "SHA256SUMS").write_text("\n".join(entries) + "\n")

    def run_installer(self, *args, success=True):
        env = dict(os.environ, HOME=str(self.home), XDG_DATA_HOME=str(self.home / "data"),
                   RUSTCODE_INSTALL_ARTIFACTS=str(self.assets),
                   PATH=str(self.commands) + os.pathsep + os.environ["PATH"])
        result = subprocess.run(["bash", str(INSTALLER), "--bin-dir", str(self.home / "bin"), *args],
                                env=env, text=True, capture_output=True, timeout=20)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result.stdout + result.stderr

    @property
    def engine(self):
        return self.home / "data/rustcode/desktop/usr/bin/rc"

    def test_auto_shares_engine_and_preserves_link_on_reinstall(self):
        self.run_installer()
        cli = self.home / "bin/rc"
        self.assertTrue(cli.is_symlink())
        self.assertEqual(cli.resolve(), self.engine)
        self.assertEqual(subprocess.check_output([str(cli)], text=True).strip(), "shared-engine")
        self.run_installer()
        self.assertTrue(cli.is_symlink())
        entry = (self.home / "data/applications/rustcode.desktop").read_text()
        self.assertIn('Exec="' + str(self.engine.with_name("rustcode-desktop")) + '" %U', entry)

    def test_no_cli_preserves_existing_cli(self):
        cli = self.home / "bin/rc"
        cli.parent.mkdir()
        cli.write_text("existing CLI")
        self.run_installer("--no-cli", "--system-webkit")
        self.assertEqual(cli.read_text(), "existing CLI")
        self.assertTrue(self.engine.exists())

    def test_missing_webkit_is_actionable_and_does_not_install(self):
        self.command("ldconfig", "exit 0")
        output = self.run_installer("--system-webkit", success=False)
        self.assertIn("sudo pacman -S webkit2gtk-4.1", output)
        self.assertFalse(self.engine.exists())
        self.assertFalse((self.home / "bin/rc").exists())

    def test_missing_transitive_dependency_stops_before_install(self):
        self.command("ldd", 'echo "libexample.so => not found"')
        self.run_installer("--system-webkit", success=False)
        self.assertFalse(self.engine.exists())

    def test_checksum_failure_stops_before_install(self):
        with (self.assets / "RustCode-1.2.3-linux-x86_64.tar.gz").open("ab") as archive:
            archive.write(b"tampered")
        self.assertIn("SHA-256 mismatch", self.run_installer("--system-webkit", success=False))
        self.assertFalse(self.engine.exists())

    def test_incomplete_archive_stops_before_install(self):
        self.archive("RustCode-1.2.3-linux-x86_64.tar.gz", {"./usr/bin/rc": b"engine"})
        self.manifest()
        self.run_installer("--system-webkit", success=False)
        self.assertFalse(self.engine.exists())

    def test_explicit_appimage_and_auto_fallback(self):
        self.run_installer("--appimage", "--no-cli")
        self.assertEqual((self.home / "Applications/RustCode.AppImage").read_bytes(), self.appimage.read_bytes())
        self.assertFalse(self.engine.exists())
        self.command("ldconfig", "exit 0")
        self.run_installer("--no-cli")
        self.assertFalse(self.engine.exists())

    def test_old_release_without_tarball_uses_appimage(self):
        (self.assets / "RustCode-1.2.3-linux-x86_64.tar.gz").unlink()
        self.manifest()
        self.run_installer("--no-cli")
        self.assertTrue((self.home / "Applications/RustCode.AppImage").exists())

    def test_with_harness_installs_standalone_rustcode_binary(self):
        self.archive("rc-linux-x86_64.tar.gz", {
            "rc-linux-x86_64": b"#!/bin/sh\necho rc-cli\n",
        })
        self.archive("rustcode-linux-x86_64.tar.gz", {
            "rustcode-linux-x86_64": b"#!/bin/sh\necho standalone-harness\n",
        })
        self.manifest()
        self.run_installer("--no-app", "--with-harness")
        self.assertEqual(
            subprocess.check_output([str(self.home / "bin/rc")], text=True).strip(),
            "rc-cli",
        )
        self.assertEqual(
            subprocess.check_output([str(self.home / "bin/rustcode")], text=True).strip(),
            "standalone-harness",
        )


if __name__ == "__main__":
    unittest.main()
