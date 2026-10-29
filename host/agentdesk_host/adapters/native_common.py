"""Bounded native process helpers; authentication output never leaves memory."""

from __future__ import annotations

import asyncio
import functools
import os
from pathlib import Path
import re
import signal
import shlex
import shutil
import tempfile


MAX_FRAME = 4 * 1024 * 1024
SSH_CONNECTION_VARIABLES = ('SSH_CONNECTION', 'SSH_CLIENT', 'SSH_TTY', 'SSH_ORIGINAL_COMMAND')


def native_environment() -> dict[str, str]:
    """Exclude transport metadata while retaining server auth and toolchain settings."""
    return {key: value for key, value in os.environ.items() if key not in SSH_CONNECTION_VARIABLES}


def native_command(arguments: list[str]) -> str:
    """Filter at exec time, including when an SDK merges the parent environment."""
    unset = [part for key in SSH_CONNECTION_VARIABLES for part in ('-u', key)]
    return shlex.join(['env', *unset, '--', *arguments])


def cached_capabilities(probe):
    """Coalesce native detection and keep its public result for ten minutes."""
    @functools.wraps(probe)
    async def cached(self):
        if self._caps and asyncio.get_running_loop().time() - self._checked_at < 600:
            return dict(self._caps)
        task = getattr(self, "_capability_task", None)
        if task is None or task.done():
            task = asyncio.create_task(probe(self))
            self._capability_task = task
        return dict(await asyncio.shield(task))
    return cached


class ClaudeProcessGroup:
    """Wrap the unmodified SDK CLI in setsid and retain its group for cleanup."""
    def __init__(self, executable: str):
        self.executable = executable
        self.directory = None
        self.pid_path = None

    def prepare(self) -> str:
        if os.name != "posix":
            return self.executable
        setsid = shutil.which("setsid")
        if not setsid:
            from .base import AdapterError
            raise AdapterError("E_CAPABILITY", "claude.process", "setsid is required for native tool process isolation")
        self.directory = tempfile.TemporaryDirectory(prefix="agentdesk-claude-")
        root = Path(self.directory.name)
        self.pid_path = root / "group.pid"
        wrapper = root / "claude"
        wrapper.write_text("#!/bin/sh\nprintf '%s\\n' \"$$\" > " + shlex.quote(str(self.pid_path)) + "\nexec " + native_command([setsid, "--", self.executable]) + ' "$@"\n', encoding="utf-8")
        wrapper.chmod(0o700)
        return str(wrapper)

    async def close(self) -> None:
        try:
            if self.pid_path and self.pid_path.is_file():
                pid = int(self.pid_path.read_text(encoding="ascii").strip())
                if pid > 1:
                    for sig in (signal.SIGTERM, signal.SIGKILL):
                        try:
                            os.killpg(pid, sig)
                        except ProcessLookupError:
                            break
                        if sig == signal.SIGTERM:
                            await asyncio.sleep(0.05)
        finally:
            if self.directory:
                self.directory.cleanup()


async def close_process(process: asyncio.subprocess.Process) -> None:
    if os.name == "posix":
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    elif process.returncode is None:
        try:
            process.terminate()
        except ProcessLookupError:
            pass
    if process.returncode is None:
        try:
            await asyncio.wait_for(process.wait(), 5)
        except TimeoutError:
            try:
                if os.name == "posix":
                    os.killpg(process.pid, signal.SIGKILL)
                else:
                    process.kill()
            except ProcessLookupError:
                pass
            await asyncio.wait_for(process.wait(), 5)
    if os.name == "posix":
        # The leader can exit before a tool process; its descendants still own
        # the reserved process-group id and must not survive transport cleanup.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass


async def command(arguments: list[str], timeout: float = 20) -> tuple[int, bytes]:
    """Drain both streams with a hard bound, never forward raw CLI diagnostics."""
    process = await asyncio.create_subprocess_exec(
        *arguments, stdin=asyncio.subprocess.DEVNULL,
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL,
        env=native_environment(),
        start_new_session=os.name == "posix", limit=MAX_FRAME,
    )
    assert process.stdout is not None

    async def read():
        output = bytearray()
        while chunk := await process.stdout.read(65536):
            output.extend(chunk)
            if len(output) > MAX_FRAME:
                raise ValueError("Native command response exceeded the frame limit")
        return await process.wait(), bytes(output)

    try:
        return await asyncio.wait_for(read(), timeout)
    finally:
        await close_process(process)


def version_of(output: bytes) -> str | None:
    match = re.search(rb"\b\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?\b", output)
    return match.group().decode("ascii") if match else None


def capability(**values) -> dict:
    return {
        "installed": False, "available": False, "loggedIn": False,
        "version": None, "reason": "E_AGENT_NOT_INSTALLED", "models": [],
        "permissionModes": ["default", "plan"], "streaming": False,
        "resume": False, "approvals": False, "stop": False, "appendInput": False,
        "authStatus": "missing",
        **values,
    }


def installed_codex() -> str | None:
    import shutil

    override = os.environ.get("AGENTDESK_CODEX_PATH")
    if override:
        return override if Path(override).is_file() else None
    managed = Path.home() / ".agentdesk/host/current/bin/codex"
    return str(managed) if managed.is_file() else shutil.which("codex")
