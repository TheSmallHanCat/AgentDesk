"""Linux daemon lifecycle checks; no SSH, account login or model requests."""
import os
from pathlib import Path
import subprocess
import sys
import time

import pytest


@pytest.mark.skipif(os.name != "posix", reason="flock and Unix socket daemon require Linux")
def test_daemon_single_instance_pid_permissions_and_idle_exit(tmp_path):
    home = tmp_path / "agentdesk"
    command = [sys.executable, "-m", "agentdesk_host", "--home", str(home), "--fake", "--idle-exit", "1"]
    environment = os.environ.copy()
    environment["PYTHONPATH"] = str(Path(__file__).resolve().parents[1])
    first = subprocess.Popen(command, env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        deadline = time.monotonic() + 10
        while not (home / "run/host.pid").exists() and time.monotonic() < deadline:
            assert first.poll() is None
            time.sleep(0.02)
        assert (home / "run/host.sock").is_socket()
        assert (home / "run/host.sock").stat().st_mode & 0o777 == 0o600
        assert (home / "run/host.pid").read_text() == str(first.pid)
        assert (home / "state/host.sqlite").stat().st_mode & 0o777 == 0o600
        second = subprocess.run(command, env=environment, capture_output=True, timeout=5)
        assert second.returncode != 0
        assert b"already running" in second.stderr + second.stdout
        assert first.wait(timeout=10) == 0
        assert not (home / "run/host.sock").exists()
        assert not (home / "run/host.pid").exists()
    finally:
        if first.poll() is None:
            first.terminate()
            first.wait(timeout=5)
