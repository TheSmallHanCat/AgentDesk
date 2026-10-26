"""Codex app-server stdio transport, isolated from Host's own JSON-RPC."""

from __future__ import annotations

import asyncio
import json
import os
from collections.abc import Awaitable, Callable

from .base import AdapterError
from .native_common import MAX_FRAME, close_process


class NativeRpc:
    def __init__(self, executable: str, config: list[str] | None = None):
        self.executable = executable
        self.config = config or []
        self.process = None
        self.reader_task = None
        self.pending: dict[int, asyncio.Future] = {}
        self.notifications: asyncio.Queue = asyncio.Queue(256)
        self.request_handler: Callable[[str, dict], Awaitable[dict]] | None = None
        self.handlers: set[asyncio.Task] = set()
        self.counter = 0
        self.write_lock = asyncio.Lock()

    async def __aenter__(self):
        self.process = await asyncio.create_subprocess_exec(
            *([self.executable] if isinstance(self.executable, str) else self.executable), *self.config, "app-server", "--listen", "stdio://",
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL, start_new_session=os.name == "posix",
            limit=MAX_FRAME,
        )
        self.reader_task = asyncio.create_task(self._read())
        try:
            await self.request("initialize", {"clientInfo": {"name": "agentdesk", "title": "AgentDesk", "version": "0.2.0"}})
            await self.send({"method": "initialized", "params": {}})
            return self
        except BaseException:
            await self.close()
            raise

    async def __aexit__(self, *args):
        await self.close()

    async def send(self, value: dict):
        data = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode() + b"\n"
        if len(data) > MAX_FRAME:
            raise AdapterError("E_INPUT", "codex.rpc", "Request exceeds 4 MiB", "Reduce the input size")
        async with self.write_lock:
            if self.process is None or self.process.stdin is None or self.process.returncode is not None:
                raise AdapterError("E_AGENT_EXITED", "codex.rpc", "App server is disconnected", "Start a new run")
            self.process.stdin.write(data)
            await self.process.stdin.drain()

    async def request(self, method: str, params: dict, timeout: float = 30) -> dict:
        self.counter += 1
        identifier = self.counter
        future = asyncio.get_running_loop().create_future()
        self.pending[identifier] = future
        try:
            await self.send({"id": identifier, "method": method, "params": params})
            return await asyncio.wait_for(future, timeout)
        except TimeoutError:
            raise AdapterError("E_TIMEOUT", "codex." + method, "App server request timed out", "Check the remote CLI and retry") from None
        finally:
            self.pending.pop(identifier, None)

    async def _server_request(self, message):
        try:
            if self.request_handler is None:
                raise AdapterError("E_CAPABILITY", "codex.request", "Unsupported server request", "Use a supported CLI version")
            value = await self.request_handler(message["method"], message.get("params", {}))
            await self.send({"id": message["id"], "result": value})
        except asyncio.CancelledError:
            raise
        except Exception:
            await self.send({"id": message["id"], "error": {"code": -32601, "message": "Unsupported or cancelled request"}})

    async def _read(self):
        assert self.process and self.process.stdout
        try:
            while line := await self.process.stdout.readline():
                if len(line) > MAX_FRAME:
                    raise ValueError("Native frame exceeded 4 MiB")
                message = json.loads(line)
                if not isinstance(message, dict):
                    raise ValueError("Expected JSON-RPC object")
                if "method" in message and "id" in message:
                    if len(self.handlers) >= 64:
                        raise ValueError("Too many pending native requests")
                    task = asyncio.create_task(self._server_request(message))
                    self.handlers.add(task)
                    def completed(task):
                        self.handlers.discard(task)
                        if not task.cancelled():
                            task.exception()
                    task.add_done_callback(completed)
                elif "method" in message:
                    await self.notifications.put(message)
                elif (future := self.pending.get(message.get("id"))) is not None and not future.done():
                    if "error" in message:
                        # The native message can contain account or tool data.
                        native_error = message.get("error", {})
                        text = str(native_error.get("message", "")).lower()
                        missing = any(marker in text for marker in ("not found", "no rollout", "does not exist", "failed to load session"))
                        future.set_exception(AdapterError("E_NATIVE_MISSING" if missing else "E_AGENT", "codex.rpc", "Native request was rejected", "Check CLI compatibility and session state"))
                    else:
                        future.set_result(message.get("result", {}))
        except asyncio.CancelledError:
            raise
        except (OSError, ValueError, ConnectionError):
            pass
        finally:
            for future in self.pending.values():
                if not future.done():
                    future.set_exception(AdapterError("E_AGENT_EXITED", "codex.rpc", "App server output closed", "Check the CLI executable and resources"))
            # Runs waiting for a terminal turn notification must also terminate.
            if self.notifications.full():
                self.notifications.get_nowait()
            self.notifications.put_nowait({"method": "_closed", "params": {}})

    async def close(self):
        for task in list(self.handlers):
            task.cancel()
        if self.handlers:
            await asyncio.gather(*self.handlers, return_exceptions=True)
        if self.reader_task:
            self.reader_task.cancel()
            await asyncio.gather(self.reader_task, return_exceptions=True)
        if self.process:
            if self.process.stdin:
                self.process.stdin.close()
            await close_process(self.process)
