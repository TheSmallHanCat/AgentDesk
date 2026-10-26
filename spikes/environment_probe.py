"""Read-only, endpoint-free runtime, policy and experiment cleanup summary."""

from __future__ import annotations

import itertools
import json
import os
from pathlib import Path
import platform
import sys
import xml.etree.ElementTree as ET


def main() -> None:
    home = Path("/home/agentdesk-test")
    result: dict = {
        "pythonVersion": platform.python_version(),
        "pythonBuild": sys.version,
        "homeEntries": sorted(path.name for path in home.iterdir()),
        "installDirectories": sorted(str(path) for path in (home / ".agentdesk/spikes").iterdir() if path.is_dir()),
        "workDirectories": sorted(str(path) for path in (home / "agentdesk-e2e").glob("spikes-*") if path.is_dir()),
        "runtimeDirectoryExists": (Path("/run/user") / str(os.getuid())).is_dir(),
        "userBusExists": (Path("/run/user") / str(os.getuid()) / "bus").is_socket(),
    }
    policy = Path("/usr/share/polkit-1/actions/org.freedesktop.login1.policy")
    result["lingerPolicy"] = {"available": policy.is_file()}
    if policy.is_file():
        try:
            for action in ET.parse(policy).getroot().findall("action"):
                if action.get("id") == "org.freedesktop.login1.set-user-linger":
                    result["lingerPolicy"].update({
                        name: action.findtext("defaults/" + name)
                        for name in ("allow_any", "allow_inactive", "allow_active")
                    })
        except (OSError, ET.ParseError):
            result["lingerPolicy"]["readable"] = False
    active = 0
    for process in itertools.islice(Path("/proc").glob("[0-9]*"), 10000):
        if process.name == str(os.getpid()):
            continue
        try:
            state = (process / "status").read_text()
            uid = next(line for line in state.splitlines() if line.startswith("Uid:")).split()[1]
            if int(uid) != os.getuid() or "State:\tZ" in state:
                continue
            # Command lines are used only for resource ownership checks; no raw
            # process data or arguments appear in the summary.
            command = (process / "cmdline").read_bytes()
            if b"socket_service.py" in command or (b"/.agentdesk/spikes/" in command and b"/codex\x00" in command):
                active += 1
        except (OSError, StopIteration, ValueError):
            continue
    result["activeSpikeWorkers"] = active
    sockets = []
    for directory in (home / "agentdesk-e2e").glob("spikes-*"):
        for kind in ("detached", "managed"):
            for name in ("host.sock", "host.pid"):
                path = directory / kind / name
                if path.exists():
                    sockets.append(str(path))
    result["remainingServiceFiles"] = sorted(sockets)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
