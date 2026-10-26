"""Prepare pinned official Windows executables; no binary is tracked in Git."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import tempfile
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]
ASSETS = (
    {
        "name": "openssh",
        "version": "10.4.0.0p1-Preview",
        "url": "https://github.com/PowerShell/Win32-OpenSSH/releases/download/10.4.0.0p1-Preview/OpenSSH-Win64.zip",
        "sha256": "b67ff8becd203502190ec42ac7787c24c2cd3325ed7700485fe7a1c83d2a7957",
        "prefix": "OpenSSH-Win64",
        "required": ["sshd.exe", "ssh-keygen.exe", "sftp-server.exe"],
    },
    {
        "name": "powershell",
        "version": "7.4.22",
        "url": "https://github.com/PowerShell/PowerShell/releases/download/v7.4.22/PowerShell-7.4.22-win-x64.zip",
        "sha256": "5f89f51860a1a3b07937945663afd7cbd44387361ff9985f5758939cb9bd6359",
        "prefix": "",
        "required": ["pwsh.exe"],
    },
)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def valid_component(path: Path, asset: dict) -> bool:
    try:
        manifest = json.loads((path / ".agentdesk-component.json").read_text(encoding="utf-8"))
        if manifest["archiveSha256"] != asset["sha256"] or manifest["version"] != asset["version"]:
            return False
        if not all((path / name).is_file() for name in asset["required"]):
            return False
        for name, digest in manifest["files"].items():
            relative = PurePosixPath(name)
            if relative.is_absolute() or ".." in relative.parts or "\\" in name:
                return False
            if sha256(path.joinpath(*relative.parts)) != digest:
                return False
        return True
    except (OSError, ValueError, KeyError, TypeError):
        return False


def download(asset: dict, destination: Path) -> None:
    request = urllib.request.Request(asset["url"], headers={"User-Agent": "AgentDesk-resource-builder"})
    digest = hashlib.sha256()
    total = 0
    with urllib.request.urlopen(request, timeout=60) as response, destination.open("xb") as output:
        while chunk := response.read(1024 * 1024):
            total += len(chunk)
            if total > 512 * 1024 * 1024:
                raise RuntimeError("Official component archive exceeds 512 MiB")
            digest.update(chunk)
            output.write(chunk)
    if digest.hexdigest() != asset["sha256"]:
        raise RuntimeError(f"SHA256 mismatch for {asset['name']}; archive rejected")


def extract(archive: Path, output: Path, asset: dict) -> dict[str, str]:
    files = {}
    with zipfile.ZipFile(archive) as source:
        for member in source.infolist():
            relative = PurePosixPath(member.filename)
            if relative.is_absolute() or ".." in relative.parts or "\\" in member.filename:
                raise RuntimeError("Unsafe path in official component archive")
            if stat.S_ISLNK(member.external_attr >> 16):
                raise RuntimeError("Symlink in official component archive")
            parts = relative.parts
            if asset["prefix"]:
                if not parts or parts[0] != asset["prefix"]:
                    raise RuntimeError("Unexpected official component archive layout")
                parts = parts[1:]
            if not parts:
                continue
            if ":" in parts[0]:
                raise RuntimeError("Drive path in official component archive")
            target = output.joinpath(*parts)
            if member.is_dir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with source.open(member) as data, target.open("xb") as destination:
                    shutil.copyfileobj(data, destination)
                files[PurePosixPath(*parts).as_posix()] = sha256(target)
    for name in asset["required"]:
        if not (output / name).is_file():
            raise RuntimeError(f"Required component is missing: {name}")
    return files


def prepare(output: Path, check_only: bool) -> int:
    output.mkdir(parents=True, exist_ok=True)
    for asset in ASSETS:
        component = output / asset["name"]
        if component.is_symlink():
            raise RuntimeError(f"Generated component path cannot be a symlink: {component}")
        if component.exists() and not (component / ".agentdesk-component.json").is_file():
            raise RuntimeError(f"Refusing to replace an unmanaged component directory: {component}")
        if valid_component(component, asset):
            print(f"Verified {asset['name']} {asset['version']}")
            continue
        if check_only:
            raise RuntimeError(f"Missing or modified component: {component}; run python scripts/build_windows_resources.py")
        with tempfile.TemporaryDirectory(prefix=".prepare-", dir=output) as temporary:
            staging = Path(temporary)
            archive = staging / "official.zip"
            print(f"Downloading official {asset['name']} {asset['version']}")
            download(asset, archive)
            unpacked = staging / "component"
            unpacked.mkdir()
            files = extract(archive, unpacked, asset)
            manifest = {"version": asset["version"], "url": asset["url"], "archiveSha256": asset["sha256"], "files": files}
            (unpacked / ".agentdesk-component.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
            if component.exists():
                # Only this builder's fixed generated component directory is replaced.
                shutil.rmtree(component)
            os.replace(unpacked, component)
            print(f"Prepared {asset['name']} {asset['version']} (SHA256 verified)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Verify existing resources without downloading")
    parser.add_argument("--output", type=Path, default=ROOT / "resources" / "windows")
    args = parser.parse_args()
    try:
        return prepare(args.output.resolve(), args.check)
    except (OSError, RuntimeError, zipfile.BadZipFile) as error:
        parser.exit(1, f"Windows resources: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
