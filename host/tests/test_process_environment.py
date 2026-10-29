"""Verify native process environments without credentials or model requests."""
import asyncio
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import types

import pytest

from agentdesk_host.adapters import native_common
from agentdesk_host.adapters.fake import FakeAdapter
from agentdesk_host.adapters.native_rpc import NativeRpc
from agentdesk_host.server.login import PtyLogin
from agentdesk_host.server.service import Client, HostService


CONNECTION_KEYS = ('SSH_CONNECTION', 'SSH_CLIENT', 'SSH_TTY', 'SSH_ORIGINAL_COMMAND')
PRESERVED = {
    'AGENTDESK_TEST_TOOLCHAIN': 'toolchain-value',
    'HTTPS_PROXY': 'http://proxy.invalid:8080',
    'ANTHROPIC_API_KEY': 'test-placeholder-not-a-credential',
    'CLAUDE_CONFIG_DIR': 'C:/server/config' if os.name == 'nt' else '/server/config',
    'SSH_AUTH_SOCK': 'C:/server/agent.sock' if os.name == 'nt' else '/server/agent.sock',
    'TZ': 'UTC',
    'LANG': 'C.UTF-8',
}
SNAPSHOT = (
    "import json,os; print(json.dumps({"
    "'connectionKeys':[k for k in " + repr(CONNECTION_KEYS) + " if k in os.environ],"
    "'preserved':{k:os.environ.get(k) for k in " + repr(tuple(PRESERVED)) + "}}),flush=True)"
)


@pytest.fixture
def inherited_environment(monkeypatch):
    for key in CONNECTION_KEYS:
        monkeypatch.setenv(key, 'connection-metadata-sentinel')
    for key, value in PRESERVED.items():
        monkeypatch.setenv(key, value)


def assert_clean(value):
    assert value['connectionKeys'] == []
    assert value['preserved'] == PRESERVED
    assert all(os.environ[key] == 'connection-metadata-sentinel' for key in CONNECTION_KEYS)


def test_native_probe_environment_is_clean_without_mutating_host(inherited_environment):
    code, output = asyncio.run(native_common.command([sys.executable, '-c', SNAPSHOT]))
    assert code == 0
    assert_clean(json.loads(output))


def test_app_server_environment_is_clean(inherited_environment, tmp_path):
    script = tmp_path / 'rpc.py'
    script.write_text(
        'import json,os,sys\n'
        'for line in sys.stdin:\n'
        ' message=json.loads(line)\n'
        ' if "id" in message:\n'
        '  value={"connectionKeys":[k for k in ' + repr(CONNECTION_KEYS) + ' if k in os.environ],'
        '"preserved":{k:os.environ.get(k) for k in ' + repr(tuple(PRESERVED)) + '}}\n'
        '  print(json.dumps({"id":message["id"],"result":value}),flush=True)\n',
        encoding='utf-8',
    )

    async def check():
        async with NativeRpc([sys.executable, str(script)]) as rpc:
            assert_clean(await rpc.request('environment', {}))
        assert rpc.process.returncode is not None
    asyncio.run(check())


@pytest.mark.skipif(not shutil.which('bash') or not shutil.which('setsid'), reason='Requires a POSIX shell and setsid')
def test_sdk_wrapper_filters_after_sdk_environment_merge(inherited_environment, monkeypatch):
    # Exercise the actual POSIX wrapper using Git Bash on Windows as well.
    monkeypatch.setattr(native_common, 'os', types.SimpleNamespace(name='posix', environ=os.environ))
    group = native_common.ClaudeProcessGroup(Path(sys.executable).as_posix())
    try:
        wrapper = group.prepare()
        result = subprocess.run(
            [shutil.which('bash'), Path(wrapper).as_posix(), '-c', SNAPSHOT],
            capture_output=True, timeout=10,
        )
        assert result.returncode == 0, result.stderr
        assert_clean(json.loads(result.stdout))
        assert group.pid_path.is_file()
    finally:
        if group.directory:
            group.directory.cleanup()


@pytest.mark.skipif(not shutil.which('bash') or not shutil.which('env'), reason='Requires a POSIX shell and env')
def test_terminal_plan_filters_metadata_and_preserves_arguments(inherited_environment, tmp_path):
    script = tmp_path / "native cli's environment.py"
    script.write_text(SNAPSHOT + '\nimport sys\nassert sys.argv[1:] == ["argument with spaces", "literal;$HOME"]\n', encoding='utf-8')

    class TerminalAdapter(FakeAdapter):
        async def terminal_command(self):
            return [Path(sys.executable).as_posix(), script.as_posix(), 'argument with spaces', 'literal;$HOME']

    async def check():
        service = HostService(tmp_path / 'host', {'fake': TerminalAdapter()})
        async def send(_):
            pass
        client = Client(send)
        try:
            await service.request(client, 'hello', {'protocolVersion': '1.0', 'client': 'test', 'deviceId': 'device', 'deviceName': 'Desktop', 'os': 'windows'})
            plan = await service.request(client, 'agent.terminal.start', {'agent': 'fake'})
            result = subprocess.run([shutil.which('bash'), '-c', 'exec ' + plan['command']], capture_output=True, timeout=10)
            assert result.returncode == 0, result.stderr
            assert_clean(json.loads(result.stdout))
        finally:
            await service.close()
    asyncio.run(check())


@pytest.mark.skipif(os.name != 'posix', reason='Login PTY requires a Unix Host')
def test_login_pty_environment_is_clean(inherited_environment):
    async def check():
        driver = PtyLogin([sys.executable, '-c', SNAPSHOT])
        try:
            await driver.start()
            output = bytearray()
            async with asyncio.timeout(5):
                while chunk := await driver.read():
                    output.extend(chunk)
            assert_clean(json.loads(output))
        finally:
            await driver.close()
        assert driver.process.returncode is not None
    asyncio.run(check())


def test_remote_run_does_not_receive_desktop_metadata(tmp_path):
    captured = []

    class RecordingAdapter(FakeAdapter):
        async def start_run(self, session, input, ctx):
            captured.append(dict(session))
            return await super().start_run(session, input, ctx)

    async def check():
        service = HostService(tmp_path / 'host', {'fake': RecordingAdapter()})
        async def send(_):
            pass
        client = Client(send)
        try:
            await service.request(client, 'hello', {'protocolVersion': '1.0', 'client': 'test', 'deviceId': 'desktop-sentinel', 'deviceName': 'personal-machine-sentinel', 'os': 'windows'})
            await service.request(client, 'project.upsert', {'id': 'remote', 'path': str(tmp_path), 'name': 'Remote project'})
            session = await service.request(client, 'session.create', {'projectId': 'remote', 'agent': 'fake', 'model': 'fake-default', 'permissionMode': 'default'})
            await service.request(client, 'run.send', {'sessionId': session['id'], 'clientMsgId': 'input', 'text': 'Inspect project', 'script': []})
            if service.watchers:
                await asyncio.wait_for(asyncio.gather(*tuple(service.watchers)), 5)
            assert len(captured) == 1
            assert captured[0]['kind'] == 'remote'
            assert captured[0]['deviceId'] is None
            assert not {'deviceInstructions', 'deviceRoot', 'sessionConfig'} & captured[0].keys()
            assert 'desktop-sentinel' not in repr(captured[0])
            assert 'personal-machine-sentinel' not in repr(captured[0])
            assert not service.devices.entries
        finally:
            await service.close()
    asyncio.run(check())
