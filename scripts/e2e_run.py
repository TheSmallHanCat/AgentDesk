"""Run ignored russh acceptance tests with redacted, structured results."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

from e2e_common import PRIVATE, ROOT, configured, save_result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--filter", default="real_", help="Rust test name substring")
    args = parser.parse_args()
    if not configured():
        return 0
    if not (PRIVATE / "id_ed25519").is_file():
        print("SKIP: run e2e_prepare.py provision and fixtures first")
        return 0
    environment = os.environ.copy()
    environment.pop("RUST_LOG", None)
    environment["PYTHONUTF8"] = "1"
    agent_added = False
    result = {"filter": args.filter, "passed": [], "skipped": [], "observations": []}
    try:
        # Do not start/change the service. Use it only when already accessible.
        agent = subprocess.run(["ssh-add", "-l"], capture_output=True, timeout=10)
        if agent.returncode in (0, 1):
            addition = subprocess.run(["ssh-add", str(PRIVATE / "id_ed25519")], capture_output=True, timeout=10)
            agent_added = addition.returncode == 0
            if agent_added:
                environment["AGENTDESK_E2E_AGENT"] = "1"
        process = subprocess.Popen(
            ["cargo", "test", "--locked", "--manifest-path", "src-tauri/Cargo.toml", "--test", "real_server", args.filter, "--", "--ignored", "--nocapture", "--test-threads=1"],
            cwd=ROOT, env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            text=True, encoding="utf-8", errors="replace",
        )
        assert process.stdout is not None
        errors = []
        for line in process.stdout:
            for prefix, field in [("PASS:", "passed"), ("SKIP:", "skipped"), ("OBSERVE:", "observations")]:
                if prefix in line:
                    safe = line[line.index(prefix):].strip()
                    protected = [os.environ.get("AGENTDESK_E2E_HOST"), os.environ.get("AGENTDESK_E2E_ROOT_PASSWORD")]
                    if any(value and value in safe for value in protected):
                        raise RuntimeError("Unexpected protected value in test output")
                    print(safe, flush=True)
                    result[field].append(safe[len(prefix):].strip())
            if "error[" in line or "Operation failed:" in line or "panicked at" in line or "test result:" in line or "Product download failed:" in line:
                errors.append(line.strip())
        result["exitCode"] = process.wait()
        # Rust compiler and panic locations only; never retain arbitrary payloads.
        result["diagnostics"] = [line for line in errors if not any(value and value in line for value in (os.environ.get("AGENTDESK_E2E_HOST"), os.environ.get("AGENTDESK_E2E_ROOT_PASSWORD")))]
        print("Rust E2E exit:", result["exitCode"], flush=True)
        for line in result["diagnostics"]:
            print(line, flush=True)
        return 0 if result["exitCode"] == 0 else 1
    finally:
        if agent_added:
            removal = subprocess.run(["ssh-add", "-d", str(PRIVATE / "id_ed25519")], capture_output=True, timeout=10)
            result["agentKeyRemoved"] = removal.returncode == 0
        save_result("acceptance-" + args.filter.replace("/", "_"), result)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception:
        print("FAIL: acceptance runner failed (details suppressed)")
        raise SystemExit(1) from None
