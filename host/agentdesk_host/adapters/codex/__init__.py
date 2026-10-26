"""Codex app-server adapter using the official stdio protocol."""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
import re

from ..base import AdapterError, RunContext, RunHandle
from ..native_common import cached_capabilities, capability, command, installed_codex, version_of
from ..native_rpc import NativeRpc
from ..login import LoginParser


class CodexLoginParser(LoginParser):
    def parse(self, text: str) -> dict:
        # Device-code instructions ask the user to type into the browser.
        result = {"needsInput": bool(re.search(r"(?:paste|enter)[^\n]{0,35}(?:authorization code|auth code)[^\n]{0,20}(?:here|:)|请[^\n]{0,20}(?:粘贴|输入)[^\n]{0,20}授权码", text, re.I))}
        if url := self.url(text):
            result["url"] = url
        if match := re.search(r"(?:^|\n)\s*([A-Z0-9]{4,8}-[A-Z0-9]{4,8})\s*(?:\n|$)", text):
            result["deviceCode"] = match.group(1)
        elif match := re.search(r"(?:device|user|one[- ]time)\s+code\s*[:：]\s*([A-Z0-9]{4,8}-[A-Z0-9]{4,8})", text, re.I):
            result["deviceCode"] = match.group(1)
        if state := self.terminal_state(text):
            result["state"] = state
        return result


class CodexAdapter:
    name = "Codex"
    kind = "codex"
    login_parser = CodexLoginParser
    local_execution_policy = {"sandbox": "workspace-write", "networkAccess": True, "version": 1}

    def __init__(self, executable: str | None = None):
        self.executable = executable
        self._caps = None
        self._checked_at = 0.0

    def path(self):
        return self.executable or installed_codex()

    @cached_capabilities
    async def capabilities(self) -> dict:
        now = asyncio.get_running_loop().time()
        if self._caps and now - self._checked_at < 600:
            return dict(self._caps)
        executable = self.path()
        result = capability()
        if executable:
            result.update(installed=True, authStatus="failed")
            try:
                code, output = await command([executable, "--version"])
                result.update(installed=True, version=version_of(output), authStatus="failed")
                if code == 0:
                    async with NativeRpc(executable) as rpc:
                        account = await rpc.request("account/read", {"refreshToken": False})
                        if not isinstance(account, dict) or "account" not in account or account["account"] is not None and not isinstance(account["account"], dict):
                            raise ValueError("Invalid authentication status")
                        public = account.get("account")
                        logged_in = isinstance(public, dict)
                        result.update(loggedIn=logged_in, authStatus="loggedIn" if logged_in else "loggedOut", accountType=public.get("planType", public.get("type")) if logged_in else None)
                        models = []
                        cursor = None
                        for _ in range(10):
                            params = {"limit": 100, "includeHidden": False}
                            if cursor:
                                params["cursor"] = cursor
                            page = await rpc.request("model/list", params)
                            for row in page.get("data", []):
                                identifier = row.get("model", row.get("id"))
                                if isinstance(identifier, str):
                                    models.append({"id": identifier, "name": row.get("displayName", identifier)})
                            cursor = page.get("nextCursor")
                            if not cursor:
                                break
                        # The protocol version is gated: newer/older native
                        # semantics require explicit compatibility verification.
                        compatible = result["version"] == "0.164.0"
                        result.update(models=models, handshake=True, streaming=compatible, resume=compatible, approvals=compatible, stop=compatible, appendInput=compatible)
                        result.update(available=logged_in and compatible, reason="E_CLI_VERSION" if not compatible else None if logged_in else "E_NOT_LOGGED_IN")
                    if os.name == "posix":
                        resource = Path(executable).parent / "codex-resources/bwrap"
                        result["resourcesAvailable"] = resource.is_file() and os.access(resource, os.X_OK)
                        sandbox_code, _ = await command([executable, "sandbox", "--", "/bin/true"])
                        result["sandboxAvailable"] = sandbox_code == 0
                        if not result["sandboxAvailable"]:
                            result.update(available=False, reason="E_SANDBOX")
                else:
                    result["reason"] = "E_AGENT_EXECUTABLE"
            except (OSError, ValueError, TimeoutError, AdapterError):
                result.update(available=False, reason="E_AGENT_PROBE")
        self._caps, self._checked_at = result, now
        return dict(result)

    async def native_exists(self, session: dict) -> bool:
        if not session.get("nativeId"):
            return True
        executable = self.path()
        if not executable:
            raise AdapterError("E_AGENT_NOT_INSTALLED", "codex.native", "Codex CLI is unavailable", "Install the CLI before checking native records")
        async with NativeRpc(executable) as rpc:
            try:
                await rpc.request("thread/read", {"threadId": session["nativeId"], "includeTurns": False})
                return True
            except AdapterError as error:
                if error.code == "E_NATIVE_MISSING":
                    return False
                raise

    async def start_run(self, session: dict, input: dict, ctx: RunContext) -> RunHandle:
        if session.get("kind") == "local" and session.get("permissionMode") == "plan":
            raise AdapterError("E_LOCAL_PLAN_UNSUPPORTED", "codex.permissions", "Plan mode cannot enforce read-only device access")
        caps = session.get("_capabilities") or await self.capabilities()
        if not caps["installed"]:
            raise AdapterError("E_AGENT_NOT_INSTALLED", "codex.preflight", "Codex is unavailable", "Install the CLI and its matching resources")
        if not caps["loggedIn"]:
            raise AdapterError("E_NOT_LOGGED_IN", "codex.auth", "Codex is not logged in", "Sign in from the environment page")
        if not caps["available"]:
            raise AdapterError(caps.get("reason") or "E_CAPABILITY", "codex.preflight", "Codex preflight failed", "Check the CLI version, resources and sandbox")
        if not session.get("_nativeExistsChecked") and not await self.native_exists(session):
            raise AdapterError("E_NATIVE_MISSING", "codex.resume", "Native thread record is missing", "Restore the native thread or create a new session")
        mode = session.get("permissionMode", "default")
        if mode not in ("default", "plan"):
            raise AdapterError("E_CAPABILITY", "codex.permissions", "Unsupported permission mode", "Select default or plan")
        if session.get("kind") == "local" and (not session.get("deviceRoot") or not session.get("deviceInstructions")):
            raise AdapterError("E_DEVICE_NOT_READY", "codex.target", "Local execution channel is not prepared")
        native = {"rpc": None, "threadId": None, "turnId": None, "stopping": False}
        return RunHandle(ctx.run_id, asyncio.create_task(self._run(session, input, ctx, native)), native)

    async def _run(self, session, input, ctx, native):
        if session.get("kind") == "local" and session.get("permissionMode") == "plan":
            raise AdapterError("E_LOCAL_PLAN_UNSUPPORTED", "codex.permissions", "Plan mode cannot enforce read-only device access")
        config = ["-c", 'sandbox_mode="read-only"'] if session.get("permissionMode") == "plan" else []
        if session.get("kind") == "local":
            # A-02, policy version 1: no user config edits or privilege fallback.
            config = ["-c", 'sandbox_mode="workspace-write"', "-c", "sandbox_workspace_write.network_access=true", "-c", "sandbox_workspace_write.writable_roots=" + json.dumps([session["deviceRoot"]])]
        async with NativeRpc(self.path(), config) as rpc:
            native["rpc"] = rpc

            async def server_request(method: str, params: dict):
                if params.get("threadId") != native["threadId"] or (native["turnId"] and params.get("turnId") != native["turnId"]):
                    raise AdapterError("E_TARGET", "codex.approval", "Approval belongs to another turn", "Discard the stale request")
                if method not in ("item/commandExecution/requestApproval", "item/fileChange/requestApproval", "item/tool/requestUserInput", "tool/requestUserInput"):
                    raise AdapterError("E_CAPABILITY", "codex.approval", "Unsupported native request", "Use the official terminal for this capability")
                question = "requestUserInput" in method
                payload = {"tool": method.split("/")[1], "input": params, "reason": params.get("reason")}
                if question:
                    payload["questions"] = params.get("questions", [])
                decision = await ctx.approve("question" if question else "tool", payload)
                if question:
                    answers = decision.get("answers", {}) if decision.get("behavior") == "allow" else {}
                    mapped = {}
                    for entry in params.get("questions", []):
                        key = entry.get("id", entry.get("question"))
                        value = answers.get(entry.get("question"), answers.get(key))
                        if key and value is not None:
                            mapped[key] = {"answers": value if isinstance(value, list) else [value]}
                    return {"answers": mapped}
                return {"decision": "accept" if decision.get("behavior") == "allow" else "decline"}

            rpc.request_handler = server_request
            params = {"cwd": session["path"], "approvalPolicy": "on-request"}
            if session.get("model"):
                params["model"] = session["model"]
            if session.get("permissionMode") == "plan":
                params["sandbox"] = "read-only"
            if session.get("kind") == "local":
                params.update(sandbox="workspace-write", developerInstructions=session["deviceInstructions"])
            if session.get("nativeId"):
                params["threadId"] = session["nativeId"]
                thread = await rpc.request("thread/resume", params)
            else:
                thread = await rpc.request("thread/start", params)
            native["threadId"] = thread["thread"]["id"]
            await ctx.save_native(native["threadId"])
            turn = await rpc.request("turn/start", {"threadId": native["threadId"], "input": [{"type": "text", "text": input["text"]}]})
            native["turnId"] = turn["turn"]["id"]
            while True:
                event = await rpc.notifications.get()
                method, params = event["method"], event.get("params", {})
                if method == "_closed":
                    raise AdapterError("E_AGENT_EXITED", "codex.run", "App server closed during the turn", "Resume the session after checking the CLI")
                if params.get("threadId") not in (None, native["threadId"]):
                    continue
                if params.get("turnId") not in (None, native["turnId"]):
                    continue
                if method == "turn/completed":
                    status = params.get("turn", {}).get("status")
                    if status == "interrupted" or native["stopping"]:
                        raise asyncio.CancelledError()
                    if status != "completed":
                        raise AdapterError("E_AGENT", "codex.run", "Codex turn failed", "Review native CLI state and retry")
                    break
                await self.map_event(method, params, ctx)
        native["rpc"] = None

    @staticmethod
    async def map_event(method, params, ctx):
        item_id = params.get("itemId")
        if method in ("item/agentMessage/delta", "item/reasoning/summaryTextDelta", "item/reasoning/textDelta"):
            await ctx.emit("item.delta", {"kind": "text" if "agentMessage" in method else "reasoning", "delta": params.get("delta", "")}, item_id)
        elif method == "item/commandExecution/outputDelta":
            await ctx.emit("item.delta", {"kind": "tool_result", "delta": params.get("delta", ""), "toolCallId": item_id}, str(item_id) + ":result")
        elif method in ("item/started", "item/completed"):
            item = params.get("item", {})
            item_id = item.get("id", item_id)
            kind = item.get("type")
            completed = method == "item/completed"
            if kind == "agentMessage":
                payload = {"kind": "text", "text": item.get("text", "")}
            elif kind == "reasoning":
                summary = item.get("summary", [])
                payload = {"kind": "reasoning", "text": "\n".join(x.get("text", "") if isinstance(x, dict) else str(x) for x in summary)}
            elif kind == "fileChange":
                payload = {"kind": "file_change", "changes": item.get("changes", []), "status": item.get("status"), "scope": "server"}
            elif kind in ("commandExecution", "mcpToolCall", "dynamicToolCall", "webSearch"):
                payload = {"kind": "tool_call", "name": item.get("tool", kind), "input": item.get("arguments", item.get("command", item.get("query", ""))), "status": item.get("status")}
                if completed:
                    await ctx.emit("item.completed", payload, item_id)
                    await ctx.emit("item.completed", {"kind": "tool_result", "toolCallId": item_id, "output": item.get("aggregatedOutput", item.get("result", item.get("contentItems", ""))), "exitCode": item.get("exitCode"), "isError": item.get("status") in ("failed", "declined")}, str(item_id) + ":result")
                    return
            elif kind == "plan":
                payload = {"kind": "plan", "text": item.get("text", "")}
            else:
                return
            await ctx.emit("item.completed" if completed else "item.started", payload, item_id)
        elif method == "thread/tokenUsage/updated":
            if params.get("tokenUsage"):
                usage = dict(params["tokenUsage"])
                last = usage.get("last", {})
                capacity = usage.get("modelContextWindow")
                used = last.get("totalTokens") if isinstance(last, dict) else None
                if type(capacity) is int and capacity > 0 and type(used) is int and 0 <= used <= capacity:
                    usage["contextPercent"] = used * 100 / capacity
                await ctx.emit("usage", usage)
        elif method == "error":
            # No raw native diagnostic payload is copied to a desktop event.
            await ctx.emit("item.completed", {"kind": "error", "code": "E_AGENT", "text": "Codex reported a run error", "stage": "codex.run", "hint": "Inspect the native CLI state"}, item_id)

    async def append_input(self, run: RunHandle, input):
        native = run.native
        if not native.get("rpc") or not native.get("turnId"):
            raise AdapterError("E_SESSION_BUSY", "codex.append", "Run has not started", "Wait for the turn to start")
        text = input["text"] if isinstance(input, dict) else input
        await native["rpc"].request("turn/steer", {"threadId": native["threadId"], "expectedTurnId": native["turnId"], "input": [{"type": "text", "text": text}]})

    async def stop(self, run: RunHandle):
        native = run.native
        native["stopping"] = True
        if native.get("rpc") and native.get("turnId"):
            try:
                await native["rpc"].request("turn/interrupt", {"threadId": native["threadId"], "turnId": native["turnId"]}, timeout=5)
                await asyncio.wait_for(asyncio.shield(run.task), 5)
                return
            except asyncio.CancelledError:
                if not run.task.done():
                    raise
                return
            except (TimeoutError, AdapterError):
                pass
        run.task.cancel()

    async def respond_approval(self, run, approval_id, decision):
        return None

    async def delete_session(self, session):
        if session.get("nativeId") and await self.native_exists(session):
            # Archive preserves native data. It is not a deletion API.
            raise AdapterError("E_CAPABILITY", "codex.delete", "Native thread deletion is unsupported", "Remove the native record using supported CLI tooling first")

    async def login_command(self):
        executable = self.path()
        if not executable:
            raise AdapterError("E_AGENT_NOT_INSTALLED", "codex.login", "Codex CLI is unavailable", "Install the official binary first")
        return [executable, "login", "--device-auth"]

    def login_rpc(self):
        executable = self.path()
        if not executable:
            raise AdapterError("E_AGENT_NOT_INSTALLED", "codex.login", "Codex CLI is unavailable")
        return NativeRpc(executable)

    async def terminal_command(self):
        return [(await self.login_command())[0]]

    async def logout(self):
        args = await self.login_command()
        code, _ = await command([args[0], "logout"])
        if code:
            raise AdapterError("E_AGENT", "codex.logout", "Official logout command failed", "Check the CLI state using the native terminal")
        self._caps = None

    async def close(self):
        task = getattr(self, "_capability_task", None)
        if task and not task.done():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
