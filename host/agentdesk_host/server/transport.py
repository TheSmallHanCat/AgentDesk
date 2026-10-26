"""Bounded NDJSON JSON-RPC over the private Unix socket."""
from __future__ import annotations

import asyncio
import json
import logging
import os
from pathlib import Path
from typing import Any

from ..adapters.base import AdapterError
from .service import Client, HostService, MAX_FRAME, encoded

logger = logging.getLogger(__name__)
DRAIN_TIMEOUT = 30


def error_reply(request_id: Any, code: int, message: str, data: dict[str, Any] | None = None) -> dict[str, Any]:
    return {"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message, **({"data": data} if data else {})}}


class HostServer:
    def __init__(self, service: HostService, socket_path: Path | None = None):
        self.service = service
        self.socket_path = socket_path or service.home / "run" / "host.sock"
        self.server: asyncio.Server | None = None
        self.connections: set[asyncio.Task] = set()
        self.writers: set[asyncio.StreamWriter] = set()

    async def start(self) -> None:
        self.socket_path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        if self.socket_path.exists() or self.socket_path.is_symlink():
            if self.socket_path.is_symlink() or not self.socket_path.is_socket():
                raise RuntimeError("Refusing to replace a non-socket Host path")
            self.socket_path.unlink()
        previous_umask = os.umask(0o177)
        try:
            self.server = await asyncio.start_unix_server(self._accept, path=self.socket_path, limit=MAX_FRAME)
            os.chmod(self.socket_path, 0o600)
        finally:
            os.umask(previous_umask)
        self.service.start_detection()

    async def _accept(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        task = asyncio.current_task()
        self.connections.add(task)
        self.writers.add(writer)
        queue: asyncio.Queue[bytes] = asyncio.Queue(maxsize=256)

        async def send(message: dict[str, Any]) -> None:
            frame = encoded(message) + b"\n"
            if len(frame) > MAX_FRAME:
                raise ConnectionError("Host frame exceeds the protocol limit")
            try:
                queue.put_nowait(frame)
            except asyncio.QueueFull:
                writer.close()
                raise ConnectionError("Slow subscriber disconnected") from None

        async def write() -> None:
            try:
                while True:
                    writer.write(await queue.get())
                    try:
                        await asyncio.wait_for(writer.drain(), timeout=DRAIN_TIMEOUT)
                    finally:
                        queue.task_done()
            except (TimeoutError, ConnectionError, OSError) as error:
                logger.warning("Host transport closed: %s", "write_timeout" if isinstance(error, TimeoutError) else "write_failed")
                writer.close()
                # A blocked reader must also exit when only the write side failed.
                if task and not task.done():
                    task.cancel()

        client = Client(send)
        write_task = asyncio.create_task(write())
        try:
            while True:
                try:
                    line = await reader.readline()
                except (ValueError, asyncio.LimitOverrunError):
                    await send(error_reply(None, -32600, "Frame exceeds 4 MiB", {"code": "E_FRAME_TOO_LARGE", "stage": "transport", "hint": "Reduce the request size"}))
                    break
                if not line:
                    break
                if len(line) > MAX_FRAME:
                    break
                if not line.endswith(b"\n"):
                    break
                try:
                    request = json.loads(line)
                except (UnicodeDecodeError, json.JSONDecodeError):
                    await send(error_reply(None, -32700, "Invalid JSON"))
                    continue
                if isinstance(request, dict) and request.get("jsonrpc") == "2.0" and "method" not in request and ("result" in request or "error" in request):
                    await self.service.handle_response(client, request)
                    continue
                if not isinstance(request, dict) or request.get("jsonrpc") != "2.0" or not isinstance(request.get("method"), str) or not isinstance(request.get("params", {}), dict) or type(request.get("id")) not in {int, str}:
                    await send(error_reply(None, -32600, "Invalid JSON-RPC request"))
                    continue
                request_id = request["id"]
                try:
                    result = await self.service.request(client, request["method"], request.get("params", {}))
                    await send({"jsonrpc": "2.0", "id": request_id, "result": result})
                except AdapterError as error:
                    data = {"code": error.code, "stage": error.stage, "hint": error.hint}
                    if hasattr(error, "decision"):
                        data["decision"] = error.decision
                    await send(error_reply(request_id, -32000, str(error), data))
                except Exception:
                    await send(error_reply(request_id, -32603, "Host operation failed", {"code": "E_INTERNAL", "stage": "host", "hint": "Check the Host environment"}))
        except (ConnectionError, BrokenPipeError, asyncio.CancelledError):
            pass
        finally:
            await self.service.disconnect(client)
            try:
                await asyncio.wait_for(queue.join(), timeout=1)
            except (TimeoutError, ConnectionError):
                pass
            write_task.cancel()
            await asyncio.gather(write_task, return_exceptions=True)
            writer.close()
            try:
                await writer.wait_closed()
            except ConnectionError:
                pass
            self.connections.discard(task)
            self.writers.discard(writer)

    async def close(self) -> None:
        if self.server:
            self.server.close()
        for writer in tuple(self.writers):
            writer.close()
        for task in tuple(self.connections):
            task.cancel()
        if self.connections:
            await asyncio.gather(*tuple(self.connections), return_exceptions=True)
        if self.server:
            # Python 3.12 waits for active transports as well as the listener.
            # Close/cancel clients before awaiting this barrier.
            await self.server.wait_closed()
        if self.socket_path.is_socket():
            self.socket_path.unlink()
