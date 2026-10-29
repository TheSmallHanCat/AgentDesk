"""Cross-language dependency checks must fail before resource downloads."""

import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location(
    "check_tauri_versions", Path(__file__).resolve().parents[1] / "build/check_tauri_versions.py"
)
checker = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(checker)


class TauriVersionTests(unittest.TestCase):
    def setUp(self):
        self.package = {"dependencies": {
            "@tauri-apps/api": "2.12.4",
            "@tauri-apps/plugin-dialog": "2.8.3",
            "vue": "3.5.42",
        }}
        self.manifest = {"dependencies": {
            "tauri": {"version": "=2.12.5", "features": ["tray-icon"]},
            "tauri-plugin-dialog": "=2.8.3",
        }}
        self.lock = {"package": [
            {"name": "tauri", "version": "2.12.5"},
            {"name": "tauri-plugin-dialog", "version": "2.8.3"},
        ]}

    def check(self):
        return checker.check_versions(self.package, self.manifest, self.lock)

    def test_core_patch_difference_is_supported(self):
        self.assertEqual(self.check(), [])

    def test_reported_release_mismatches_are_rejected(self):
        self.package["dependencies"]["@tauri-apps/api"] = "2.11.0"
        self.package["dependencies"]["@tauri-apps/plugin-dialog"] = "2.7.1"
        errors = self.check()
        self.assertEqual(len(errors), 2)
        self.assertIn("same major/minor", errors[0])
        self.assertIn("same exact version", errors[1])

    def test_plugin_patch_difference_is_rejected(self):
        self.package["dependencies"]["@tauri-apps/plugin-dialog"] = "2.8.4"
        self.assertIn("same exact version", self.check()[0])

    def test_javascript_ranges_are_rejected(self):
        self.package["dependencies"]["@tauri-apps/api"] = "^2.12.4"
        self.assertIn("pin an exact", self.check()[0])

    def test_cargo_ranges_and_stale_lock_are_rejected(self):
        for declaration in ("2", "=2.8.2"):
            with self.subTest(declaration=declaration):
                self.manifest["dependencies"]["tauri-plugin-dialog"] = declaration
                self.assertIn("Cargo.toml must pin =2.8.3", self.check()[0])

    def test_missing_rust_counterpart_is_rejected(self):
        self.lock["package"].pop()
        self.assertIn("expected one locked version, found 0", self.check()[0])

    def test_duplicate_rust_versions_are_rejected(self):
        self.lock["package"].append({"name": "tauri", "version": "2.11.0"})
        self.assertIn("expected one locked version, found 2", self.check()[0])

    def test_new_plugin_is_automatically_checked(self):
        self.package["dependencies"]["@tauri-apps/plugin-opener"] = "2.7.2"
        self.assertIn("tauri-plugin-opener", self.check()[0])

    def test_missing_core_api_is_rejected(self):
        del self.package["dependencies"]["@tauri-apps/api"]
        self.assertIn("Missing @tauri-apps/api dependency", self.check())


if __name__ == "__main__":
    unittest.main()
