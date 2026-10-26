"""Probe installed official clients without login, prompts, or credential output."""

from __future__ import annotations

import importlib.metadata
from dataclasses import fields
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import subprocess
import sys
import time


def command(arguments: list[str], timeout: int = 20) -> dict:
    try:
        result = subprocess.run(
            arguments,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
            check=False,
        )
        raw = result.stdout + result.stderr
        match = re.search(rb"\b\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?\b", raw)
        # No raw command output may leave this process, including auth status.
        return {
            "exitCode": result.returncode,
            "version": match.group().decode("ascii") if match else None,
            "glibcIncompatible": b"GLIBC_" in raw or b"GLIBCXX_" in raw,
            "sandboxUnavailable": result.returncode != 0 and any(value in raw.lower() for value in (b"landlock", b"seccomp", b"bubblewrap", b"bwrap", b"unshare", b"operation not permitted")),
            "namespaceFailure": result.returncode != 0 and b"namespace" in raw.lower(),
        }
    except subprocess.TimeoutExpired:
        return {"timedOut": True}
    except OSError:
        return {"executableUnavailable": True}


def codex_rpc(executable: str) -> dict:
    process = subprocess.Popen(
        [executable, "app-server", "--listen", "stdio://"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    assert process.stdin is not None and process.stdout is not None
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ)
    buffer = bytearray()

    def send(value: dict) -> None:
        process.stdin.write(json.dumps(value).encode() + b"\n")
        process.stdin.flush()

    def request(identifier: int, method: str, params: dict) -> dict:
        send({"id": identifier, "method": method, "params": params})
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            while b"\n" in buffer:
                line, _, rest = buffer.partition(b"\n")
                buffer[:] = rest
                value = json.loads(line)
                if value.get("id") == identifier:
                    return value
                if "id" in value and "method" in value:
                    # No unexpected approval/tool request is ever accepted.
                    send({"id": value["id"], "error": {"code": -32601, "message": "Unsupported request"}})
            if not selector.select(timeout=min(0.25, max(0, deadline - time.monotonic()))):
                continue
            data = os.read(process.stdout.fileno(), 65536)
            if not data:
                raise RuntimeError("App server closed its output")
            buffer.extend(data)
            if len(buffer) > 1024 * 1024:
                raise RuntimeError("App server response exceeded the probe limit")
        raise TimeoutError("App server response timed out")

    def shape(response: dict) -> dict:
        result = response.get("result")
        error = response.get("error")
        return {
            "success": isinstance(result, dict),
            "fields": sorted(result) if isinstance(result, dict) else [],
            "errorCode": error.get("code") if isinstance(error, dict) else None,
        }

    try:
        initialize = request(1, "initialize", {"clientInfo": {"name": "agentdesk_spike", "title": "AgentDesk Spike", "version": "0.1.0"}})
        result = {"initialize": shape(initialize)}
        if not result["initialize"]["success"]:
            return result
        send({"method": "initialized", "params": {}})
        account = request(2, "account/read", {"refreshToken": False})
        result["account/read"] = shape(account)
        account_result = account.get("result", {})
        result["account/read"]["loggedIn"] = isinstance(account_result.get("account"), dict)
        result["account/read"]["requiresOpenaiAuth"] = account_result.get("requiresOpenaiAuth") is True
        models = request(3, "model/list", {"limit": 20, "includeHidden": False})
        result["model/list"] = shape(models)
        model_result = models.get("result", {})
        rows = model_result.get("data", [])
        result["model/list"]["count"] = len(rows) if isinstance(rows, list) else 0
        result["model/list"]["entryFields"] = sorted(rows[0]) if isinstance(rows, list) and rows and isinstance(rows[0], dict) else []
        result["model/list"]["hasNextCursor"] = model_result.get("nextCursor") is not None
        result["modelCalls"] = "skipped_no_prompt_or_credentials_used"
        return result
    except (TimeoutError, RuntimeError, OSError, ValueError):
        return {"status": "failed", "reason": "bounded_rpc_probe_failed"}
    finally:
        selector.close()
        # CLI descendants must not survive a failed or cancelled probe.
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=5)
        process.stdin.close()
        process.stdout.close()


def main() -> None:
    result: dict = {}
    try:
        import claude_agent_sdk

        result["claudeSdk"] = {
            "installed": True,
            "version": importlib.metadata.version("claude-agent-sdk"),
            "exports": {
                name: hasattr(claude_agent_sdk, name)
                for name in ("ClaudeSDKClient", "ClaudeAgentOptions", "StreamEvent", "get_session_info", "list_sessions", "get_session_messages")
            },
            "clientMethods": {
                name: hasattr(claude_agent_sdk.ClaudeSDKClient, name)
                for name in ("connect", "query", "receive_response", "interrupt", "disconnect", "get_server_info")
            },
            "optionFields": {
                name: name in {field.name for field in fields(claude_agent_sdk.ClaudeAgentOptions)}
                for name in ("session_id", "resume", "can_use_tool", "permission_mode", "include_partial_messages", "system_prompt")
            },
        }
        bundled = Path(claude_agent_sdk.__file__).parent / "_bundled" / "claude"
    except (ImportError, importlib.metadata.PackageNotFoundError):
        result["claudeSdk"] = {"installed": False}
        bundled = Path("/nonexistent")
    claude = shutil.which("claude")
    if not claude and bundled.is_file():
        claude = str(bundled)
    if claude:
        version = command([claude, "--version"])
        status = command([claude, "auth", "status"])
        result["claude"] = {
            "present": True,
            "bundled": claude == str(bundled),
            "executable": version,
            "authStatusChecked": status.get("exitCode") in (0, 1),
            "loggedIn": status.get("exitCode") == 0,
            "modelCalls": "skipped_no_prompt_or_credentials_used",
        }
    else:
        result["claude"] = {"present": False}
    codex = shutil.which("codex") or (sys.argv[1] if len(sys.argv) > 1 else None)
    if codex:
        version = command([codex, "--version"])
        status = command([codex, "login", "status"])
        result["codex"] = {
            "present": True,
            "executable": version,
            "authStatusChecked": status.get("exitCode") in (0, 1),
            "loggedIn": status.get("exitCode") == 0,
        }
        if version.get("exitCode") == 0:
            result["codex"]["rpc"] = codex_rpc(codex)
            result["codex"]["sandboxHelp"] = command([codex, "sandbox", "linux", "--help"])
            result["codex"]["sandboxLocalProcess"] = command([codex, "sandbox", "linux", "--", "/bin/sh", "-c", "printf agentdesk-sandbox"])
            result["codex"]["sandboxModelCalls"] = False
    else:
        result["codex"] = {"present": False}
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
