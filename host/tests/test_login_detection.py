"""Authentication status reflects official status responses, not CLI presence."""

import asyncio
import json
import sys
from types import SimpleNamespace

import pytest

from agentdesk_host.adapters import claude, codex


@pytest.mark.parametrize("factory,module,path_method", [(claude.ClaudeAdapter, claude, "executable"), (codex.CodexAdapter, codex, "path")])
def test_missing_and_failed_detection_are_distinct(factory, module, path_method, monkeypatch):
    async def check():
        adapter = factory()
        monkeypatch.setattr(adapter, path_method, lambda: None)
        assert (await adapter.capabilities())["authStatus"] == "missing"
        adapter._caps = None
        monkeypatch.setattr(adapter, path_method, lambda: "present-official-cli")
        async def broken(*args, **kwargs):
            raise OSError("not executable")
        monkeypatch.setattr(module, "command", broken)
        caps = await adapter.capabilities()
        assert caps["installed"] and caps["authStatus"] == "failed"
        assert not caps["available"] and not caps["loggedIn"]
        await adapter.close()
    asyncio.run(check())


@pytest.mark.parametrize("auth,exit_code,expected", [
    ({"loggedIn": False}, 1, "loggedOut"),
    ({"loggedIn": True, "subscriptionType": "max"}, 0, "loggedIn"),
    ({"loggedIn": "true"}, 0, "failed"),
    ({"loggedIn": True}, 1, "failed"),
    ({"unexpected": "FAKE_ACCOUNT_SECRET"}, 0, "failed"),
])
def test_claude_official_auth_status(auth, exit_code, expected, monkeypatch):
    class SdkClient:
        async def connect(self):
            pass
        async def get_server_info(self):
            return {"models": []}
        async def disconnect(self):
            pass
    sdk = SimpleNamespace(ClaudeSDKClient=lambda **kwargs: SdkClient(), ClaudeAgentOptions=lambda **kwargs: kwargs)
    monkeypatch.setitem(sys.modules, "claude_agent_sdk", sdk)
    monkeypatch.setattr(claude.importlib.metadata, "version", lambda _: "0.2.174")
    async def command(args):
        if args[-1] == "--version":
            return 0, b"2.1.310"
        return exit_code, json.dumps(auth).encode()
    monkeypatch.setattr(claude, "command", command)

    async def check():
        adapter = claude.ClaudeAdapter()
        monkeypatch.setattr(adapter, "executable", lambda: "present-official-cli")
        caps = await adapter.capabilities()
        assert caps["installed"] and caps["authStatus"] == expected
        assert caps["loggedIn"] is (expected == "loggedIn")
        assert "FAKE_ACCOUNT_SECRET" not in json.dumps(caps)
        if expected == "loggedIn":
            assert caps["accountType"] == "max"
        await adapter.close()
    asyncio.run(check())


@pytest.mark.parametrize("account,expected", [(None, "loggedOut"), ({"type": "chatgpt", "planType": "pro", "email": "FAKE_ACCOUNT_SECRET"}, "loggedIn"), ("invalid", "failed")])
def test_codex_reads_only_public_account_type(account, expected, monkeypatch):
    class Rpc:
        async def __aenter__(self):
            return self
        async def __aexit__(self, *args):
            pass
        async def request(self, method, params):
            return {"account": account} if method == "account/read" else {"data": []}
    monkeypatch.setattr(codex, "NativeRpc", lambda _: Rpc())
    async def command(args):
        return (0, b"0.164.0") if args[-1] == "--version" else (0, b"")
    monkeypatch.setattr(codex, "command", command)

    async def check():
        adapter = codex.CodexAdapter("present-official-cli")
        caps = await adapter.capabilities()
        assert caps["installed"] and caps["authStatus"] == expected
        assert caps["loggedIn"] is (expected == "loggedIn")
        assert "FAKE_ACCOUNT_SECRET" not in json.dumps(caps)
        if expected == "loggedIn":
            assert caps["accountType"] == "pro"
        await adapter.close()
    asyncio.run(check())
