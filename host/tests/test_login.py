"""Login parsing, live-only transport, ownership and process cleanup."""

import asyncio
import json
import os
from pathlib import Path
import sys

import pytest

from agentdesk_host.adapters.base import AdapterError
from agentdesk_host.adapters.claude import ClaudeLoginParser
from agentdesk_host.adapters.codex import CodexLoginParser
from agentdesk_host.adapters.fake import FakeAdapter
from agentdesk_host.adapters.login import clean_output, safe_url
from agentdesk_host.server import login as login_module
from agentdesk_host.server.login import LoginManager, PtyLogin
from agentdesk_host.server.service import Client, HostService


FIXTURES = Path(__file__).parent / "fixtures" / "login"
FAKE_URL = "https://login.example.invalid/authorize?state=FAKE_LOGIN_STATE"
FAKE_CODE = "FAKE-12345"
FAKE_INPUT = "FAKE_RETURN_CODE_982734"


@pytest.mark.parametrize("parser,filename", [(ClaudeLoginParser, "claude-waiting.txt"), (CodexLoginParser, "codex-waiting.txt")])
def test_recorded_waiting_output_and_split_escape_sequences(parser, filename):
    output = (FIXTURES / filename).read_text(encoding="utf-8")
    parsed = parser()
    for char in output:
        prompt = parsed.feed(char)
    assert prompt["url"].startswith("https://login.example.invalid/")
    assert prompt["needsInput"] is (parser is ClaudeLoginParser)
    if parser is CodexLoginParser:
        assert prompt["deviceCode"] == FAKE_CODE
        assert "state" not in prompt  # "expires in" is not "expired".
    assert "\x1b" not in clean_output(output)
    parsed.clear()
    assert parsed.output == ""


@pytest.mark.parametrize("parser,output,state", [
    (ClaudeLoginParser, "Login successful.\n", "succeeded"),
    (ClaudeLoginParser, "Login failed: Authentication failed: Invalid authorization code", "failed"),
    (ClaudeLoginParser, "Token exchange failed (401): Unauthorized", "failed"),
    (CodexLoginParser, "Successfully logged in", "succeeded"),
    (CodexLoginParser, "Error logging in with device code: request failed", "failed"),
    (CodexLoginParser, "Error logging in with device code: device auth timed out after 15 minutes", "expired"),
    # Constructed forward-compatibility cases, not authorized-session recordings.
    (ClaudeLoginParser, "Authorization code has expired", "expired"),
    (CodexLoginParser, "device_code_expired", "expired"),
])
def test_official_message_templates_and_constructed_terminal_cases(parser, output, state):
    assert parser().feed(output)["state"] == state


@pytest.mark.parametrize("parser", [ClaudeLoginParser, CodexLoginParser])
def test_constructed_device_input_fallback_and_url_validation(parser):
    assert parser().feed("User code: " + FAKE_CODE)["deviceCode"] == FAKE_CODE
    assert parser().feed("Paste authorization code here: ")["needsInput"]
    prompt = parser().feed("\x1b[31mUnexpected login instruction\x1b[0m\x00\x08")
    assert prompt == {"needsInput": True, "rawOutput": "Unexpected login instruction"}
    unknown = parser().feed(FAKE_URL + "\nA new input instruction from a future CLI version")
    assert unknown["url"] == FAKE_URL
    assert unknown["rawOutput"].endswith("future CLI version") and unknown["needsInput"]
    partial = parser()
    assert "url" not in partial.feed("Open " + FAKE_URL[:25])
    assert partial.feed(FAKE_URL[25:] + "\n")["url"] == FAKE_URL
    assert safe_url("javascript:alert(1)") is None
    assert safe_url("http://localhost:1234/callback") is None
    assert safe_url("https://name:password@example.invalid/") is None
    assert safe_url(FAKE_URL) == FAKE_URL


class AccountAdapter(FakeAdapter):
    login_parser = ClaudeLoginParser

    def __init__(self):
        self.logged_in = False
        self.probes = 0

    async def capabilities(self):
        self.probes += 1
        return {**await super().capabilities(), "loggedIn": self.logged_in, "authStatus": "loggedIn" if self.logged_in else "loggedOut", "accountType": "subscription" if self.logged_in else None}

    async def login_command(self):
        return ["official-cli", "auth", "login"]

    async def logout(self):
        self.logged_in = False


class MemoryPty:
    instances = []
    output = f"Opening browser to sign in…\n{FAKE_URL}\nPaste code here if prompted > ".encode()

    def __init__(self, arguments):
        self.arguments = arguments
        self.process = self
        self.returncode = None
        self.reads = 0
        self.closed = False
        self.inputs = []
        self.release = asyncio.Event()
        self.instances.append(self)

    async def start(self):
        pass

    async def read(self):
        self.reads += 1
        if self.reads == 1:
            return self.output
        await self.release.wait()
        return b""

    async def respond(self, text):
        self.inputs.append(text)

    async def close(self):
        self.closed = True
        self.returncode = -15
        self.release.set()

    async def wait(self):
        await self.release.wait()
        return self.returncode or 0


async def settle(predicate):
    async with asyncio.timeout(3):
        while not predicate():
            await asyncio.sleep(0.001)


async def connect(service, device="test-device"):
    messages = []
    async def send(message):
        messages.append(message)
    client = Client(send)
    await service.request(client, "hello", {"protocolVersion": "1.0", "deviceId": device, "deviceName": "Login tests", "os": "linux", "client": "tests"})
    return client, messages


def test_login_is_live_only_owner_checked_and_cancelled_without_persistence(tmp_path, monkeypatch, caplog):
    monkeypatch.setattr(login_module, "PtyLogin", MemoryPty)
    monkeypatch.setattr(MemoryPty, "output", MemoryPty.output + ("\nUser code: " + FAKE_CODE + "\n").encode())

    async def check():
        service = HostService(tmp_path, {"claude": AccountAdapter()})
        client, messages = await connect(service)
        other, other_messages = await connect(service, "other-device")
        started = await service.request(client, "login.start", {"agent": "claude"})
        identifier = started["loginId"]
        await settle(lambda: any(message["method"] == "login.prompt" for message in messages))
        process = MemoryPty.instances[-1]
        assert not other_messages
        prompt = next(message["params"] for message in messages if message["method"] == "login.prompt")
        assert prompt["url"] == FAKE_URL and prompt["needsInput"]
        assert prompt["deviceCode"] == FAKE_CODE
        assert "seq" not in prompt
        with pytest.raises(AdapterError, match="another connection"):
            await service.request(other, "login.respond", {"loginId": identifier, "text": FAKE_INPUT})
        with pytest.raises(AdapterError) as busy:
            await service.request(other, "login.start", {"agent": "claude"})
        assert busy.value.code == "E_LOGIN_BUSY"
        with pytest.raises(AdapterError) as logout_busy:
            await service.request(client, "agent.logout", {"agent": "claude"})
        assert logout_busy.value.code == "E_LOGIN_BUSY"
        with pytest.raises(AdapterError):
            await service.request(client, "host.prepareShutdown", {})
        for text in ("line\nsecond", "control\x1b", "control\x9b", "a" * 2049):
            with pytest.raises(AdapterError) as invalid:
                await service.request(client, "login.respond", {"loginId": identifier, "text": text})
            assert invalid.value.code == "E_INVALID_PARAMS"
        assert await service.request(client, "login.respond", {"loginId": identifier, "text": FAKE_INPUT}) == {"accepted": True}
        assert process.inputs == [FAKE_INPUT]
        assert await service.request(client, "login.cancel", {"loginId": identifier}) == {"accepted": True}
        assert process.closed
        assert messages[-1]["params"]["state"] == "cancelled"
        assert messages[-1]["params"]["capability"]["authStatus"] == "loggedOut"
        assert not service.logins.sessions and not service.logins.agents
        sync = await service.request(client, "sync.since", {"lastSeq": 0})
        assert not sync["events"] and not sync["snapshots"]
        assert not service.db.all("SELECT * FROM events")
        dump = "\n".join(service.db.connection.iterdump()) if hasattr(service.db, "connection") else "\n".join(service.db.conn.iterdump())
        await service.close()
        # The Host has no diagnostic exporter: scan its persistent files and
        # log capture; the desktop's actual export is covered by Rust E2E.
        persisted = b"\n".join(path.read_bytes() for path in tmp_path.rglob("*") if path.is_file())
        for secret in (FAKE_URL, FAKE_CODE, FAKE_INPUT):
            assert secret not in json.dumps(sync)
            assert secret not in dump
            assert secret not in caplog.text
            assert secret.encode() not in persisted
    asyncio.run(check())


def test_disconnect_destroys_login_and_new_connection_cannot_resume(tmp_path, monkeypatch):
    monkeypatch.setattr(login_module, "PtyLogin", MemoryPty)

    async def check():
        service = HostService(tmp_path, {"claude": AccountAdapter()})
        client, messages = await connect(service)
        identifier = (await service.request(client, "login.start", {"agent": "claude"}))["loginId"]
        await settle(lambda: any(message["method"] == "login.prompt" for message in messages))
        session = service.logins.sessions[identifier]
        await service.disconnect(client)
        assert MemoryPty.instances[-1].closed
        assert session.parser.output == "" and session.prompt == {}
        replacement, _ = await connect(service)
        with pytest.raises(AdapterError) as gone:
            await service.request(replacement, "login.respond", {"loginId": identifier, "text": FAKE_INPUT})
        assert gone.value.code == "E_LOGIN_GONE"
        await service.close()
    asyncio.run(check())


def test_concurrent_cancel_and_disconnect_wait_for_one_complete_cleanup(tmp_path, monkeypatch):
    class SlowClosePty(MemoryPty):
        async def close(self):
            self.closing = True
            await asyncio.sleep(0.02)
            await super().close()
    monkeypatch.setattr(login_module, "PtyLogin", SlowClosePty)

    async def check():
        service = HostService(tmp_path, {"claude": AccountAdapter()})
        client, messages = await connect(service)
        identifier = (await service.request(client, "login.start", {"agent": "claude"}))["loginId"]
        await settle(lambda: any(message["method"] == "login.prompt" for message in messages))
        session = service.logins.sessions[identifier]
        cancel = asyncio.create_task(service.request(client, "login.cancel", {"loginId": identifier}))
        await settle(lambda: getattr(session.pty, "closing", False))
        await asyncio.gather(service.disconnect(client), cancel)
        assert session.pty.closed
        assert session.parser.output == "" and session.prompt == {}
        assert not service.logins.sessions and not service.logins.agents
        await service.close()
    asyncio.run(check())


def test_cancel_during_final_status_probe_keeps_cleanup_intact(tmp_path, monkeypatch):
    monkeypatch.setattr(login_module, "PtyLogin", MemoryPty)

    async def check():
        adapter = AccountAdapter()
        service = HostService(tmp_path, {"claude": adapter})
        client, messages = await connect(service)
        identifier = (await service.request(client, "login.start", {"agent": "claude"}))["loginId"]
        await settle(lambda: any(message["method"] == "login.prompt" for message in messages))
        session = service.logins.sessions[identifier]
        original = service.logins.refresh_capability
        probing = asyncio.Event()
        async def slow_probe(agent):
            probing.set()
            await asyncio.sleep(0.02)
            return await original(agent)
        service.logins.refresh_capability = slow_probe
        session.pty.release.set()
        await probing.wait()
        await service.request(client, "login.cancel", {"loginId": identifier})
        assert session.pty.closed and session.parser.output == ""
        assert messages[-1]["params"]["state"] == "cancelled"
        assert not service.logins.sessions
        await service.close()
    asyncio.run(check())


@pytest.mark.parametrize("logged_in,expected", [(True, "succeeded"), (False, "failed")])
def test_exit_reprobes_auth_and_never_trusts_success_text_alone(tmp_path, monkeypatch, logged_in, expected):
    monkeypatch.setattr(login_module, "PtyLogin", MemoryPty)
    monkeypatch.setattr(MemoryPty, "output", MemoryPty.output + b"\nLogin successful.\n")

    async def check():
        adapter = AccountAdapter()
        service = HostService(tmp_path, {"claude": adapter})
        client, messages = await connect(service)
        await service.request(client, "login.start", {"agent": "claude"})
        await settle(lambda: any(message["method"] == "login.prompt" for message in messages))
        adapter.logged_in = logged_in
        process = MemoryPty.instances[-1]
        process.release.set()
        await settle(lambda: not service.logins.sessions)
        assert messages[-1]["params"]["state"] == expected
        assert adapter.probes >= 2
        if not logged_in:
            assert messages[-1]["params"]["reason"] == "E_LOGIN_VERIFY"
        await service.close()
    asyncio.run(check())


class DeviceRpc:
    unsupported = False
    failure = False

    def __init__(self):
        self.calls = []
        self.notifications = asyncio.Queue()
        self.closed = False

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        self.closed = True

    async def request(self, method, params, **kwargs):
        self.calls.append((method, params))
        if method == "account/login/start":
            if self.unsupported or self.failure:
                raise AdapterError("E_NATIVE_UNSUPPORTED" if self.unsupported else "E_AGENT", "codex.rpc", "Safe rejection")
            return {"loginId": "native-device-login", "verificationUrl": FAKE_URL, "userCode": FAKE_CODE}
        return {}


def test_codex_device_rpc_first_cancels_and_does_not_spawn_pty(monkeypatch):
    async def check():
        rpc = DeviceRpc()
        adapter = AccountAdapter()
        adapter.login_rpc = lambda: rpc
        notifications = []
        client = Client(lambda message: append_message(notifications, message))
        manager = LoginManager(lambda _: adapter.capabilities())
        identifier = manager.start(client, "codex", adapter)["loginId"]
        await settle(lambda: any(message["method"] == "login.prompt" for message in notifications))
        prompt = next(message["params"] for message in notifications if message["method"] == "login.prompt")
        assert prompt["deviceCode"] == FAKE_CODE and not prompt["needsInput"]
        assert manager.sessions[identifier].pty is None
        await manager.cancel(client, identifier)
        assert rpc.closed
        assert rpc.calls == [("account/login/start", {"type": "chatgptDeviceCode"}), ("account/login/cancel", {"loginId": "native-device-login"})]
    asyncio.run(check())


def test_device_rpc_timeout_cancels_native_login_and_closes_transport():
    async def check():
        rpc = DeviceRpc()
        adapter = AccountAdapter()
        adapter.login_rpc = lambda: rpc
        messages = []
        client = Client(lambda message: append_message(messages, message))
        manager = LoginManager(lambda _: adapter.capabilities())
        from agentdesk_host.server.login import LoginSession
        session = LoginSession("timeout", "codex", client, adapter, ClaudeLoginParser())
        with pytest.raises(TimeoutError):
            async with asyncio.timeout(0.02):
                await manager._device_rpc(session)
        assert rpc.closed
        assert rpc.calls[-1] == ("account/login/cancel", {"loginId": "native-device-login"})
    asyncio.run(check())


@pytest.mark.parametrize("success,error,expected", [(True, None, "succeeded"), (False, "device_code_expired", "expired"), (False, "sensitive reason " + FAKE_URL + FAKE_CODE, "failed")])
def test_device_rpc_completion_is_scoped_and_reason_is_safe(success, error, expected):
    async def check():
        rpc = DeviceRpc()
        adapter = AccountAdapter()
        adapter.login_rpc = lambda: rpc
        messages = []
        client = Client(lambda message: append_message(messages, message))
        manager = LoginManager(lambda _: adapter.capabilities())
        identifier = manager.start(client, "codex", adapter)["loginId"]
        await settle(lambda: any(message["method"] == "login.prompt" for message in messages))
        await rpc.notifications.put({"method": "account/login/completed", "params": {"loginId": "another-login", "success": True}})
        await asyncio.sleep(0)
        assert identifier in manager.sessions
        adapter.logged_in = success
        await rpc.notifications.put({"method": "account/login/completed", "params": {"loginId": "native-device-login", "success": success, "error": error}})
        await settle(lambda: not manager.sessions)
        assert rpc.closed
        assert messages[-1]["params"]["state"] == expected
        assert FAKE_URL not in json.dumps(messages[-1]) and FAKE_CODE not in json.dumps(messages[-1])
    asyncio.run(check())


async def append_message(messages, message):
    messages.append(message)


@pytest.mark.parametrize("unsupported", [True, False])
def test_rpc_fallback_only_for_explicit_unsupported_error(monkeypatch, unsupported):
    monkeypatch.setattr(login_module, "PtyLogin", MemoryPty)

    async def check():
        rpc = DeviceRpc()
        rpc.unsupported, rpc.failure = unsupported, not unsupported
        adapter = AccountAdapter()
        adapter.login_rpc = lambda: rpc
        messages = []
        client = Client(lambda message: append_message(messages, message))
        manager = LoginManager(lambda _: adapter.capabilities())
        count = len(MemoryPty.instances)
        identifier = manager.start(client, "codex", adapter)["loginId"]
        if unsupported:
            await settle(lambda: any(message["method"] == "login.prompt" for message in messages))
            assert len(MemoryPty.instances) == count + 1
            await manager.cancel(client, identifier)
        else:
            await settle(lambda: not manager.sessions)
            assert len(MemoryPty.instances) == count
            assert messages[-1]["params"]["state"] == "failed"
    asyncio.run(check())


@pytest.mark.skipif(os.name != "posix", reason="PTY is only implemented on the Unix Host")
def test_real_pty_input_has_no_echo_and_cancel_reaps_process():
    async def check():
        driver = PtyLogin([sys.executable, "-u", "-c", "import sys,time; print('Paste code here if prompted >', flush=True); sys.stdin.readline(); print('RECEIVED',flush=True); time.sleep(60)"])
        try:
            await driver.start()
            assert b"Paste code" in await asyncio.wait_for(driver.read(), 3)
            await driver.respond(FAKE_INPUT)
            response = await asyncio.wait_for(driver.read(), 3)
            assert FAKE_INPUT.encode() not in response
            assert b"RECEIVED" in response
        finally:
            await driver.close()
        assert driver.master is None
        assert driver.process.returncode is not None
    asyncio.run(check())
