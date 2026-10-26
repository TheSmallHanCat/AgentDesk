"""Bounded native process helpers; authentication output never leaves memory."""

from __future__ import annotations

import asyncio
import os
from pathlib import Path
import re
import signal


MAX_FRAME = 4 * 1024 * 1024


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
        **values,
    }


def installed_codex() -> str | None:
    import shutil

    override = os.environ.get("AGENTDESK_CODEX_PATH")
    if override:
        return override if Path(override).is_file() else None
    managed = Path.home() / ".agentdesk/host/current/bin/codex"
    return str(managed) if managed.is_file() else shutil.which("codex")
