"""Private per-user Host daemon entry point."""
from __future__ import annotations

import argparse
import asyncio
import json
import os
import signal
from pathlib import Path

from .adapters.fake import FakeAdapter
from .server.service import HostService
from .server.transport import HostServer


class InstanceLock:
    def __init__(self, path: Path):
        self.path = path
        self.file = None

    def __enter__(self):
        import fcntl
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        fd = os.open(self.path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        self.file = os.fdopen(fd, "a+")
        try:
            fcntl.flock(self.file, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            self.file.close()
            raise SystemExit("AgentDesk Host is already running") from None
        return self

    def __exit__(self, *args):
        self.file.close()


async def serve(home: Path, fake: bool, idle_exit: float, retention_days: int = 180) -> None:
    adapters = {}
    if fake:
        adapters["fake"] = FakeAdapter()
    from .adapters.claude import ClaudeAdapter
    from .adapters.codex import CodexAdapter
    adapters["claude"] = ClaudeAdapter()
    adapters["codex"] = CodexAdapter()
    service = HostService(home, adapters)
    service.db.prune_events(retention_days)
    server = HostServer(service)
    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, stop.set)
    pid_path = home / "run" / "host.pid"
    next_retention_check = loop.time() + 3600
    try:
        await server.start()
        fd = os.open(pid_path, os.O_CREAT | os.O_WRONLY | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "w") as handle:
            handle.write(str(os.getpid()))
        while not stop.is_set():
            try:
                await asyncio.wait_for(stop.wait(), timeout=min(30, max(0.05, idle_exit)))
            except TimeoutError:
                if loop.time() >= next_retention_check:
                    service.db.prune_events(retention_days)
                    next_retention_check = loop.time() + 3600
                if idle_exit > 0 and not service.handles and not service.clients and loop.time() - service.last_activity >= idle_exit:
                    break
    finally:
        await server.close()
        await service.close()
        pid_path.unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser(prog="agentdesk-host")
    parser.add_argument("--home", type=Path, default=Path.home() / ".agentdesk")
    parser.add_argument("--fake", action="store_true", default=os.environ.get("AGENTDESK_FAKE_AGENT") == "1")
    parser.add_argument("--idle-exit", type=float, default=None)
    args = parser.parse_args()
    config_path = args.home / "config.json"
    config = json.loads(config_path.read_text(encoding="utf-8")) if config_path.is_file() else {}
    idle_exit = args.idle_exit if args.idle_exit is not None else config.get("host", {}).get("idle_exit", 24 * 3600)
    if not isinstance(idle_exit, (int, float)) or idle_exit < 0:
        parser.error("host.idle_exit must be a nonnegative duration in seconds")
    retention_days = config.get("events", {}).get("retention_days", 180)
    if type(retention_days) is not int or not 0 <= retention_days <= 36500:
        parser.error("events.retention_days must be an integer from 0 to 36500")
    if os.name == "nt":
        parser.error("AgentDesk Host requires a Unix server")
    os.umask(0o077)
    args.home.mkdir(parents=True, exist_ok=True, mode=0o700)
    with InstanceLock(args.home / "run" / "host.lock"):
        asyncio.run(serve(args.home, args.fake or config.get("agents", {}).get("fake", False), idle_exit, retention_days))


if __name__ == "__main__":
    main()
