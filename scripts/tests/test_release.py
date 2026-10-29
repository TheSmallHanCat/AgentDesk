"""Release metadata and archive boundary tests; no downloads or service access."""

import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("release", ROOT / "scripts/build/release.py")
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="agentdesk-release-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def write(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        return path

    def write_json(self, name, data):
        return self.write(name, json.dumps(data))

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.root, text=True, stderr=subprocess.PIPE).strip()

    def prepare_git(self):
        self.git("init", "--quiet")
        self.git("config", "user.name", "Release Tests")
        self.git("config", "user.email", "release@example.invalid")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "tag.gpgsign", "false")
        self.git("commit", "--allow-empty", "-m", "release fixture", "--quiet")

    def prepare_versions(self):
        for name in ("package.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock", "host/pyproject.toml"):
            self.write(name, (ROOT / name).read_text(encoding="utf-8"))

    def prepare_assets(self):
        self.write("build/agentdesk.exe", "desktop executable")
        self.write("build/bundle/nsis/installer.exe", "installer")
        self.write("LICENSE", "license text")
        self.write("NOTICE", "copyright notices")
        entries = []
        for name, filename in (("host", "agentdesk_host.whl"), ("dependencies", "dependencies.txt"), ("python-metadata", "python-downloads.json")):
            path = self.write(f"resources/host/{filename}", name)
            entries.append({"name": name, "path": filename, "sha256": release.sha256(path)})
        entries.append({"name": "python", "url": "https://example.invalid/python.tar.gz"})
        self.write_json("resources/host/manifest.json", {"files": entries})
        for name, executable in (("openssh", "sshd.exe"), ("powershell", "pwsh.exe")):
            path = self.write(f"resources/windows/{name}/{executable}", name)
            license_file = self.write(f"resources/windows/{name}/LICENSE.txt", "third-party license")
            self.write_json(f"resources/windows/{name}/.agentdesk-component.json", {"files": {
                executable: release.sha256(path), "LICENSE.txt": release.sha256(license_file),
            }})

    def collect(self):
        return release.collect_windows(self.root, "v1.2.3-rc.1", self.root / "build", self.root / "output")

    def test_valid_tags(self):
        for tag in ("v0.1.0", "v1.20.300", "v1.2.3-rc.1", "v1.2.3-beta", "v1.2.3-0"):
            with self.subTest(tag=tag):
                self.assertEqual(release.version_from_tag(tag), tag[1:])

    def test_invalid_tags(self):
        for tag in ("1.2.3", "v1.2", "v01.2.3", "v1.02.3", "v1.2.03", "v1.2.3-01", "v1.2.3-rc.01", "v1.2.3+local", "v1.2.3\n", "v1.2.3;echo bad", "v1.2.3-", "v1.2.3-rc..1", "../v1.2.3"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                release.version_from_tag(tag)

    def test_lightweight_and_annotated_tags(self):
        self.prepare_git()
        self.git("tag", "v1.2.3")
        self.git("tag", "-a", "v1.2.4-rc.1", "-m", "candidate")
        head = self.git("rev-parse", "HEAD")
        self.assertEqual(release.metadata(self.root, "v1.2.3", head), {
            "tag": "v1.2.3", "version": "1.2.3", "prerelease": "false", "commit": head,
        })
        self.assertEqual(release.metadata(self.root, "v1.2.4-rc.1")["prerelease"], "true")

    def test_tag_must_match_checkout(self):
        self.prepare_git()
        self.git("tag", "v1.2.3")
        self.git("commit", "--allow-empty", "-m", "next", "--quiet")
        with self.assertRaisesRegex(ValueError, "checked-out"):
            release.metadata(self.root, "v1.2.3")

    def test_moved_tag_rejected(self):
        self.prepare_git()
        original = self.git("rev-parse", "HEAD")
        self.git("commit", "--allow-empty", "-m", "next", "--quiet")
        self.git("tag", "v1.2.3")
        with self.assertRaisesRegex(ValueError, "moved"):
            release.metadata(self.root, "v1.2.3", original)

    def test_missing_tag_rejected(self):
        self.prepare_git()
        with self.assertRaises(subprocess.CalledProcessError):
            release.metadata(self.root, "v1.2.3")

    def test_sync_updates_all_desktop_versions_only(self):
        self.prepare_versions()
        before = tomllib.loads((self.root / "src-tauri/Cargo.lock").read_text())["package"]
        host_before = (self.root / "host/pyproject.toml").read_bytes()
        release.sync_version(self.root, "v1.2.3-rc.1")
        for name in ("package.json", "src-tauri/tauri.conf.json"):
            self.assertEqual(json.loads((self.root / name).read_text(encoding="utf-8"))["version"], "1.2.3-rc.1")
        self.assertEqual(tomllib.loads((self.root / "src-tauri/Cargo.toml").read_text())["package"]["version"], "1.2.3-rc.1")
        after = tomllib.loads((self.root / "src-tauri/Cargo.lock").read_text())["package"]
        expected = [dict(p, version="1.2.3-rc.1") if p["name"] == "agentdesk" else p for p in before]
        self.assertEqual(after, expected)
        self.assertEqual((self.root / "host/pyproject.toml").read_bytes(), host_before)
        release.sync_version(self.root, "v1.2.3-rc.1")
        self.assertEqual(tomllib.loads((self.root / "src-tauri/Cargo.lock").read_text())["package"], expected)

    def test_invalid_lock_does_not_partially_sync(self):
        self.prepare_versions()
        before = (self.root / "package.json").read_bytes()
        self.write("src-tauri/Cargo.lock", 'version = 4\n[[package]]\nname = "other"\nversion = "0.1.0"\n')
        with self.assertRaises(ValueError):
            release.sync_version(self.root, "v1.2.3")
        self.assertEqual((self.root / "package.json").read_bytes(), before)

    def test_archive_layout_and_checksums_exclude_local_data(self):
        self.prepare_assets()
        for name in ("resources/host/old.whl", "resources/windows/powershell/private.txt", "build/data/settings.json", ".env", "ssh.txt"):
            self.write(name, "must not be shipped")
        assets = self.collect()
        prefix = "AgentDesk-1.2.3-rc.1-windows-x64/"
        with zipfile.ZipFile(assets[1]) as archive:
            names = set(archive.namelist())
            expected = {"agentdesk.exe", "agentdesk.portable", "README.txt", "LICENSE", "NOTICE"}
            expected |= {path.relative_to(self.root).as_posix() for path in release.windows_resources(self.root)}
            self.assertEqual(names, {prefix + name for name in expected})
            self.assertEqual(archive.read(prefix + "agentdesk.portable"), b"")
            self.assertIn(b"Credential Manager", archive.read(prefix + "README.txt"))
        self.assertEqual(assets[0].read_bytes(), b"installer")
        lines = (self.root / "output/SHA256SUMS.txt").read_text().splitlines()
        self.assertEqual(len(lines), 2)
        for line in lines:
            digest, name = line.split("  ")
            self.assertEqual(digest, release.sha256(self.root / "output" / name))

    def test_changed_resource_rejected_before_output(self):
        self.prepare_assets()
        self.write("resources/windows/powershell/pwsh.exe", "modified")
        with self.assertRaisesRegex(ValueError, "checksum"):
            self.collect()
        self.assertFalse((self.root / "output").exists())

    def test_missing_host_resource_rejected(self):
        self.prepare_assets()
        self.write_json("resources/host/manifest.json", {"files": []})
        with self.assertRaisesRegex(ValueError, "Incomplete Host"):
            self.collect()

    def test_unsafe_resource_paths_rejected(self):
        self.prepare_assets()
        for name in ("../LICENSE", "/LICENSE", "C:/LICENSE", "folder\\LICENSE", ""):
            with self.subTest(name=name), self.assertRaises(ValueError):
                release.resource_file(self.root, name)

    def test_missing_or_multiple_installers_rejected(self):
        self.prepare_assets()
        self.write("build/bundle/nsis/second.exe", "second")
        with self.assertRaisesRegex(ValueError, "one nonempty"):
            self.collect()

    def test_empty_executable_rejected(self):
        self.prepare_assets()
        self.write("build/agentdesk.exe", "")
        with self.assertRaisesRegex(ValueError, "nonempty"):
            self.collect()

    def test_output_is_not_overwritten(self):
        self.prepare_assets()
        existing = self.write("output/keep.txt", "existing data")
        with self.assertRaisesRegex(ValueError, "must be empty"):
            self.collect()
        self.assertEqual(existing.read_text(), "existing data")


if __name__ == "__main__":
    unittest.main()
