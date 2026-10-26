"""Bounded Unix-socket echo process for the opt-in S6 deployment experiment."""

from __future__ import annotations

import os
from pathlib import Path
import signal
import socket
import sys
import time


def main() -> None:
    directory = Path(sys.argv[1]).resolve()
    allowed = Path("/home/agentdesk-test/agentdesk-e2e")
    if not directory.is_relative_to(allowed) or directory == allowed:
        raise ValueError("Socket files must belong to the isolated test directory")
    path = directory / "host.sock"
    pid = directory / "host.pid"
    # Every invocation gets a fresh directory; never replace an existing socket.
    if path.exists() or path.is_symlink() or pid.exists():
        raise FileExistsError("Socket experiment already exists")
    os.umask(0o077)
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    running = True

    def stop(_signum: int, _frame: object) -> None:
        nonlocal running
        running = False

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        server.bind(str(path))
        os.chmod(path, 0o600)
        with pid.open("x", encoding="ascii") as handle:
            handle.write(str(os.getpid()))
        server.listen(2)
        server.settimeout(0.5)
        deadline = time.monotonic() + 180
        while running and time.monotonic() < deadline:
            try:
                connection, _ = server.accept()
            except TimeoutError:
                continue
            with connection:
                connection.settimeout(5)
                try:
                    data = connection.recv(256)
                    if data == b"stop":
                        running = False
                    connection.sendall(data)
                except (TimeoutError, OSError):
                    pass
    finally:
        server.close()
        path.unlink(missing_ok=True)
        pid.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
