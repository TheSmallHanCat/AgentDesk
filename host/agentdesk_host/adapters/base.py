"""Agent adapters translate native streams without depending on the Host service."""
from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
from typing import Any, Awaitable, Callable, Protocol


class AdapterError(Exception):
    def __init__(self, code: str, stage: str, message: str, hint: str = ""):
        super().__init__(message)
        self.code = code
        self.stage = stage
        self.hint = hint


Emit = Callable[[str, dict[str, Any], str | None], Awaitable[None]]
Approve = Callable[[str, dict[str, Any]], Awaitable[dict[str, Any]]]
SaveNative = Callable[[str], Awaitable[None]]


@dataclass
class RunContext:
    run_id: str
    emit: Emit
    approve: Approve
    save_native: SaveNative


@dataclass
class RunHandle:
    run_id: str
    task: asyncio.Task[None]
    native: dict[str, Any] = field(default_factory=dict)


class AgentAdapter(Protocol):
    async def login_command(self) -> list[str]: ...
    async def logout(self) -> None: ...
    async def capabilities(self) -> dict[str, Any]: ...
    async def start_run(self, session: dict[str, Any], input: dict[str, Any], context: RunContext) -> RunHandle: ...
    async def append_input(self, handle: RunHandle, text: str) -> None: ...
    async def stop(self, handle: RunHandle) -> None: ...
    async def respond_approval(self, handle: RunHandle, approval_id: str, decision: dict[str, Any]) -> None: ...
    async def native_exists(self, session: dict[str, Any]) -> bool: ...
    async def delete_session(self, session: dict[str, Any]) -> None: ...
