"""Official Claude Agent SDK bridge. Never handles account credentials."""

from __future__ import annotations

import asyncio
import importlib.metadata
import json
import os
from pathlib import Path
import re
import uuid

from ..base import AdapterError, RunContext, RunHandle
from ..native_common import ClaudeProcessGroup, cached_capabilities, capability, command, version_of
from ..login import LoginParser


class ClaudeLoginParser(LoginParser):
    def parse(self, text: str) -> dict:
        result = {"needsInput": bool(re.search(r"(?:paste|enter)[^\n]{0,50}(?:code|token)|请[^\n]{0,20}(?:粘贴|输入)[^\n]{0,20}(?:代码|授权码)", text, re.I))}
        if url := self.url(text):
            result["url"] = url
        if match := re.search(r"(?:device|user|one[- ]time)\s+code\s*[:：]\s*([A-Z0-9]{4,8}-[A-Z0-9]{4,8})", text, re.I):
            result["deviceCode"] = match.group(1)
        if state := self.terminal_state(text):
            result["state"] = state
        return result


class ClaudeAdapter:
    kind = "claude"
    name = "Claude Code"
    login_parser = ClaudeLoginParser

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

    @cached_capabilities
    async def capabilities(self) -> dict:
        now = asyncio.get_running_loop().time()
        if self._caps and now - self._checked_at < 600:
            return dict(self._caps)
        executable = self.executable()
        result = capability(permissionModes=["default", "acceptEdits", "plan"])
        if executable:
            result.update(installed=True, authStatus="failed")
            try:
                import claude_agent_sdk as sdk
                code, output = await command([executable, "--version"])
                result.update(installed=True, version=version_of(output), sdkVersion=importlib.metadata.version("claude-agent-sdk"), authStatus="failed")
                if code != 0:
                    result["reason"] = "E_AGENT_EXECUTABLE"
                else:
                    auth_code, auth_output = await command([executable, "auth", "status"])
                    try:
                        auth = json.loads(auth_output)
                    except (ValueError, UnicodeError):
                        raise ValueError("Invalid authentication status") from None
                    if not isinstance(auth, dict) or type(auth.get("loggedIn")) is not bool or auth_code not in (0, 1) or auth_code == 1 and auth["loggedIn"]:
                        raise ValueError("Invalid authentication status")
                    # Keep only public status fields; discard all other output.
                    logged_in = auth.get("loggedIn") is True and auth_code == 0
                    account_type = auth.get("subscriptionType") or auth.get("authMethod")
                    result.update(loggedIn=logged_in, authStatus="loggedIn" if logged_in else "loggedOut", accountType=account_type if logged_in and isinstance(account_type, str) else None)
                    process_group = ClaudeProcessGroup(executable)
                    client = sdk.ClaudeSDKClient(options=sdk.ClaudeAgentOptions(cli_path=process_group.prepare(), stderr=lambda _: None))
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
                        try:
                            await client.disconnect()
                        finally:
                            await process_group.close()
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
        if session.get("kind") == "local" and session.get("permissionMode") == "plan":
            raise AdapterError("E_LOCAL_PLAN_UNSUPPORTED", "claude.permissions", "Plan mode cannot enforce read-only device access")
        caps = session.get("_capabilities") or await self.capabilities()
        if not caps["installed"]:
            raise AdapterError("E_AGENT_NOT_INSTALLED", "claude.preflight", "Claude SDK CLI is unavailable", "Install the Claude SDK and its bundled CLI")
        if not caps["loggedIn"]:
            raise AdapterError("E_NOT_LOGGED_IN", "claude.auth", "Claude is not logged in", "Sign in from the environment page")
        if not session.get("_nativeExistsChecked") and not await self.native_exists(session):
            raise AdapterError("E_NATIVE_MISSING", "claude.resume", "Native session record is missing", "Restore the native record or create a new session")
        mode = session.get("permissionMode", "default")
        if mode not in caps["permissionModes"]:
            raise AdapterError("E_CAPABILITY", "claude.permissions", "Unsupported permission mode", "Select a supported permission mode")
        native = {"client": None, "stopping": False, "inputQueue": [], "inputLock": asyncio.Lock(), "sessionId": session.get("nativeId")}
        task = asyncio.create_task(self._run(session, input, ctx, native))
        return RunHandle(ctx.run_id, task, native)

    async def _run(self, session: dict, input: dict, ctx: RunContext, native: dict):
        if session.get("kind") == "local" and session.get("permissionMode") == "plan":
            raise AdapterError("E_LOCAL_PLAN_UNSUPPORTED", "claude.permissions", "Plan mode cannot enforce read-only device access")
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
        process_group = ClaudeProcessGroup(self.executable())
        options = sdk.ClaudeAgentOptions(
            cwd=session["path"], cli_path=process_group.prepare(), model=session.get("model") or None,
            permission_mode=session.get("permissionMode", "default"), can_use_tool=approve,
            include_partial_messages=True, hooks={"PreToolUse": [HookMatcher(hooks=[keep_stream_open])]},
            resume=native_id if session.get("nativeId") else None,
            session_id=None if session.get("nativeId") else native_id,
            stderr=lambda _: None,
            system_prompt={"type": "preset", "preset": "claude_code", "append": session["deviceInstructions"]} if session.get("deviceInstructions") else None,
        )
        client = sdk.ClaudeSDKClient(options=options)
        native["client"] = client
        # Stream blocks are keyed by native message id + index. Completed SDK
        # blocks replace the same item, rather than duplicating streamed text.
        streamed: dict[int, tuple[str, str, str]] = {}
        message_streams: dict[str, dict[int, tuple[str, str, str]]] = {}
        active_indices: dict[str, int] = {}
        file_changes: dict[str, dict] = {}
        completed_blocks: set[tuple[str, str]] = set()
        completed_indices: set[tuple[str, int]] = set()
        fallback_blocks: dict[tuple[str, str, str], str] = {}
        message_id = ""
        native["sessionId"] = native_id
        native.setdefault("inputQueue", [])
        native.setdefault("inputLock", asyncio.Lock())
        try:
            async with asyncio.timeout(30):
                await client.connect()
            if native["stopping"]:
                raise asyncio.CancelledError()
            await client.query(input["text"], session_id=native_id)
            native["acceptingInput"] = True
            async for message in client.receive_messages():
                kind = type(message).__name__
                if kind == "SystemMessage":
                    if message.subtype == "init" and message.data.get("session_id"):
                        await ctx.save_native(message.data["session_id"])
                elif kind == "StreamEvent":
                    event = message.event
                    event_type = event.get("type")
                    if event_type == "message_start":
                        message_id = event.get("message", {}).get("id") or uuid.uuid4().hex
                        streamed = message_streams.setdefault(message_id, {})
                    elif event_type == "content_block_start":
                        block = event.get("content_block", {})
                        index = event.get("index", 0)
                        active_indices[message_id] = index
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
                    for index, block in enumerate(message.content):
                        name = type(block).__name__
                        block_kind = {"TextBlock": "text", "ThinkingBlock": "reasoning", "ToolUseBlock": "tool_call", "ToolResultBlock": "tool_result"}.get(name)
                        block_id = getattr(block, "id", None) or (str(block.tool_use_id) + ":result" if name == "ToolResultBlock" else None)
                        # Split AssistantMessages restart enumerate at zero. Match
                        # a native block id first, then the next compatible stream
                        # index for this message, never the message id alone.
                        candidates = [(i, entry) for i, entry in message_streams.get(mid, {}).items() if entry[1] == block_kind]
                        match = next(((i, entry) for i, entry in candidates if block_id and entry[0] == block_id), None)
                        if match is None and not block_id:
                            preferred = index if len(message.content) > 1 else active_indices.get(mid)
                            match = next(((i, entry) for i, entry in candidates if i == preferred), None)
                        if match is None:
                            match = next(((i, entry) for i, entry in candidates if (mid, i) not in completed_indices), None)
                        if match is None and not block_id:
                            text = getattr(block, "text", getattr(block, "thinking", None))
                            match = next(((i, entry) for i, entry in candidates if entry[2] == text and (mid, i) in completed_indices), None)
                        if match:
                            stream_index, entry = match
                            item_id = entry[0]
                            completed_indices.add((mid, stream_index))
                        elif block_id:
                            item_id = block_id
                        else:
                            value = getattr(block, "text", getattr(block, "thinking", repr(getattr(block, "content", ""))))
                            signature = (mid, name, str(getattr(message, "uuid", None) or value) + (":" + str(index) if len(message.content) > 1 else ""))
                            item_id = fallback_blocks.setdefault(signature, mid + ":" + str(len(fallback_blocks)))
                        dedup = (mid, str(block_id or item_id))
                        if dedup in completed_blocks:
                            continue
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
                        completed_blocks.add(dedup)
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
                    async with native["inputLock"]:
                        if native["stopping"] or not native["inputQueue"]:
                            native["acceptingInput"] = False
                            break
                        # Submit only after the CLI returns to idle. There is
                        # never more than one outstanding native input, so CLI
                        # coalescing cannot strand a per-input result counter.
                        message = native["inputQueue"].pop(0)
                        await client.query(message["text"], session_id=native_id)
                        message["delivered"] = True
        finally:
            async with native["inputLock"]:
                native["acceptingInput"] = False
            try:
                await client.disconnect()
            finally:
                native["client"] = None
                await process_group.close()
        if native["stopping"]:
            raise asyncio.CancelledError()

    async def append_input(self, run: RunHandle, input):
        async with run.native.setdefault("inputLock", asyncio.Lock()):
            if run.native.get("stopping") or run.task.done() or run.native.get("acceptingInput") is False:
                raise AdapterError("E_APPEND_CLOSED", "claude.append", "Run has finished receiving input")
            if run.native.get("acceptingInput") is not True:
                raise AdapterError("E_APPEND_NOT_READY", "claude.append", "Run is starting", "Retry this input shortly")
            run.native.setdefault("inputQueue", []).append(input if isinstance(input, dict) else {"text": input})

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

    async def terminal_command(self):
        return [(await self.login_command())[0]]

    async def logout(self):
        args = await self.login_command()
        code, _ = await command([args[0], "auth", "logout"])
        if code:
            raise AdapterError("E_AGENT", "claude.logout", "Official logout command failed", "Check the CLI state using the native terminal")
        self._caps = None

    async def close(self):
        task = getattr(self, "_capability_task", None)
        if task and not task.done():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
