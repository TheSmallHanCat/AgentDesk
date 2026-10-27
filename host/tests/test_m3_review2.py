"""Task 07 regressions without credentials or model requests."""
import asyncio
import json

import pytest

from agentdesk_host.adapters.base import AdapterError, RunHandle
from agentdesk_host.adapters.claude import ClaudeAdapter
from agentdesk_host.adapters.codex import CodexAdapter
from agentdesk_host.adapters.fake import FakeAdapter
from agentdesk_host.device.manager import DeviceManager
from agentdesk_host.server.service import HostService
from test_claude_replay import ReplayClient, sdk_stub, result
from test_device import device_home
from test_host import connected, finish, setup


@pytest.fixture
def probe(monkeypatch):
    from agentdesk_host.device import manager
    class Process:
        returncode = 0
        async def communicate(self, script): return b'ok\n', b''
    async def spawn(*args, **kwargs): return Process()
    async def close(*args): pass
    monkeypatch.setattr(manager.asyncio, 'create_subprocess_exec', spawn)
    monkeypatch.setattr(manager, 'close_process', close)


async def local_pair(home, same_path=False):
    service = HostService(home, {'fake': FakeAdapter()})
    client, messages = await connected(service)
    sessions = []
    for project in ['one', 'two']:
        await service.request(client, 'project.upsert', {'id': project, 'kind': 'local', 'path': 'C:/Work/' + ('same' if same_path else project), 'name': project})
        sessions.append(await service.request(client, 'session.create', {'projectId': project, 'agent': 'fake', 'model': 'fake-default', 'permissionMode': 'default'}))
    await ready(service, client, ['one', 'two'])
    return service, client, messages, sessions


async def ready(service, client, projects):
    reg = await service.request(client, 'device.register', {})
    for project in projects:
        await service.request(client, 'device.project.enable', {'projectId': project})
    await service.request(client, 'device.tunnel.ready', {'generation': reg['generation'], 'privateKey': '-----BEGIN OPENSSH PRIVATE KEY-----\nfixture\n', 'hostPublicKey': 'ssh-ed25519 fixture', 'user': 'User', 'scriptCommand': 'pwsh runner'})


async def running(service, client, sid, msg):
    return await service.request(client, 'run.send', {'sessionId': sid, 'clientMsgId': msg, 'text': 'Inspect', 'script': [{'type': 'sleep', 'seconds': 60}]})


def test_revoke_stops_only_project_runs(device_home, probe):
    async def check():
        service, client, messages, sessions = await local_pair(device_home)
        try:
            runs = [await running(service, client, s['id'], s['id']) for s in sessions]
            await service.request(client, 'device.project.revoke', {'projectId': 'one'})
            await asyncio.sleep(.01)
            assert service.db.one('SELECT state FROM runs WHERE id=?', (runs[0]['runId'],))['state'] == 'interrupted'
            assert service.db.one('SELECT state FROM runs WHERE id=?', (runs[1]['runId'],))['state'] == 'running'
            assert not (device_home / 'session-ssh' / (sessions[0]['id'] + '.conf')).exists()
            assert (service.devices.entry(client.device_id) / 'keys/id').exists()
            event = service.db.one("SELECT payload_json FROM events WHERE run_id=? AND type='run.finished'", (runs[0]['runId'],))
            assert json.loads(event['payload_json'])['reason'] == '本机授权已撤销'
        finally:
            await service.close()
    asyncio.run(check())


def test_offline_and_reconnect_only_b_never_restore_a(device_home, probe):
    async def check():
        service, client, messages, sessions = await local_pair(device_home)
        try:
            runs = [await running(service, client, s['id'], s['id']) for s in sessions]
            await service.request(client, 'device.offline', {})
            assert not list((device_home / 'session-ssh').glob('*.conf'))
            await ready(service, client, ['two'])
            await asyncio.sleep(.01)
            assert service.db.one('SELECT state FROM runs WHERE id=?', (runs[0]['runId'],))['state'] == 'interrupted'
            assert service.db.one('SELECT state FROM runs WHERE id=?', (runs[1]['runId'],))['state'] == 'running'
            assert not (device_home / 'session-ssh' / (sessions[0]['id'] + '.conf')).exists()
            assert any(m.get('method') == 'device.state' and m['params']['state'] == 'closed' for m in messages)
        finally:
            await service.close()
    asyncio.run(check())


def test_same_path_projects_keep_independent_grants(device_home, probe):
    async def check():
        service, client, _, sessions = await local_pair(device_home, True)
        try:
            reply = await service.request(client, 'device.project.revoke', {'projectId': 'one'})
            assert reply['state'] == 'ready'
            await service.request(client, 'run.send', {'sessionId': sessions[1]['id'], 'clientMsgId': 'two', 'text': 'Inspect', 'script': []})
            await finish(service)
        finally:
            await service.close()
    asyncio.run(check())


def test_revoke_validates_all_ids_before_mutation(device_home):
    async def check():
        async def noop(*args, **kwargs): pass
        manager = DeviceManager(device_home, noop, noop)
        owner = object()
        await manager.register('device', owner, 'windows')
        manager.enable_project('device', owner, 'p')
        root = device_home / 'session-ssh'
        root.mkdir()
        (root / 'valid.conf').write_text('keep')
        with pytest.raises(AdapterError):
            await manager.revoke_project('device', owner, 'p', ('valid', '../invalid'))
        assert (root / 'valid.conf').exists()
        assert 'p' in manager.owned('device', owner)['projects']
        await manager.close()
    asyncio.run(check())


@pytest.mark.parametrize('ending', ['stop', 'error', 'exit', 'emit'])
def test_claude_closes_input_before_disconnect_and_reports_undelivered(monkeypatch, tmp_path, ending):
    async def check():
        started, release, disconnecting, disconnected = (asyncio.Event() for _ in range(4))
        class Client(ReplayClient):
            async def receive_messages(self):
                started.set()
                await release.wait()
                if ending == 'exit': return
                message = result()
                message.is_error = ending == 'error'
                yield message
            async def interrupt(self): release.set()
            async def disconnect(self):
                disconnecting.set()
                await disconnected.wait()
        sdk = Client([])
        sdk_stub(monkeypatch, sdk)
        service, client, _, sid = await setup(tmp_path / 'home', tmp_path)
        adapter = ClaudeAdapter()
        service.adapters['fake'] = adapter
        async def start(session, input, ctx):
            native = {'client': None, 'stopping': False}
            if ending == 'emit':
                emit = ctx.emit
                async def failing(kind, payload, item_id=None):
                    if kind == 'usage': raise RuntimeError('emit failed')
                    await emit(kind, payload, item_id)
                ctx.emit = failing
            return RunHandle(ctx.run_id, asyncio.create_task(adapter._run(session, input, ctx, native)), native)
        adapter.start_run = start
        try:
            run = await service.request(client, 'run.send', {'sessionId': sid, 'clientMsgId': 'first', 'text': 'First'})
            await started.wait()
            await service.request(client, 'run.append', {'sessionId': sid, 'runId': run['runId'], 'clientMsgId': 'queued', 'text': 'Queued'})
            if ending == 'stop':
                stopping = asyncio.create_task(service.request(client, 'run.stop', {'sessionId': sid, 'runId': run['runId']}))
            else:
                release.set()
            await asyncio.wait_for(disconnecting.wait(), 2)
            with pytest.raises(AdapterError):
                await adapter.append_input(service.handles[run['runId']][1], 'Late')
            disconnected.set()
            if ending == 'stop': await stopping
            await finish(service)
            assert [text for text, _ in sdk.queries] == ['First']
            rows = service.db.all("SELECT payload_json FROM events WHERE run_id=? AND type='item.completed'", (run['runId'],))
            notices = [json.loads(row['payload_json']) for row in rows if json.loads(row['payload_json']).get('delivery') == 'undelivered']
            assert len(notices) == 1 and notices[0]['clientMsgId'] == 'queued'
        finally:
            disconnected.set()
            await service.close()
    asyncio.run(check())


def test_startup_append_is_retryable_and_codex_stop_blocks_steer(tmp_path):
    async def check():
        task = asyncio.create_task(asyncio.sleep(60))
        try:
            for adapter, native in [(ClaudeAdapter(), {'client': None}), (CodexAdapter(), {'rpc': None})]:
                with pytest.raises(AdapterError) as error:
                    await adapter.append_input(RunHandle('r', task, native), 'Append')
                assert error.value.code == 'E_APPEND_NOT_READY'
            class Rpc:
                async def request(self, *args, **kwargs): raise AssertionError('steer after stop')
            with pytest.raises(AdapterError):
                await CodexAdapter().append_input(RunHandle('r', task, {'rpc': Rpc(), 'turnId': 't', 'stopping': True}), 'Append')
        finally:
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
    asyncio.run(check())


def test_fake_append_then_stop_never_executes_queued_input(tmp_path):
    async def check():
        service, client, _, sid = await setup(tmp_path / 'home', tmp_path)
        try:
            run = await service.request(client, 'run.send', {'sessionId': sid, 'clientMsgId': 'first', 'text': 'First', 'script': [{'type': 'sleep', 'seconds': 60}, {'type': 'wait_input'}]})
            await service.request(client, 'run.append', {'sessionId': sid, 'runId': run['runId'], 'clientMsgId': 'queued', 'text': 'Queued'})
            await service.request(client, 'run.stop', {'sessionId': sid, 'runId': run['runId']})
            await finish(service)
            rows = [json.loads(row['payload_json']) for row in service.db.all("SELECT payload_json FROM events WHERE run_id=? AND type='item.completed'", (run['runId'],))]
            assert not any(row.get('kind') == 'text' and row.get('text') == 'Queued' for row in rows)
            assert sum(row.get('delivery') == 'undelivered' for row in rows) == 1
        finally:
            await service.close()
    asyncio.run(check())


@pytest.mark.parametrize('path', ['//./C:/Work', chr(92) + '??' + chr(92) + 'C:/Work'])
def test_windows_namespace_aliases_are_rejected(path):
    from agentdesk_host.device.manager import project_key
    with pytest.raises(AdapterError) as error:
        project_key('device', path, 'windows')
    assert error.value.code == 'E_INVALID_PARAMS'


def test_revoke_and_enable_are_serialized_without_losing_new_grant(device_home, probe):
    async def check():
        service, client, _, sessions = await local_pair(device_home)
        entered, release = asyncio.Event(), asyncio.Event()
        original = service.devices.interrupt
        async def paused(*args, **kwargs):
            entered.set()
            await release.wait()
            await original(*args, **kwargs)
        service.devices.interrupt = paused
        try:
            revoking = asyncio.create_task(service.request(client, 'device.project.revoke', {'projectId': 'one'}))
            await entered.wait()
            enabling = asyncio.create_task(service.request(client, 'device.project.enable', {'projectId': 'one'}))
            await asyncio.sleep(0)
            assert not enabling.done()
            release.set()
            await asyncio.gather(revoking, enabling)
            service.devices.require_ready(client.device_id, client, 'one')
            assert service.devices.owned(client.device_id, client)['projects'] == {'one', 'two'}
        finally:
            release.set()
            await service.close()
    asyncio.run(check())


def test_append_native_timeout_is_unknown_and_must_not_trigger_new_send(tmp_path):
    async def check():
        service, client, _, sid = await setup(tmp_path / 'home', tmp_path)
        async def timeout(*args):
            raise AdapterError('E_TIMEOUT', 'codex.turn/steer', 'Native response timed out')
        service.adapters['fake'].append_input = timeout
        try:
            run = await service.request(client, 'run.send', {'sessionId': sid, 'clientMsgId': 'first', 'text': 'First', 'script': [{'type': 'sleep', 'seconds': 60}]})
            with pytest.raises(AdapterError) as error:
                await service.request(client, 'run.append', {'sessionId': sid, 'runId': run['runId'], 'clientMsgId': 'uncertain', 'text': 'Queued'})
            assert error.value.code == 'E_DELIVERY_UNKNOWN'
            assert service.db.one("SELECT * FROM client_msgs WHERE client_msg_id='uncertain'") is None
            assert service.db.one('SELECT state FROM runs WHERE id=?', (run['runId'],))['state'] == 'running'
        finally:
            await service.close()
    asyncio.run(check())
