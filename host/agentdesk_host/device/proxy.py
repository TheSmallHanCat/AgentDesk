"""Private Unix socket ProxyCommand. Never writes protocol bytes to diagnostics."""
from __future__ import annotations

import argparse
import select
import socket
import sys
import time
from pathlib import Path


def connect(path: Path, ready: Path, timeout: float) -> socket.socket:
    deadline = time.monotonic() + timeout
    while True:
        if ready.is_file():
            connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                connection.connect(str(path))
                return connection
            except OSError:
                connection.close()
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("Device execution channel did not become ready before the deadline")
        time.sleep(min(0.1, remaining))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", type=Path, required=True)
    parser.add_argument("--ready", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()
    try:
        with connect(args.socket, args.ready, args.timeout) as connection:
            sources = [connection, sys.stdin.buffer]
            while connection in sources:
                readable, _, _ = select.select(sources, [], [])
                for source in readable:
                    if source is connection:
                        chunk = connection.recv(65536)
                        if not chunk:
                            return
                        sys.stdout.buffer.write(chunk)
                        sys.stdout.buffer.flush()
                    else:
                        chunk = sys.stdin.buffer.read1(65536)
                        if chunk:
                            connection.sendall(chunk)
                        else:
                            sources.remove(source)
                            connection.shutdown(socket.SHUT_WR)
    except (OSError, TimeoutError):
        print("E_DEVICE_OFFLINE: Device execution channel is unavailable or its wait deadline expired", file=sys.stderr)
        raise SystemExit(1) from None


if __name__ == "__main__":
    main()
