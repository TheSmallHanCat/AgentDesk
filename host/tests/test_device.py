import asyncio
import os
from pathlib import Path
import tempfile
import time

import pytest

from agentdesk_host.adapters import registered_adapters
from agentdesk_host.adapters.base import AdapterError, RunContext
from agentdesk_host.adapters.fake import FakeAdapter
from agentdesk_host.device.manager import DeviceManager, project_key
from agentdesk_host.device.proxy import connect
from agentdesk_host.server.service import HostService
from test_host import connected, finish


@pytest.fixture
def device_home():
    # Unix streamlocal paths are limited to 100 bytes on the Host.
    with tempfile.TemporaryDirectory(prefix='ad-') as directory:
        yield Path(directory)


def test_windows_project_key_ignores_case_and_separator():
    assert project_key('one', r'C:\Work\Project\.', 'windows') == project_key('one', 'c:/work/project', 'windows')
    assert project_key('one', 'C:/work/project', 'windows') != project_key('two', 'C:/work/project', 'windows')


def test_registry_has_all_production_adapters_and_explicit_fake():
    assert set(registered_adapters()) == {'claude', 'codex'}
    assert set(registered_adapters(fake=True)) == {'claude', 'codex', 'fake'}


def test_device_probe_ownership_keys_reconnect_and_grace(device_home, monkeypatch):
    from agentdesk_host.device import manager
    class Process:
        returncode = 0
        async def communicate(self, script):
            assert script == b'echo ok\n'
            return b'ok\n', b''
    async def spawn(*args, **kwargs):
        assert args[0] == 'ssh' and args[-1] == 'device-script'
        return Process()
    async def close(process): pass
    monkeypatch.setattr(manager.asyncio, 'create_subprocess_exec', spawn)
    monkeypatch.setattr(manager, 'close_process', close)
    async def check():
        class Capture(FakeAdapter):
            async def start_run(self, session, input, ctx):
                self.session = session
                return await super().start_run(session, input, ctx)
        adapter = Capture()
        service = HostService(device_home, {'fake': adapter}, device_options={'grace': .02, 'wait_timeout': .02})
        client, messages = await connected(service)
        project = await service.request(client, 'project.upsert', {'id': 'local', 'kind': 'local', 'path': 'C:/Work/Project', 'name': 'Project'})
        repeated = await service.request(client, 'project.upsert', {'id': 'local', 'kind': 'local', 'deviceId': client.device_id, 'path': r'c:\work\project\.', 'name': 'Project'})
        assert repeated['nativeRoot'] == project['nativeRoot']
        session = await service.request(client, 'session.create', {'projectId': 'local', 'agent': 'fake', 'model': 'fake-default', 'permissionMode': 'default'})
        sid = session['id']
        assert session['path'] == project['nativeRoot'] and session['projectPath'] == r'C:\Work\Project'
        await service.request(client, 'sync.subscribe', {'sessionId': sid})
        params = {'sessionId': sid, 'clientMsgId': 'before-ready', 'text': 'Inspect', 'script': []}
        with pytest.raises(AdapterError) as error:
            await service.request(client, 'run.send', params)
        assert error.value.code in {'E_TARGET', 'E_DEVICE_NOT_READY'}
        registered = await service.request(client, 'device.register', {})
        await service.request(client, 'device.project.enable', {'projectId': 'local'})
        root = service.devices.entry(client.device_id)
        key = '-----BEGIN OPENSSH PRIVATE KEY-----\npublic-fixture-one\n'
        ready_params = {'generation': registered['generation'], 'privateKey': key, 'hostPublicKey': 'ssh-ed25519 public-fixture', 'user': 'DesktopUser', 'scriptCommand': 'pwsh fixed-runner'}
        assert (await service.request(client, 'device.tunnel.ready', ready_params))['state'] == 'ready'
        assert (root / 'keys/id').read_text() == key
        if os.name != 'nt':
            assert (root / 'keys').stat().st_mode & 0o777 == 0o700
            assert (root / 'keys/id').stat().st_mode & 0o777 == 0o600
        run = await service.request(client, 'run.send', {**params, 'clientMsgId': 'run', 'script': [{'type': 'sleep', 'seconds': 60}]})
        config = Path(adapter.session['sessionConfig'])
        before_config = config.read_text()
        before_prompt = adapter.session['deviceInstructions']
        assert len(before_config.splitlines()) == 1
        assert str(config) in before_prompt and key not in before_prompt
        other, _ = await connected(service, 'other')
        with pytest.raises(AdapterError) as ownership:
            await service.request(other, 'run.send', {**params, 'clientMsgId': 'other'})
        assert ownership.value.code == 'E_TARGET'
        await service.request(client, 'device.offline', {})
        assert not (root / 'keys').exists() and not (root / 'ready').exists()
        assert (root / 'ssh.conf').exists() and config.read_text() == before_config
        await asyncio.sleep(.05)
        await finish(service)
        assert service.db.one('SELECT state,stop_reason FROM runs WHERE id=?', (run['runId'],)) == {'state': 'interrupted', 'stop_reason': 'device_grace_expired'}
        registered = await service.request(client, 'device.register', {})
        await service.request(client, 'device.project.enable', {'projectId': 'local'})
        assert registered['tunnelSocket'] == str(root / 'tunnel.sock')
        await service.request(client, 'device.tunnel.ready', {**ready_params, 'generation': registered['generation'], 'privateKey': key.replace('one', 'two')})
        assert config.read_text() == before_config
        await service.request(client, 'run.send', {**params, 'clientMsgId': 'resumed'})
        await finish(service)
        assert adapter.session['deviceInstructions'] == before_prompt
        assert project['nativeRoot'] == adapter.session['path']
        states = [message['params']['payload']['state'] for message in messages if message.get('method') == 'event' and message['params']['type'] == 'device.state']
        assert states[:3] == ['preparing', 'probing', 'ready']
        assert 'closed' in states
        await service.close()
        assert not (root / 'keys').exists() and Path(project['nativeRoot']).is_dir()
    asyncio.run(check())


def test_probe_failure_never_marks_device_ready(device_home, monkeypatch):
    from agentdesk_host.device import manager
    class Process:
        returncode = 1
        async def communicate(self, script): return b'', b''
    async def spawn(*args, **kwargs): return Process()
    async def close(process): pass
    monkeypatch.setattr(manager.asyncio, 'create_subprocess_exec', spawn)
    monkeypatch.setattr(manager, 'close_process', close)
    async def check():
        states = []
        async def state(device, value, reason): states.append(value)
        async def interrupt(device): pass
        devices = DeviceManager(device_home, state, interrupt)
        owner = object()
        registration = await devices.register('one', owner, 'windows')
        devices.enable_project('one', owner, 'project')
        with pytest.raises(AdapterError) as error:
            await devices.ready('one', owner, {'generation': registration['generation'], 'privateKey': '-----BEGIN OPENSSH PRIVATE KEY-----\nfixture\n', 'hostPublicKey': 'ssh-ed25519 fixture', 'user': 'User', 'scriptCommand': 'pwsh runner'})
        assert error.value.code == 'E_DEVICE_PROBE'
        assert states == ['preparing', 'probing', 'error']
        assert not (devices.entry('one') / 'keys').exists()
        with pytest.raises(AdapterError): devices.require_ready('one', owner, 'project')
        await devices.close()
    asyncio.run(check())


def test_offline_proxy_waits_until_timeout(device_home):
    start = time.monotonic()
    with pytest.raises(TimeoutError):
        connect(device_home / 'tunnel.sock', device_home / 'ready', .03)
    assert time.monotonic() - start >= .03


def test_old_openssh_remotecommand_failure_has_explicit_version_code(device_home, monkeypatch):
    from agentdesk_host.device import manager
    class Process:
        returncode = 255
        async def communicate(self, script):
            return b'', b'Bad configuration option: remotecommand\n'
    async def spawn(*args, **kwargs): return Process()
    async def close(process): pass
    monkeypatch.setattr(manager.asyncio, 'create_subprocess_exec', spawn)
    monkeypatch.setattr(manager, 'close_process', close)
    async def check():
        async def state(*args): pass
        devices = DeviceManager(device_home, state, state)
        owner = object()
        generation = (await devices.register('one', owner, 'windows'))['generation']
        devices.enable_project('one', owner, 'project')
        with pytest.raises(AdapterError) as error:
            await devices.ready('one', owner, {'generation': generation, 'privateKey': '-----BEGIN OPENSSH PRIVATE KEY-----\nfixture\n', 'hostPublicKey': 'ssh-ed25519 fixture', 'user': 'User', 'scriptCommand': 'pwsh runner'})
        assert error.value.code == 'E_DEVICE_SSH_VERSION'
        assert '7.6' in error.value.hint
        assert not (devices.entry('one') / 'keys').exists()
        await devices.close()
    asyncio.run(check())


def test_late_probe_cannot_remove_new_connection_keys(device_home, monkeypatch):
    from agentdesk_host.device import manager
    async def check():
        release, started = asyncio.Event(), asyncio.Event()
        class Process:
            returncode = 0
            def __init__(self, wait): self.wait = wait
            async def communicate(self, script):
                if self.wait:
                    started.set()
                    await release.wait()
                return b'ok\n', b''
        count = 0
        async def spawn(*args, **kwargs):
            nonlocal count
            count += 1
            return Process(count == 1)
        async def close(process): pass
        async def state(*args): pass
        monkeypatch.setattr(manager.asyncio, 'create_subprocess_exec', spawn)
        monkeypatch.setattr(manager, 'close_process', close)
        devices = DeviceManager(device_home, state, state)
        old, new = object(), object()
        generation = (await devices.register('one', old, 'windows'))['generation']
        devices.enable_project('one', old, 'project')
        params = {'privateKey': '-----BEGIN OPENSSH PRIVATE KEY-----\nfixture\n', 'hostPublicKey': 'ssh-ed25519 fixture', 'user': 'User', 'scriptCommand': 'pwsh runner'}
        pending = asyncio.create_task(devices.ready('one', old, {**params, 'generation': generation}))
        await started.wait()
        await devices.offline('one', old)
        new_generation = (await devices.register('one', new, 'windows'))['generation']
        devices.enable_project('one', new, 'project')
        await devices.ready('one', new, {**params, 'generation': new_generation})
        release.set()
        with pytest.raises(AdapterError) as error: await pending
        assert error.value.code == 'E_TARGET'
        assert (devices.entry('one') / 'keys/id').exists()
        assert devices.entries['one']['state'] == 'ready'
        await devices.close()
    asyncio.run(check())


def test_fake_device_tool_uses_ssh_stdin_and_preserves_nonzero_unicode(tmp_path, monkeypatch):
    from agentdesk_host.adapters import fake
    script = "Set-Location -LiteralPath 'C:\\Work'\nforeach ($n in 1..2) {\n Write-Output \"中文 $n\"\n}\nexit 7\n"
    async def check():
        sent, calls, events = [], [], []
        class Input:
            def write(self, data): sent.append(data)
            async def drain(self): pass
            def close(self): pass
        class Process:
            stdin = Input()
            stdout = asyncio.StreamReader()
            async def wait(self): return 7
        process = Process()
        process.stdout.feed_data('中文 1\n中文 2\n'.encode())
        process.stdout.feed_eof()
        async def spawn(*args, **kwargs): calls.append(args); return process
        async def close(value): calls.append('closed')
        async def emit(kind, payload, item_id=None): events.append((kind, payload, item_id))
        async def approve(*args): return {'behavior': 'allow'}
        async def save(*args): pass
        monkeypatch.setattr(fake.asyncio, 'create_subprocess_exec', spawn)
        monkeypatch.setattr(fake, 'close_process', close)
        adapter = FakeAdapter()
        with pytest.raises(AdapterError):
            await adapter.start_run({'path': str(tmp_path)}, {'script': [{'type': 'device', 'script': script}]}, RunContext('run', emit, approve, save))
        handle = await adapter.start_run({'kind': 'local', 'sessionConfig': '/session-ssh/stable.conf'}, {'script': [{'type': 'device', 'script': script}]}, RunContext('run', emit, approve, save))
        await handle.task
        assert sent == [script.encode()]
        assert calls == [('ssh', '-F', '/session-ssh/stable.conf', 'device-script'), 'closed']
        result = events[-1][1]
        assert result['output'] == '中文 1\n中文 2\n' and result['exitCode'] == 7 and result['isError']
    asyncio.run(check())
