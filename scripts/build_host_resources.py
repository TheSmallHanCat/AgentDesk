"""Build the small Host wheel and manifest; large official assets stay external."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "resources" / "host"
PYTHON = {
    "name": "cpython", "arch": {"family": "x86_64", "variant": None},
    "os": "linux", "libc": "gnu", "major": 3, "minor": 12, "patch": 15,
    "prerelease": "", "variant": None, "build": "20261021",
    "url": "https://github.com/astral-sh/python-build-standalone/releases/download/20261021/cpython-3.12.15%2B20261021-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz",
    "sha256": "6d7521138fd33afddc1ccd08abb007d9a2829e38117c2576f9efdc0ec73ce772",
}
ASSETS = [
    {"name": "python", "version": "3.12.15+20261021", "url": PYTHON["url"], "sha256": PYTHON["sha256"]},
    {"name": "claude-sdk", "version": "0.2.174", "url": "https://files.pythonhosted.org/packages/7c/f6/6f63561fbb79d9d2bd26219dffe1458c99b59ee5ae78bb9dac808c22d546/claude_agent_sdk-0.2.174-py3-none-manylinux_2_17_x86_64.whl", "sha256": "728592effdfe63e96fbad7ba3b2ddf642e3aad344e73692b429898fbfcb33f12", "filename": "claude_agent_sdk-0.2.174-py3-none-manylinux_2_17_x86_64.whl"},
    {"name": "uv", "version": "0.13.5", "url": "https://github.com/astral-sh/uv/releases/download/0.13.5/uv-x86_64-unknown-linux-musl.tar.gz", "sha256": "57ef9d9ccb01fb1bd998df995b0dd221b52f7d7841fbea93f2d4463bf61fb7a9", "member": "uv-x86_64-unknown-linux-musl/uv"},
    {"name": "codex", "version": "0.164.0", "url": "https://github.com/openai/codex/releases/download/rust-v0.164.0/codex-x86_64-unknown-linux-musl.tar.gz", "sha256": "90dd0b03b28afcb7fa733a4f73b413db2916a47347efd43fb669cc1c774ee8e4", "member": "codex-x86_64-unknown-linux-musl"},
    {"name": "bwrap", "version": "0.164.0", "url": "https://github.com/openai/codex/releases/download/rust-v0.164.0/bwrap-x86_64-unknown-linux-musl.tar.gz", "sha256": "60cd1c139823eabac2e4544cc75081d02af5100cfd276efa378dd48a123e109d", "member": "bwrap-x86_64-unknown-linux-musl"},
]


def main():
    OUTPUT.mkdir(parents=True, exist_ok=True)
    result = subprocess.run(["uv", "build", "--wheel", "--out-dir", str(OUTPUT), str(ROOT / "host")], cwd=ROOT, capture_output=True)
    if result.returncode:
        print(result.stderr.decode(errors="replace"), file=sys.stderr)
        return result.returncode
    import tomllib
    version = tomllib.loads((ROOT / "host/pyproject.toml").read_text(encoding="utf-8"))["project"]["version"]
    wheel = OUTPUT / f"agentdesk_host-{version}-py3-none-any.whl"
    deps = OUTPUT / "dependencies.txt"
    subprocess.run(["uv", "export", "--project", str(ROOT / "host"), "--extra", "claude", "--no-dev", "--no-emit-project", "--format", "requirements-txt", "--output-file", str(deps)], cwd=ROOT, check=True, capture_output=True)
    metadata = OUTPUT / "python-downloads.json"
    metadata.write_text(json.dumps({"cpython-3.12.15-linux-x86_64-gnu": PYTHON}, indent=2) + "\n", encoding="utf-8")
    manifest = {"version": version, "sdkVersion": "0.2.174", "files": [{"name": "host", "path": wheel.name, "sha256": hashlib.sha256(wheel.read_bytes()).hexdigest()}, {"name": "dependencies", "path": deps.name, "sha256": hashlib.sha256(deps.read_bytes()).hexdigest()}, {"name": "python-metadata", "path": metadata.name, "sha256": hashlib.sha256(metadata.read_bytes()).hexdigest()}, *ASSETS]}
    (OUTPUT / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print("Host wheel and SHA256 manifest prepared:", version)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
