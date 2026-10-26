"""Official Claude Agent SDK bridge. Never handles account credentials."""

from __future__ import annotations

import asyncio
import importlib.metadata
import json
import os
from pathlib import Path
import uuid

from ..base import AdapterError, RunContext, RunHandle
from ..native_common import capability, command, version_of


class ClaudeAdapter:
    kind = "claude"

    def __init__(self):
        self._caps = None
        self._checked_at = 0.0

    @staticmethod
    def executable() -> str | None:
        try:
            import claude_agent_sdk
        except ImportError:
            return None
        override = os.environ.get("AGENTDESK_CLAUDE_PATH")
        path = Path(override) if override else Path(claude_agent_sdk.__file__).parent / "_bundled" / ("claude.exe" if os.name == "nt" else "claude")
        return str(path) if path.is_file() else None

    async def capabilities(self) -> dict:
        now = asyncio.get_running_loop().time()
        if self._caps and now - self._checked_at < 10:
            return dict(self._caps)
        executable = self.executable()
        result = capability(permissionModes=["default", "acceptEdits", "plan"])
        if executable:
            try:
                import claude_agent_sdk as sdk
                code, output = await command([executable, "--version"])
                result.update(installed=code == 0, version=version_of(output), sdkVersion=importlib.metadata.version("claude-agent-sdk"))
                if code != 0:
                    result["reason"] = "E_AGENT_EXECUTABLE"
                else:
                    auth_code, auth_output = await command([executable, "auth", "status"])
                    try:
                        auth = json.loads(auth_output)
                    except (ValueError, UnicodeError):
                        auth = {}
                    # Keep only public status fields; discard all other output.
                    logged_in = auth.get("loggedIn") is True and auth_code == 0
                    result.update(loggedIn=logged_in, accountType=auth.get("subscriptionType") if isinstance(auth.get("subscriptionType"), str) else None)
                    client = sdk.ClaudeSDKClient(options=sdk.ClaudeAgentOptions(cli_path=executable, stderr=lambda _: None))
                    try:
                        async with asyncio.timeout(25):
                            await client.connect()
                        info = await client.get_server_info() or {}
                        rows = info.get("supportedModels", info.get("models", []))
                        models = []
                        for entry in rows if isinstance(rows, list) else []:
                            if isinstance(entry, dict) and isinstance(entry.get("value", entry.get("id")), str):
                                identifier = entry.get("value", entry.get("id"))
                                models.append({"id": identifier, "name": entry.get("displayName", entry.get("name", identifier))})
                        result.update(models=models, handshake=True, streaming=True, resume=hasattr(sdk, "get_session_info"), approvals=True, stop=hasattr(client, "interrupt"), appendInput=hasattr(client, "query"))
                    finally:
                        await client.disconnect()
                    result.update(available=logged_in, reason=None if logged_in else "E_NOT_LOGGED_IN")
            except Exception:
                # Never serialize native exceptions: they may embed auth output.
                result.update(available=False, reason="E_AGENT_PROBE", handshake=False)
        self._caps, self._checked_at = result, now
        return dict(result)

    async def native_exists(self, session: dict) -> bool:
        if not session.get("nativeId"):
            return True
        import claude_agent_sdk as sdk
        return await asyncio.to_thread(sdk.get_session_info, session["nativeId"], session.get("path")) is not None

    async def start_run(self, session: dict, input: dict, ctx: RunContext) -> RunHandle:
        caps = await self.capabilities()
        if not caps["installed"]:
            raise AdapterError("E_AGENT_NOT_INSTALLED", "claude.preflight", "Claude SDK CLI is unavailable", "Install the Claude SDK and its bundled CLI")
        if not caps["loggedIn"]:
            raise AdapterError("E_NOT_LOGGED_IN", "claude.auth", "Claude is not logged in", "Sign in from the environment page")
        if not await self.native_exists(session):
            raise AdapterError("E_NATIVE_MISSING", "claude.resume", "Native session record is missing", "Restore the native record or create a new session")
        mode = session.get("permissionMode", "default")
        if mode not in caps["permissionModes"]:
            raise AdapterError("E_CAPABILITY", "claude.permissions", "Unsupported permission mode", "Select a supported permission mode")
        native = {"client": None, "stopping": False}
        task = asyncio.create_task(self._run(session, input, ctx, native))
        return RunHandle(ctx.run_id, task, native)

    async def _run(self, session: dict, input: dict, ctx: RunContext, native: dict):
        import claude_agent_sdk as sdk
        from claude_agent_sdk.types import HookMatcher, PermissionResultAllow, PermissionResultDeny

        async def approve(tool: str, data: dict, permission_context):
            payload = {"tool": tool, "input": data}
            for key in ("title", "description", "decision_reason", "blocked_path"):
                value = getattr(permission_context, key, None)
                if value:
                    payload[key] = value
            question = tool == "AskUserQuestion"
            if question:
                payload["questions"] = data.get("questions", [])
            decision = await ctx.approve("question" if question else "tool", payload)
            if decision.get("behavior") != "allow":
                return PermissionResultDeny(message="Request declined by the user", interrupt=False)
            updated = dict(data)
            if question:
                updated["answers"] = decision.get("answers", {})
            return PermissionResultAllow(updated_input=updated)

        async def keep_stream_open(*_):
            return {"continue_": True}

        native_id = session.get("nativeId") or str(uuid.uuid4())
        options = sdk.ClaudeAgentOptions(
            cwd=session["path"], cli_path=self.executable(), model=session.get("model") or None,
            permission_mode=session.get("permissionMode", "default"), can_use_tool=approve,
            include_partial_messages=True, hooks={"PreToolUse": [HookMatcher(hooks=[keep_stream_open])]},
            resume=native_id if session.get("nativeId") else None,
            session_id=None if session.get("nativeId") else native_id,
            stderr=lambda _: None,
        )
        client = sdk.ClaudeSDKClient(options=options)
        native["client"] = client
        # Stream blocks are keyed by native message id + index. Completed SDK
        # blocks replace the same item, rather than duplicating streamed text.
        streamed: dict[int, tuple[str, str, str]] = {}
        file_changes: dict[str, dict] = {}
        completed_messages: set[str] = set()
        message_id = ""
        try:
            async with asyncio.timeout(30):
                await client.connect()
            await client.query(input["text"], session_id=native_id)
            async for message in client.receive_response():
                kind = type(message).__name__
                if kind == "SystemMessage":
                    if message.subtype == "init" and message.data.get("session_id"):
                        await ctx.save_native(message.data["session_id"])
                elif kind == "StreamEvent":
                    event = message.event
                    event_type = event.get("type")
                    if event_type == "message_start":
                        message_id = event.get("message", {}).get("id") or uuid.uuid4().hex
                        streamed = {}
                    elif event_type == "content_block_start":
                        block = event.get("content_block", {})
                        index = event.get("index", 0)
                        item_id = block.get("id") or message_id + ":" + str(index)
                        block_kind = {"text": "text", "thinking": "reasoning", "tool_use": "tool_call"}.get(block.get("type"))
                        if block_kind:
                            text = block.get("text", block.get("thinking", ""))
                            streamed[index] = (item_id, block_kind, text)
                            await ctx.emit("item.started", {"kind": block_kind, "text": text, "name": block.get("name"), "input": block.get("input", {})}, item_id)
                    elif event_type == "content_block_delta":
                        index = event.get("index", 0)
                        if index in streamed:
                            item_id, block_kind, text = streamed[index]
                            delta = event.get("delta", {})
                            value = delta.get("text", delta.get("thinking", ""))
                            if value:
                                streamed[index] = (item_id, block_kind, text + value)
                                await ctx.emit("item.delta", {"kind": block_kind, "delta": value}, item_id)
                elif kind == "AssistantMessage":
                    mid = getattr(message, "message_id", None) or message_id or uuid.uuid4().hex
                    if mid in completed_messages:
                        continue
                    completed_messages.add(mid)
                    for index, block in enumerate(message.content):
                        name = type(block).__name__
                        item_id = streamed.get(index, (mid + ":" + str(index), "", ""))[0]
                        if name == "TextBlock":
                            payload = {"kind": "text", "text": block.text}
                        elif name == "ThinkingBlock":
                            payload = {"kind": "reasoning", "text": block.thinking}
                        elif name == "ToolUseBlock":
                            item_id = block.id
                            payload = {"kind": "tool_call", "name": block.name, "input": block.input}
                            if block.name in ("Edit", "Write", "MultiEdit"):
                                file_changes[block.id] = {"kind": "file_change", "name": block.name, "input": block.input, "scope": "server"}
                        elif name == "ToolResultBlock":
                            payload = {"kind": "tool_result", "output": block.content, "isError": block.is_error, "toolCallId": block.tool_use_id}
                            if not block.is_error and block.tool_use_id in file_changes:
                                await ctx.emit("item.completed", file_changes.pop(block.tool_use_id), block.tool_use_id + ":file")
                        else:
                            continue
                        await ctx.emit("item.completed", payload, item_id)
                elif kind == "UserMessage":
                    for block in message.content if isinstance(message.content, list) else []:
                        if type(block).__name__ == "ToolResultBlock":
                            await ctx.emit("item.completed", {"kind": "tool_result", "toolCallId": block.tool_use_id, "output": block.content, "isError": block.is_error}, block.tool_use_id + ":result")
                            if not block.is_error and block.tool_use_id in file_changes:
                                await ctx.emit("item.completed", file_changes.pop(block.tool_use_id), block.tool_use_id + ":file")
                elif kind in ("TaskStartedMessage", "TaskProgressMessage", "TaskNotificationMessage"):
                    await ctx.emit("run.state", {"state": "awaiting_background", "taskId": getattr(message, "task_id", None), "status": getattr(message, "status", None)})
                elif kind == "ResultMessage":
                    if message.session_id:
                        await ctx.save_native(message.session_id)
                    if message.usage:
                        await ctx.emit("usage", {"tokens": message.usage})
                    if message.is_error and not native["stopping"]:
                        raise AdapterError("E_AGENT", "claude.run", "Claude run failed", "Review the native CLI state and retry")
        finally:
            await client.disconnect()
            native["client"] = None
        if native["stopping"]:
            raise asyncio.CancelledError()

    async def append_input(self, run: RunHandle, input):
        client = run.native.get("client")
        if client is None:
            raise AdapterError("E_SESSION_BUSY", "claude.append", "Run is not ready", "Wait for the run to start")
        await client.query(input["text"] if isinstance(input, dict) else input)

    async def stop(self, run: RunHandle):
        run.native["stopping"] = True
        client = run.native.get("client")
        if client is not None:
            try:
                await asyncio.wait_for(client.interrupt(), 5)
                await asyncio.wait_for(asyncio.shield(run.task), 5)
            except asyncio.CancelledError:
                if not run.task.done():
                    raise
            except Exception:
                run.task.cancel()
        else:
            run.task.cancel()

    async def respond_approval(self, run, approval_id, decision):
        # RunContext owns arbitration and resolves the callback's future.
        return None

    async def delete_session(self, session):
        if session.get("nativeId") and await self.native_exists(session):
            raise AdapterError("E_CAPABILITY", "claude.delete", "This SDK has no native session deletion API", "Delete the native record with the official CLI before removing its mirror")

    async def login_command(self):
        executable = self.executable()
        if not executable:
            raise AdapterError("E_AGENT_NOT_INSTALLED", "claude.login", "Claude CLI is unavailable", "Install the Host SDK first")
        return [executable, "auth", "login"]

    async def logout(self):
        args = await self.login_command()
        code, _ = await command([args[0], "auth", "logout"])
        if code:
            raise AdapterError("E_AGENT", "claude.logout", "Official logout command failed", "Check the CLI state using the native terminal")
        self._caps = None
