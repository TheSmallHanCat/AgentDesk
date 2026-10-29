"""Check the paired JavaScript/Rust Tauri versions before building installers."""

from __future__ import annotations

import json
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")


def check_versions(package: dict, manifest: dict, lock: dict) -> list[str]:
    errors = []
    dependencies = package["dependencies"]
    if "@tauri-apps/api" not in dependencies:
        errors.append("Missing @tauri-apps/api dependency")
    for name, js_version in dependencies.items():
        if name == "@tauri-apps/api":
            crate = "tauri"
        elif name.startswith("@tauri-apps/plugin-"):
            crate = name.replace("@tauri-apps/", "tauri-", 1)
        else:
            continue
        if not VERSION.fullmatch(js_version):
            errors.append(f"{name}: pin an exact stable version instead of {js_version!r}")
            continue
        packages = [item for item in lock["package"] if item["name"] == crate]
        if len(packages) != 1:
            errors.append(f"{crate}: expected one locked version, found {len(packages)}")
            continue
        rust_version = packages[0]["version"]
        if not VERSION.fullmatch(rust_version):
            errors.append(f"{crate}: expected a stable version, found {rust_version!r}")
            continue
        declaration = manifest["dependencies"].get(crate)
        constraint = declaration.get("version") if isinstance(declaration, dict) else declaration
        if constraint != f"={rust_version}":
            errors.append(f"{crate}: Cargo.toml must pin ={rust_version} to match Cargo.lock")
        compatible = (
            js_version.split(".")[:2] == rust_version.split(".")[:2]
            if crate == "tauri"
            else js_version == rust_version
        )
        if not compatible:
            rule = "same major/minor" if crate == "tauri" else "same exact version"
            errors.append(f"{name} {js_version} / {crate} {rust_version}: require {rule}")
    return errors


def main() -> int:
    package = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))
    manifest = tomllib.loads((ROOT / "src-tauri/Cargo.toml").read_text(encoding="utf-8"))
    lock = tomllib.loads((ROOT / "src-tauri/Cargo.lock").read_text(encoding="utf-8"))
    errors = check_versions(package, manifest, lock)
    for error in errors:
        print(f"Tauri versions: {error}", file=sys.stderr)
    if errors:
        return 1
    print("Tauri JavaScript/Rust versions are aligned and pinned.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
