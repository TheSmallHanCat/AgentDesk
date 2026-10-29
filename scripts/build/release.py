"""Resolve release tags, synchronize desktop versions and assemble Windows assets."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[2]
NUMBER = r"(?:0|[1-9][0-9]*)"
TAG = re.compile(rf"v({NUMBER}\.{NUMBER}\.{NUMBER})(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?")


def version_from_tag(tag: str) -> str:
    match = TAG.fullmatch(tag)
    if not match or any(
        item.isdigit() and len(item) > 1 and item.startswith("0")
        for item in (match[2] or "").split(".")
    ):
        raise ValueError("Expected vMAJOR.MINOR.PATCH or vMAJOR.MINOR.PATCH-prerelease")
    return tag[1:]


def metadata(root: Path, tag: str, expected_commit: str = "") -> dict[str, str]:
    version = version_from_tag(tag)

    def commit(ref: str) -> str:
        return subprocess.check_output(
            ["git", "rev-parse", "--verify", f"{ref}^{{commit}}"], cwd=root, text=True
        ).strip()

    head = commit("HEAD")
    if commit(f"refs/tags/{tag}") != head:
        raise ValueError("The checked-out commit does not match the release tag")
    if expected_commit and commit(expected_commit) != head:
        raise ValueError("The release tag moved after the build was requested")
    return {"tag": tag, "version": version, "prerelease": str("-" in version).lower(), "commit": head}


def sync_version(root: Path, tag: str) -> None:
    version = version_from_tag(tag)
    changes = {}
    for name in ("package.json", "src-tauri/tauri.conf.json"):
        path = root / name
        data = json.loads(path.read_text(encoding="utf-8"))
        data["version"] = version
        changes[path] = json.dumps(data, ensure_ascii=False, indent=2) + "\n"

    cargo = root / "src-tauri/Cargo.toml"
    text = cargo.read_text(encoding="utf-8")
    package = tomllib.loads(text)["package"]
    if package["name"] != "agentdesk":
        raise ValueError("Unexpected desktop package name")
    pattern = r'(?ms)(^\[package\]\n(?:(?!^\[).)*?^version = ")[^"]+("$)'
    changes[cargo], count = re.subn(pattern, lambda m: m[1] + version + m[2], text)
    if count != 1:
        raise ValueError("Cannot locate the desktop Cargo.toml version")

    lock = root / "src-tauri/Cargo.lock"
    text = lock.read_text(encoding="utf-8")
    packages = [p for p in tomllib.loads(text)["package"] if p["name"] == "agentdesk"]
    if len(packages) != 1 or "source" in packages[0]:
        raise ValueError("Expected one local agentdesk package in Cargo.lock")
    pattern = r'(?m)(^name = "agentdesk"\nversion = ")[^"]+("$)'
    changes[lock], count = re.subn(pattern, lambda m: m[1] + version + m[2], text)
    if count != 1:
        raise ValueError("Cannot locate the desktop Cargo.lock version")
    for path, content in changes.items():
        path.write_text(content, encoding="utf-8", newline="\n")


def sha256(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def resource_file(directory: Path, name: str, digest: str | None = None) -> Path:
    relative = PurePosixPath(name)
    if not relative.parts or relative.is_absolute() or ".." in relative.parts or "\\" in name or ":" in name:
        raise ValueError(f"Unsafe resource path: {name}")
    path = directory.joinpath(*relative.parts)
    if not path.resolve().is_relative_to(directory.resolve()) or not path.is_file():
        raise ValueError(f"Missing or external resource: {name}")
    if digest is not None and sha256(path) != digest:
        raise ValueError(f"Resource checksum mismatch: {name}")
    return path


def windows_resources(root: Path) -> list[Path]:
    host = root / "resources/host"
    manifest = resource_file(host, "manifest.json")
    files = [manifest]
    entries = json.loads(manifest.read_text(encoding="utf-8"))["files"]
    local = [entry for entry in entries if "path" in entry]
    if {entry["name"] for entry in local} != {"host", "dependencies", "python-metadata"}:
        raise ValueError("Incomplete Host resources")
    files.extend(resource_file(host, entry["path"], entry["sha256"]) for entry in local)
    for component in ("openssh", "powershell"):
        directory = root / "resources/windows" / component
        manifest = resource_file(directory, ".agentdesk-component.json")
        entries = json.loads(manifest.read_text(encoding="utf-8"))["files"]
        if not entries:
            raise ValueError(f"Empty Windows component: {component}")
        files.append(manifest)
        files.extend(resource_file(directory, name, digest) for name, digest in entries.items())
    return files


def collect_windows(root: Path, tag: str, target: Path, output: Path) -> list[Path]:
    version = version_from_tag(tag)
    executable = resource_file(target, "agentdesk.exe")
    installers = list((target / "bundle/nsis").glob("*.exe"))
    if len(installers) != 1 or not installers[0].stat().st_size or not executable.stat().st_size:
        raise ValueError("Expected one nonempty NSIS installer and desktop executable")
    resources = windows_resources(root)
    licenses = [resource_file(root, name) for name in ("LICENSE", "NOTICE")]
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        raise ValueError("Release output must be empty; choose a new output directory")
    name = f"AgentDesk-{version}-windows-x64"
    installer = output / f"{name}-setup.exe"
    archive = output / f"{name}-portable.zip"
    shutil.copyfile(installers[0], installer)
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as package:
        package.write(executable, f"{name}/agentdesk.exe")
        package.writestr(f"{name}/agentdesk.portable", "")
        package.writestr(
            f"{name}/README.txt",
            "AgentDesk Windows x64\n\n"
            "Extract the entire archive into a writable directory and run agentdesk.exe.\n"
            "Microsoft Edge WebView2 Runtime is required.\n"
            "Application data is stored in data/ beside the executable.\n"
            "Passwords and private-key passphrases remain in Windows Credential Manager;\n"
            "they do not travel with this directory.\n"
            "This build is unsigned.\n",
        )
        for source in [*licenses, *resources]:
            package.write(source, f"{name}/{source.relative_to(root).as_posix()}")
    assets = [installer, archive]
    (output / "SHA256SUMS.txt").write_text(
        "".join(f"{sha256(path)}  {path.name}\n" for path in sorted(assets)),
        encoding="utf-8", newline="\n",
    )
    return assets


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("metadata", "sync-version", "collect-windows"))
    parser.add_argument("--tag", required=True)
    parser.add_argument("--expected-commit", default="")
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--target", type=Path, default=ROOT / "src-tauri/target/x86_64-pc-windows-msvc/release")
    parser.add_argument("--output", type=Path, default=ROOT / ".tmp/release")
    args = parser.parse_args()
    try:
        if args.command == "metadata":
            data = metadata(ROOT, args.tag, args.expected_commit)
            if args.github_output:
                with args.github_output.open("a", encoding="utf-8", newline="\n") as output:
                    output.write("".join(f"{key}={value}\n" for key, value in data.items()))
            print(json.dumps(data))
        elif args.command == "sync-version":
            sync_version(ROOT, args.tag)
            print(f"Desktop version: {version_from_tag(args.tag)}")
        else:
            for path in collect_windows(ROOT, args.tag, args.target.resolve(), args.output.resolve()):
                print(path.name)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Release: {error}\n")


if __name__ == "__main__":
    main()
