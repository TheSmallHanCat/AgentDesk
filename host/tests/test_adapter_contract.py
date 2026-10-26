"""The same lifecycle contract runs on Fake in CI and on authenticated native CLIs."""
import asyncio
import os

import pytest

from agentdesk_host.adapters.base import RunContext
from agentdesk_host.adapters.fake import FakeAdapter
from agentdesk_host.adapters.claude import ClaudeAdapter
from agentdesk_host.adapters.codex import CodexAdapter


@pytest.mark.parametrize("kind,factory", [("fake", FakeAdapter), ("claude", ClaudeAdapter), ("codex", CodexAdapter)])
def test_adapter_contract(kind, factory, tmp_path):
    if kind != "fake" and os.environ.get("AGENTDESK_NATIVE_CONTRACT") != "1":
        pytest.skip("Real Agent contract requires explicit AGENTDESK_NATIVE_CONTRACT=1 after user login")
    async def check():
        adapter = factory()
        caps = await adapter.capabilities()
        if not caps.get("installed"):
            pytest.skip(f"{kind}: CLI is not installed")
        if not caps.get("loggedIn"):
            pytest.skip(f"{kind}: CLI is not logged in; user login is required")
        if not caps.get("models"):
            pytest.skip(f"{kind}: native model list is unavailable")
        events, native_ids = [], []
        async def emit(event_type, payload, item_id=None): events.append((event_type, payload, item_id))
        async def approve(kind, payload): return {"behavior": "deny"}
        async def save_native(value): native_ids.append(value)
        model = caps["models"][0]
        session = {"id": "contract", "path": str(tmp_path), "model": model["id"] if isinstance(model, dict) else model, "permissionMode": caps["permissionModes"][0]}
        input = {"text": "Reply with the single word ready. Do not use any tools."}
        if kind == "fake":
            input["script"] = [{"type": "reasoning", "text": "Checking"}, {"type": "text", "text": "ready", "chunks": 3}, {"type": "tool", "name": "Read", "output": "content"}, {"type": "file_change", "text": "diff"}, {"type": "approval"}, {"type": "question"}, {"type": "background", "seconds": 0.001}, {"type": "usage", "usage": {"inputTokens": 5}}]
        handle = await adapter.start_run(session, input, RunContext("run", emit, approve, save_native))
        await asyncio.wait_for(handle.task, 60)
        assert native_ids
        assert any(event_type == "item.completed" for event_type, _, _ in events)
        assert not any(event_type in {"run.started", "run.finished"} for event_type, _, _ in events)
        assert await adapter.native_exists({**session, "nativeId": native_ids[-1]})
        if kind == "fake":
            kinds = {payload.get("kind") for _, payload, _ in events}
            assert {"text", "reasoning", "tool_call", "tool_result", "file_change"} <= kinds
    asyncio.run(check())
