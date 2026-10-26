"""Deterministic scripted adapter for development and contract tests."""
from __future__ import annotations

import asyncio
import uuid
from typing import Any

from .base import AdapterError, RunContext, RunHandle


class FakeAdapter:
    async def capabilities(self) -> dict[str, Any]:
        return {
            "installed": True, "available": True, "loggedIn": True,
            "version": "1.0", "models": [{"id": "fake-default", "name": "Scripted Agent"}],
            "permissionModes": ["default", "plan"], "streaming": True, "resume": True,
            "approvals": True, "stop": True, "appendInput": True,
        }

    async def native_exists(self, session: dict[str, Any]) -> bool:
        return True

    async def delete_session(self, session: dict[str, Any]) -> None:
        return None

    async def start_run(self, session: dict[str, Any], input: dict[str, Any], context: RunContext) -> RunHandle:
        steps = input.get("script")
        if steps is None:
            steps = [
                {"type": "text", "text": "正在处理请求。", "chunks": 4},
                {"type": "reasoning", "text": "检查项目状态并准备响应。"},
                {"type": "tool", "name": "Read", "input": {"path": "README.md"}, "output": "Project documentation"},
                {"type": "approval", "tool": "Write", "input": {"path": "result.txt"}},
                {"type": "text", "text": "请求已完成。", "chunks": 3},
            ]
        if not isinstance(steps, list) or len(steps) > 1000 or any(not isinstance(step, dict) for step in steps):
            raise AdapterError("E_INVALID_PARAMS", "adapter.start", "Invalid scripted run", "Provide an array of at most 1000 steps")
        await context.save_native(session.get("nativeId") or f"fake_{uuid.uuid4().hex}")
        queue: asyncio.Queue[str] = asyncio.Queue()
        task = asyncio.create_task(self._run(steps, context, queue))
        return RunHandle(context.run_id, task, {"queue": queue, "context": context})

    async def _item(self, context: RunContext, kind: str, text: str, chunks: int = 1, delay: float = 0.005, **extra: Any) -> None:
        item_id = f"itm_{uuid.uuid4().hex}"
        await context.emit("item.started", {"kind": kind, **extra}, item_id)
        size = max(1, (len(text) + chunks - 1) // chunks)
        for offset in range(0, len(text), size):
            await context.emit("item.delta", {"kind": kind, "delta": text[offset:offset + size]}, item_id)
            await asyncio.sleep(delay)
        await context.emit("item.completed", {"kind": kind, "text": text, **extra}, item_id)

    async def _run(self, steps: list[dict[str, Any]], context: RunContext, queue: asyncio.Queue[str]) -> None:
        for step in steps:
            kind = step.get("type", "text")
            if kind in {"text", "reasoning", "file_change", "plan"}:
                await self._item(context, kind, str(step.get("text", "")), min(1000, max(1, int(step.get("chunks", 3)))), min(10, max(0, float(step.get("delay", 0.005)))), **step.get("extra", {}))
            elif kind == "long_output":
                size = min(8_000_000, max(1, int(step.get("size", 5_000_000))))
                pattern = str(step.get("text", "x")) or "x"
                text = (pattern * ((size + len(pattern) - 1) // len(pattern)))[:size]
                await self._item(context, "tool_result", text, 1, 0)
            elif kind == "tool":
                item_id = f"itm_{uuid.uuid4().hex}"
                await context.emit("item.completed", {"kind": "tool_call", "name": step.get("name", "Read"), "input": step.get("input", {})}, item_id)
                await context.emit("item.completed", {"kind": "tool_result", "name": step.get("name", "Read"), "output": step.get("output", "")}, f"itm_{uuid.uuid4().hex}")
            elif kind in {"approval", "question"}:
                payload = {key: value for key, value in step.items() if key != "type"}
                if kind == "question" and "questions" not in payload:
                    payload["questions"] = [{"question": "请选择处理方式", "header": "处理方式", "options": [{"label": "继续", "description": "执行下一步"}, {"label": "停止", "description": "结束当前操作"}], "multiSelect": False}]
                decision = await context.approve(kind, payload)
                await context.emit("item.completed", {"kind": "tool_result", "name": step.get("tool", "AskUserQuestion"), "output": decision}, f"itm_{uuid.uuid4().hex}")
            elif kind in {"sleep", "background"}:
                if kind == "background":
                    await context.emit("run.state", {"state": "awaiting_background"}, None)
                await asyncio.sleep(min(3600, max(0, float(step.get("seconds", 0.05)))))
                if kind == "background":
                    await context.emit("run.state", {"state": "running"}, None)
            elif kind == "wait_input":
                text = await asyncio.wait_for(queue.get(), timeout=min(3600, max(0.1, float(step.get("timeout", 60)))))
                await self._item(context, "text", text)
            elif kind == "error":
                raise AdapterError("E_AGENT", "adapter.run", str(step.get("message", "Scripted agent failed")), "Retry the run")
            elif kind == "usage":
                await context.emit("usage", step.get("usage", {}), None)
            else:
                raise AdapterError("E_INVALID_PARAMS", "adapter.run", f"Unknown scripted step: {kind}", "Use a supported script step")

    async def append_input(self, handle: RunHandle, text: str) -> None:
        handle.native["queue"].put_nowait(text)

    async def stop(self, handle: RunHandle) -> None:
        handle.task.cancel()

    async def respond_approval(self, handle: RunHandle, approval_id: str, decision: dict[str, Any]) -> None:
        return None
