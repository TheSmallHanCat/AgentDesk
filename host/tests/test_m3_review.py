"""M3 review regressions; no credentials or model access."""
import asyncio
from pathlib import Path
import types

import pytest

from agentdesk_host.adapters.base import AdapterError, RunContext
from agentdesk_host.adapters.claude import ClaudeAdapter
from agentdesk_host.adapters.codex import CodexAdapter
from agentdesk_host.device.manager import DeviceManager, project_key
from agentdesk_host.server.service import HostService
from test_device import device_home
from test_host import connected, finish
from test_m2_review import setup


@pytest.mark.parametrize('adapter', [ClaudeAdapter(), CodexAdapter('codex')])
def test_local_plan_is_rejected_before_any_native_process(adapter):
    async def check():
        async def noop(*args): pass
        context = RunContext('run', noop, noop, noop)
        for entry in ['start_run', '_run']:
            with pytest.raises(AdapterError) as error:
                if entry == 'start_run':
                    await adapter.start_run({'kind': 'local', 'permissionMode': 'plan'}, {'text': 'Inspect'}, context)
                else:
                    await adapter._run({'kind': 'local', 'permissionMode': 'plan'}, {'text': 'Inspect'}, context, {})
            assert error.value.code == 'E_LOCAL_PLAN_UNSUPPORTED'
    asyncio.run(check())


def test_project_key_extended_paths_and_original_case(device_home):
    prefix = chr(92) * 2 + '?' + chr(92)
    assert project_key('one', prefix + 'C:/Work/Project', 'windows') == project_key('one', 'c:/work/project', 'windows')
    assert project_key('one', prefix + 'UNC/Server/Share/Work', 'windows') == project_key('one', '//server/share/work', 'windows')
    async def check():
        service = HostService(device_home, {})
        client, _ = await connected(service)
        params = {'id': 'local', 'kind': 'local', 'path': prefix + 'C:/Work/Project', 'name': 'Project'}
        project = await service.request(client, 'project.upsert', params)
        again = await service.request(client, 'project.upsert', {**params, 'path': 'c:/work/project'})
        assert project['path'] == again['path'] == r'C:\Work\Project'
        assert project['nativeRoot'] == again['nativeRoot']
        await service.close()
    asyncio.run(check())


def test_project_authorization_revoke_and_reconnect_takeover(device_home, monkeypatch):
    from agentdesk_host.device import manager
    class Process:
        returncode = 0
        async def communicate(self, script): return b'ok\n', b''
    async def spawn(*args, **kwargs): return Process()
    async def close(*args): pass
    monkeypatch.setattr(manager.asyncio, 'create_subprocess_exec', spawn)
    monkeypatch.setattr(manager, 'close_process', close)
    async def check():
        from agentdesk_host.adapters.fake import FakeAdapter
        service = HostService(device_home, {'fake': FakeAdapter()})
        client, _ = await connected(service)
        sessions = []
        for id in ['one', 'two']:
            await service.request(client, 'project.upsert', {'id': id, 'kind': 'local', 'path': 'C:/Work/' + id, 'name': id})
            with pytest.raises(AdapterError) as error:
                await service.request(client, 'session.create', {'projectId': id, 'agent': 'fake', 'model': 'fake-default', 'permissionMode': 'plan'})
            assert error.value.code == 'E_LOCAL_PLAN_UNSUPPORTED'
            sessions.append(await service.request(client, 'session.create', {'projectId': id, 'agent': 'fake', 'model': 'fake-default', 'permissionMode': 'default'}))
        reg = await service.request(client, 'device.register', {})
        ready = {'generation': reg['generation'], 'privateKey': '-----BEGIN OPENSSH PRIVATE KEY-----\nfixture\n', 'hostPublicKey': 'ssh-ed25519 fixture', 'user': 'User', 'scriptCommand': 'pwsh runner'}
        root = service.devices.entry(client.device_id)
        with pytest.raises(AdapterError): await service.request(client, 'device.tunnel.ready', ready)
        assert not (root / 'keys').exists()
        await service.request(client, 'device.project.enable', {'projectId': 'one'})
        await service.request(client, 'device.tunnel.ready', ready)
        sid = sessions[1]['id']
        with pytest.raises(AdapterError) as error:
            await service.request(client, 'run.send', {'sessionId': sid, 'clientMsgId': 'unauthorized', 'text': 'Inspect'})
        assert error.value.code == 'E_DEVICE_UNAUTHORIZED'
        assert not (device_home / 'session-ssh' / (sid + '.conf')).exists()
        for id, session in zip(['one', 'two'], sessions):
            await service.request(client, 'device.project.enable', {'projectId': id})
            await service.request(client, 'run.send', {'sessionId': session['id'], 'clientMsgId': id, 'text': 'Inspect', 'script': []})
            await finish(service)
        await service.request(client, 'device.project.revoke', {'projectId': 'one'})
        assert (root / 'keys/id').exists()
        assert not (device_home / 'session-ssh' / (sessions[0]['id'] + '.conf')).exists()
        with pytest.raises(AdapterError):
            await service.request(client, 'run.send', {'sessionId': sessions[0]['id'], 'clientMsgId': 'blocked', 'text': 'Inspect'})
        await service.request(client, 'device.project.revoke', {'projectId': 'two'})
        assert not (root / 'keys').exists() and not (root / 'ready').exists()
        old = await service.request(client, 'device.register', {})
        # Simulate a blackholed old control connection which is still alive.
        replacement, _ = await connected(service)
        fresh = await service.request(replacement, 'device.register', {})
        assert fresh['generation'] != old['generation']
        await service.request(client, 'device.offline', {'generation': old['generation']})
        assert service.devices.entries[client.device_id]['owner'] is replacement
        await service.close()
    asyncio.run(check())


def test_failed_append_is_not_persisted_or_replayed_as_accepted(tmp_path):
    async def check():
        service, client, messages, sid = await setup(tmp_path / 'home', tmp_path)
        run = await service.request(client, 'run.send', {'sessionId': sid, 'clientMsgId': 'first', 'text': 'Start', 'script': [{'type': 'sleep', 'seconds': .03}]})
        adapter, handle = service.handles[run['runId']]
        async def reject(*args):
            raise AdapterError('E_SESSION_BUSY', 'claude.append', 'Run has finished receiving input')
        adapter.append_input = reject
        params = {'sessionId': sid, 'runId': run['runId'], 'clientMsgId': 'late', 'text': 'Continue'}
        for _ in range(2):
            with pytest.raises(AdapterError) as error: await service.request(client, 'run.append', params)
            assert error.value.code == 'E_APPEND_CLOSED'
        assert not handle.task.cancelled()
        assert service.db.one("SELECT * FROM client_msgs WHERE client_msg_id='late'") is None
        assert len(service.db.all("SELECT * FROM events WHERE type='user.message'")) == 1
        await finish(service)
        assert service.db.one('SELECT state FROM runs WHERE id=?', (run['runId'],))['state'] == 'completed'
        await service.close()
    asyncio.run(check())


def test_claude_serializes_queued_inputs_until_cli_idle(monkeypatch, tmp_path):
    from test_claude_replay import ReplayClient, sdk_stub, result, context
    from agentdesk_host.adapters.base import RunHandle
    async def check():
        started, release = asyncio.Event(), asyncio.Event()
        class Client(ReplayClient):
            async def receive_messages(self):
                started.set()
                await release.wait()
                # A CLI that would coalesce concurrent queries receives one only.
                assert len(self.queries) == 1
                yield result()
                assert len(self.queries) == 2
                yield result()
                assert len(self.queries) == 3
                yield result()
        client = Client([])
        sdk_stub(monkeypatch, client)
        native = {'client': None, 'stopping': False}
        adapter = ClaudeAdapter()
        task = asyncio.create_task(adapter._run({'path': str(tmp_path)}, {'text': 'First'}, await context([]), native))
        await started.wait()
        handle = RunHandle('run', task, native)
        await adapter.append_input(handle, 'Second')
        await adapter.append_input(handle, 'Third')
        assert len(client.queries) == 1
        release.set()
        await asyncio.wait_for(task, 2)
        assert [text for text, _ in client.queries] == ['First', 'Second', 'Third']
        with pytest.raises(AdapterError): await adapter.append_input(handle, 'Late')
    asyncio.run(check())
