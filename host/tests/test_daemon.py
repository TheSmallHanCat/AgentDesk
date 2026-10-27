"""Linux daemon lifecycle checks; no SSH, account login or model requests."""
import os
from pathlib import Path
import subprocess
import sys
import time

import pytest


@pytest.mark.skipif(os.name != "posix", reason="flock and Unix socket daemon require Linux")
def test_daemon_single_instance_pid_permissions_and_idle_exit(socket_home):
    home = socket_home / "agentdesk"
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


@pytest.mark.skipif(os.name != 'posix', reason='Unix socket daemon requires Linux')
def test_sigterm_exits_while_control_connection_is_still_open(tmp_path):
    import socket
    with __import__('tempfile').TemporaryDirectory(prefix='ad-stop-') as directory:
        home = Path(directory)
        environment = {**os.environ, 'PYTHONPATH': str(Path(__file__).resolve().parents[1])}
        process = subprocess.Popen([sys.executable, '-m', 'agentdesk_host', '--home', str(home), '--fake', '--idle-exit', '0'], env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        control = socket.socket(socket.AF_UNIX)
        try:
            deadline = time.monotonic() + 10
            while not (home / 'run/host.pid').exists() and time.monotonic() < deadline:
                assert process.poll() is None
                time.sleep(.02)
            control.connect(str(home / 'run/host.sock'))
            control.sendall(b'{"jsonrpc":"2.0","id":1,"method":"hello","params":{"protocolVersion":"1.0","client":"test","deviceId":"shutdown-test","deviceName":"Test","os":"linux"}}\n')
            control.settimeout(5)
            assert b'"result"' in control.recv(65536)
            process.terminate()
            # Keep the client alive until after process exit: wait_closed must
            # not depend on the caller releasing its transport first.
            assert process.wait(timeout=5) == 0
            assert not (home / 'run/host.sock').exists()
            assert not (home / 'run/host.pid').exists()
        finally:
            control.close()
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
