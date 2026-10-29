"""Isolation policy and real stdio-worker tests without containers or model access."""
import asyncio
import json
import os
from pathlib import Path
import sqlite3
import sys
from types import SimpleNamespace

import pytest

from agentdesk_host.adapters.base import AdapterError
from agentdesk_host.adapters.fake import FakeAdapter
from agentdesk_host.runtime.adapter import IsolatedAdapter, RuntimePool
from agentdesk_host.runtime.client import RuntimeClient
from agentdesk_host.runtime.engine import OWNER_LABEL, RUNTIME_LABEL, PodmanEngine, validate_project
from agentdesk_host.server.service import Client, HostService
from agentdesk_host.store.database import Database, MIGRATIONS

IDENTITY = 'a' * 64
RUNTIME = 'rt_' + 'b' * 32


def engine_fixture(tmp_path, monkeypatch):
    monkeypatch.setattr(os, 'getuid', lambda: 1000, raising=False)
    monkeypatch.setattr(os, 'getgid', lambda: 1000, raising=False)
    home, project = tmp_path / 'host', tmp_path / 'project'
    home.mkdir(); project.mkdir()
    engine = PodmanEngine(home, 'host-test', {'image': 'sha256:' + IDENTITY})
    engine.image_identity = IDENTITY
    data = engine.directory(RUNTIME)
    record = {
        'Image': IDENTITY,
        'Config': {'Labels': {OWNER_LABEL: 'host-test', RUNTIME_LABEL: RUNTIME}, 'User': '1000:1000'},
        'HostConfig': {'Privileged': False, 'ReadonlyRootfs': True, 'NetworkMode': 'private', 'PidMode': 'private', 'IpcMode': 'private', 'UTSMode': 'private', 'UsernsMode': 'private', 'SecurityOpt': ['no-new-privileges'], 'CapAdd': [], 'PidsLimit': 256, 'Memory': 2 * 1024**3, 'NanoCpus': 2_000_000_000, 'LogConfig': {'Type': 'none'}},
        'Mounts': [{'Type': 'bind', 'Source': str(project), 'Destination': '/workspace', 'RW': True}, {'Type': 'bind', 'Source': str(data), 'Destination': '/home/agent', 'RW': True}],
    }
    return engine, project, data, record


def test_launch_policy_is_fixed_and_does_not_pull_or_expose_host(tmp_path, monkeypatch):
    engine, project, data, record = engine_fixture(tmp_path, monkeypatch)
    args = engine.create_args('agentdesk-' + 'c' * 32, RUNTIME, project, data)
    assert '--pull=never' in args and '--read-only' in args and '--cap-drop=all' in args
    assert '--security-opt=no-new-privileges' in args and '--network=private' in args
    assert '--pid=private' in args and '--userns=keep-id' in args
    assert args.count('--mount') == 2
    assert '--privileged=false' in args and '--cgroupns=private' in args
    assert '--http-proxy=false' in args and '--env-host=false' in args
    assert not any('host.sock' in value or 'devices/' in value or 'network=host' in value or value in ('--privileged', '--privileged=true') for value in args)
    engine.verify(record, RUNTIME, project, data)


@pytest.mark.parametrize('change', ['root', 'writable-root', 'host-network', 'host-pid', 'host-ipc', 'host-user', 'wrong-user', 'image', 'mount', 'duplicate-mount', 'ports', 'limits', 'cpu', 'logs', 'privileges'])
def test_inspection_rejects_drift_before_worker_execution(tmp_path, monkeypatch, change):
    engine, project, data, record = engine_fixture(tmp_path, monkeypatch)
    if change == 'root': record['HostConfig']['Privileged'] = True
    if change == 'writable-root': record['HostConfig']['ReadonlyRootfs'] = False
    if change == 'host-network': record['HostConfig']['NetworkMode'] = 'host'
    if change == 'host-pid': record['HostConfig']['PidMode'] = 'host'
    if change == 'host-ipc': record['HostConfig']['IpcMode'] = 'host'
    if change == 'host-user': record['HostConfig']['UsernsMode'] = 'host'
    if change == 'wrong-user': record['Config']['User'] = '0:0'
    if change == 'image': record['Image'] = 'd' * 64
    if change == 'mount': record['Mounts'].append({'Type': 'bind', 'Source': str(engine.home), 'Destination': '/control', 'RW': True})
    if change == 'duplicate-mount': record['Mounts'].append(record['Mounts'][0].copy())
    if change == 'ports': record['HostConfig']['PortBindings'] = {'22/tcp': []}
    if change == 'limits': record['HostConfig']['Memory'] = 0
    if change == 'cpu': record['HostConfig']['NanoCpus'] = 0
    if change == 'logs': record['HostConfig']['LogConfig']['Type'] = 'journald'
    if change == 'privileges': record['HostConfig']['SecurityOpt'] = []
    with pytest.raises(AdapterError):
        engine.verify(record, RUNTIME, project, data)


def test_project_mount_cannot_contain_control_data_or_external_git_metadata(tmp_path):
    home = tmp_path / 'host'; home.mkdir()
    project = tmp_path / 'project'; project.mkdir()
    assert validate_project(str(project), home) == project.resolve()
    for path in (home, tmp_path):
        with pytest.raises(AdapterError): validate_project(str(path), home)
    (project / '.git').write_text('gitdir: ../outside')
    with pytest.raises(AdapterError): validate_project(str(project), home)


@pytest.mark.parametrize('rootless,cgroup,expected', [(False, 'v2', 'E_RUNTIME_ROOTLESS'), (True, 'v1', 'E_RUNTIME_LIMITS'), (True, 'v2', None)])
def test_status_requires_rootless_resource_support(tmp_path, rootless, cgroup, expected):
    engine = PodmanEngine(tmp_path, 'host', {'image': 'sha256:' + IDENTITY})
    async def invoke(*args):
        if args[0] == 'info':
            return json.dumps({'host': {'security': {'rootless': rootless}, 'cgroupVersion': cgroup}}).encode()
        assert args == ('image', 'inspect', 'sha256:' + IDENTITY)
        return json.dumps([{'Id': IDENTITY}]).encode()
    engine.invoke = invoke
    result = asyncio.run(engine.status())
    assert result['supported'] is (expected is None)
    if expected: assert result['reason'] == expected


def test_missing_image_fails_before_any_runtime_command(tmp_path):
    engine = PodmanEngine(tmp_path, 'host', {'image': 'repository:latest'})
    async def unexpected(*args):
        raise AssertionError('Must not invoke Podman')
    engine.invoke = unexpected
    assert asyncio.run(engine.status())['reason'] == 'E_RUNTIME_IMAGE'


class FakeEngine:
    def __init__(self): self.removed = []
    async def remove(self, name): self.removed.append(name)
    async def close(self): pass
    async def status(self): return {'supported': True, 'state': 'configured', 'agents': ['fake']}


class AttachedClient(RuntimeClient):
    """Only the container launch/probe is substituted; worker RPC is a real process."""
    def __init__(self, engine, runtime_id, path, script):
        super().__init__(engine, runtime_id, path)
        self.script = script
    async def ensure(self):
        async with self.lock:
            if self.closing:
                raise AdapterError('E_RUNTIME_EXITED', 'test', 'Closed')
            if self.process: return
            self.name = self.runtime_id
            self.process = await asyncio.create_subprocess_exec(
                sys.executable, '-m', 'agentdesk_host.runtime.worker', '--fake',
                stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env={**os.environ, 'PYTHONPATH': str(Path(__file__).resolve().parents[1])},
                start_new_session=os.name == 'posix',
            )
            self.reader = asyncio.create_task(self.read())
            assert await self.exchange('probe', {}) == {'protocol': 1}
    async def request(self, method, params):
        if method == 'run.start': params = {**params, 'script': self.script}
        return await super().request(method, params)


class TestAdapter(IsolatedAdapter):
    __test__ = False
    async def capabilities(self):
        return await self.client.request('capabilities', {'agent': self.agent})


class TestPool(RuntimePool):
    __test__ = False
    def __init__(self, script):
        self.engine = FakeEngine()
        self.clients = {}
        self.launch_lock = asyncio.Lock()
        self.closing = False
        self.script = script
    def client(self, project):
        if project['runtimeId'] not in self.clients:
            self.clients[project['runtimeId']] = AttachedClient(self.engine, project['runtimeId'], project['path'], self.script)
        return self.clients[project['runtimeId']]
    def adapter(self, project, agent): return TestAdapter(self.client(project), agent)


async def connected(service):
    messages = []
    async def send(message): messages.append(message)
    owner = Client(send)
    await service.request(owner, 'hello', {'protocolVersion': '1.0', 'deviceId': 'desktop', 'deviceName': 'Personal device', 'os': 'windows', 'client': 'test'})
    return owner, messages


def test_project_and_session_runtime_are_immutable_and_old_projects_stay_direct(tmp_path):
    async def check():
        project = tmp_path / 'project'; project.mkdir()
        service = HostService(tmp_path / 'host', {'fake': FakeAdapter()})
        owner, _ = await connected(service)
        try:
            params = {'id': 'direct', 'path': str(project), 'name': 'Project'}
            direct = await service.request(owner, 'project.upsert', params)
            assert direct['runtimeMode'] == 'direct' and direct['runtimeId'] is None
            with pytest.raises(AdapterError): await service.request(owner, 'project.upsert', {**params, 'runtimeMode': 'isolated'})
            isolated = await service.request(owner, 'project.upsert', {**params, 'id': 'isolated', 'runtimeMode': 'isolated'})
            repeated = await service.request(owner, 'project.upsert', {**params, 'id': 'isolated', 'runtimeMode': 'isolated', 'runtimeId': 'attacker'})
            assert repeated['runtimeId'] == isolated['runtimeId']
            with pytest.raises(AdapterError): await service.request(owner, 'project.upsert', {**params, 'id': 'local', 'kind': 'local', 'runtimeMode': 'isolated'})
            caps = await service.request(owner, 'agent.status', {'projectId': 'isolated'})
            assert not caps['fake']['available']
            with pytest.raises(AdapterError):
                await service.request(owner, 'session.create', {'projectId': 'isolated', 'agent': 'fake', 'model': 'fake-default', 'permissionMode': 'default'})
            session = await service.request(owner, 'session.create', {'projectId': 'direct', 'agent': 'fake', 'model': 'fake-default', 'permissionMode': 'default'})
            assert session['runtimeId'] is None
            service.db.execute('UPDATE projects SET runtime_id=? WHERE id=?', ('rt_' + 'e' * 32, 'direct'))
            with pytest.raises(AdapterError): service._session(session['id'])
        finally: await service.close()
    asyncio.run(check())


@pytest.mark.parametrize('stop', [False, True])
def test_real_worker_roundtrip_preserves_events_approvals_native_identity_and_stop(tmp_path, stop):
    async def check():
        project = tmp_path / 'project'; project.mkdir()
        service = HostService(tmp_path / 'host', {'fake': FakeAdapter()})
        service.runtimes = TestPool([{'type': 'sleep', 'seconds': 60}] if stop else [{'type': 'text', 'text': 'ready'}, {'type': 'approval', 'tool': 'Read'}])
        owner, messages = await connected(service)
        try:
            registered = await service.request(owner, 'project.upsert', {'id': 'project', 'path': str(project), 'name': 'Project', 'runtimeMode': 'isolated'})
            session = await service.request(owner, 'session.create', {'projectId': 'project', 'agent': 'fake', 'model': 'fake-default', 'permissionMode': 'default'})
            assert session['runtimeId'] == registered['runtimeId']
            await service.request(owner, 'sync.subscribe', {'sessionId': session['id']})
            run = await service.request(owner, 'run.send', {'sessionId': session['id'], 'clientMsgId': 'input', 'text': 'Inspect'})
            if stop:
                for method in ('login.start', 'agent.logout'):
                    with pytest.raises(AdapterError) as error:
                        await service.request(owner, method, {'projectId': 'project', 'agent': 'fake'})
                    assert error.value.code == 'E_SESSION_BUSY'
                await service.request(owner, 'run.stop', {'sessionId': session['id'], 'runId': run['runId']})
            else:
                async with asyncio.timeout(5):
                    while not (approval := service.db.one("SELECT id FROM approvals WHERE state='pending'")):
                        await asyncio.sleep(.01)
                await service.request(owner, 'approval.respond', {'sessionId': session['id'], 'approvalId': approval['id'], 'decision': {'behavior': 'deny'}})
            await asyncio.wait_for(asyncio.gather(*tuple(service.watchers)), 5)
            assert service._session(session['id'])['nativeId'].startswith('fake_')
            state = service.db.one('SELECT state FROM runs WHERE id=?', (run['runId'],))['state']
            assert state == ('interrupted' if stop else 'completed')
            if not stop: assert any(m.get('params', {}).get('type') == 'item.completed' for m in messages)
            with pytest.raises(AdapterError):
                await service.request(owner, 'agent.terminal.start', {'projectId': 'project', 'agent': 'fake'})
        finally:
            clients = list(service.runtimes.clients.values())
            await service.close()
            assert all(c.process.returncode is not None for c in clients)
    asyncio.run(check())


def test_worker_rejects_host_paths_and_unknown_operations(tmp_path):
    async def check():
        client = AttachedClient(FakeEngine(), RUNTIME, str(tmp_path), [])
        try:
            with pytest.raises(AdapterError) as error:
                await client.request('native.exists', {'session': {'id': 's', 'agent': 'fake', 'model': 'fake-default', 'permissionMode': 'default', 'path': '/host/private'}})
            assert error.value.code == 'E_TARGET'
            with pytest.raises(AdapterError): await client.request('device.register', {})
        finally: await client.close()
    asyncio.run(check())


def test_runtime_login_notifications_remain_owned_and_transient():
    async def check():
        client = RuntimeClient(FakeEngine(), RUNTIME, '/project')
        one, two = [], []
        async def send_one(value): one.append(value)
        async def send_two(value): two.append(value)
        owner, stranger = Client(send_one), Client(send_two)
        async def request(method, params):
            await client.notification({'method': 'login.prompt', 'params': {'agent': 'claude', 'loginId': 'login', 'url': 'https://example.invalid/authorization'}})
            return {'agent': 'claude', 'loginId': 'login'}
        client.request = request
        await client.login_start(owner, 'claude')
        pool = TestPool([]); pool.clients[RUNTIME] = client
        assert pool.login('login', owner) is client
        with pytest.raises(AdapterError): pool.login('login', stranger)
        await client.notification({'method': 'login.state', 'params': {'agent': 'claude', 'loginId': 'login', 'state': 'cancelled'}})
        assert not client.logins and not two and len(one) == 2
        await client.close()
    asyncio.run(check())


def test_worker_exit_keeps_accepted_append_delivery_unknown(tmp_path):
    async def check():
        project = tmp_path / 'project'; project.mkdir()
        service = HostService(tmp_path / 'host', {'fake': FakeAdapter()})
        service.runtimes = TestPool([{'type': 'sleep', 'seconds': 60}])
        owner, _ = await connected(service)
        try:
            await service.request(owner, 'project.upsert', {'id': 'project', 'path': str(project), 'name': 'Project', 'runtimeMode': 'isolated'})
            session = await service.request(owner, 'session.create', {'projectId': 'project', 'agent': 'fake', 'model': 'fake-default', 'permissionMode': 'default'})
            run = await service.request(owner, 'run.send', {'sessionId': session['id'], 'clientMsgId': 'first', 'text': 'Inspect'})
            await service.request(owner, 'run.append', {'sessionId': session['id'], 'runId': run['runId'], 'clientMsgId': 'second', 'text': 'Queued input'})
            worker = next(iter(service.runtimes.clients.values()))
            worker.process.terminate()
            await asyncio.wait_for(asyncio.gather(*tuple(service.watchers)), 5)
            notices = [json.loads(row['payload_json']) for row in service.db.all("SELECT payload_json FROM events WHERE type='item.completed'")]
            pending = [row for row in notices if row.get('clientMsgId') == 'second']
            assert len(pending) == 1 and pending[0]['delivery'] == 'unknown'
        finally: await service.close()
    asyncio.run(check())


def test_handshake_does_not_launch_agents_outside_the_requested_scope(tmp_path):
    async def check():
        calls = []
        class Recording(FakeAdapter):
            async def capabilities(self):
                calls.append('probe')
                return await super().capabilities()
        service = HostService(tmp_path, {'fake': Recording()})
        try:
            owner, _ = await connected(service)
            await asyncio.sleep(0)
            assert not calls
            await service.request(owner, 'agent.status', {})
            assert calls == ['probe']
        finally: await service.close()
    asyncio.run(check())


def test_failed_login_cancellation_closes_worker_instead_of_leaving_authorization_active():
    async def check():
        client = RuntimeClient(FakeEngine(), RUNTIME, '/project')
        owner = object()
        client.logins['claude'] = {'client': owner, 'id': 'login'}
        async def reject(*args): raise AdapterError('E_RUNTIME_EXITED', 'test', 'Closed')
        client.request = reject
        await client.disconnect(owner)
        assert client.closing and not client.logins
    asyncio.run(check())


def test_cpu_quota_inspection_accepts_equivalent_podman_representation(tmp_path, monkeypatch):
    engine, project, data, record = engine_fixture(tmp_path, monkeypatch)
    record['HostConfig'].update(NanoCpus=0, CpuQuota=200_000, CpuPeriod=100_000)
    engine.verify(record, RUNTIME, project, data)
    record['HostConfig']['CpuQuota'] = 400_000
    with pytest.raises(AdapterError): engine.verify(record, RUNTIME, project, data)


def test_legacy_database_upgrade_preserves_project_session_and_events(tmp_path):
    path = tmp_path / 'host.db'
    connection = sqlite3.connect(path)
    connection.executescript('\n'.join(MIGRATIONS[:2]) + '\nPRAGMA user_version=2;')
    connection.execute("INSERT INTO meta VALUES('host_id','existing-host')")
    connection.execute("INSERT INTO meta VALUES('seq','17')")
    connection.execute("INSERT INTO projects(id,path,display_name,created_at) VALUES('p','/project','Project','time')")
    connection.execute("INSERT INTO sessions(id,project_id,agent,native_id,title,model,permission_mode,status,created_at,updated_at) VALUES('s','p','claude','native-id','Title','model','default','idle','time','time')")
    connection.execute("INSERT INTO events(seq,session_id,ts,type,payload_json) VALUES(17,'s','time','item.completed','{}')")
    connection.commit(); connection.close()
    database = Database(path)
    try:
        assert database.host_id == 'existing-host' and database.current_seq == 17
        assert database.one("SELECT runtime_mode,runtime_id FROM projects WHERE id='p'") == {'runtime_mode': 'direct', 'runtime_id': None}
        assert database.one("SELECT native_id,runtime_id FROM sessions WHERE id='s'") == {'native_id': 'native-id', 'runtime_id': None}
        assert database.one('SELECT COUNT(*) AS total FROM events')['total'] == 1
    finally: database.close()


@pytest.mark.parametrize('busy', [False, True])
def test_pool_recycles_idle_workers_but_never_busy_workers(tmp_path, busy):
    async def check():
        pool = RuntimePool(tmp_path, 'host')
        pool.engine = FakeEngine()
        launched = []
        async def launch(runtime_id, path):
            launched.append(runtime_id)
            return 'container', SimpleNamespace(returncode=None)
        pool.engine.launch = launch
        for index in range(8):
            identifier = 'rt_' + str(index) * 32
            client = RuntimeClient(pool.engine, identifier, '/project')
            client.process = SimpleNamespace(returncode=None)
            client.runs = {'run': {}} if busy else {}
            async def close(client=client): client.closing = True
            client.close = close
            pool.clients[identifier] = client
        if busy:
            with pytest.raises(AdapterError) as error: await pool.launch(RUNTIME, '/project')
            assert error.value.code == 'E_RUNTIME_LIMIT'
            assert not launched and len(pool.clients) == 8
        else:
            await pool.launch(RUNTIME, '/project')
            assert launched == [RUNTIME] and len(pool.clients) == 7
    asyncio.run(check())


def test_failed_cleanup_blocks_replacement_and_retains_container_ownership(tmp_path):
    async def check():
        pool = RuntimePool(tmp_path, 'host')
        client = pool.client({'runtimeId': RUNTIME, 'path': '/project'})
        name = 'agentdesk-' + 'c' * 32
        client.name = name
        pool.engine.owned.add(name)
        async def remove_failure(name): raise AdapterError('E_RUNTIME_UNAVAILABLE', 'test', 'Unavailable')
        pool.engine._remove = remove_failure
        with pytest.raises(AdapterError): await client.close()
        assert client.cleanup_error is not None and name in pool.engine.owned
        assert not (await pool.engine.status())['supported']
        with pytest.raises(AdapterError) as error: pool.client({'runtimeId': RUNTIME, 'path': '/project'})
        assert error.value.code == 'E_RUNTIME_UNAVAILABLE' and pool.clients[RUNTIME] is client
        async def removed(name): pool.engine.owned.discard(name)
        pool.engine._remove = removed
        await pool.close()
        assert not pool.engine.owned and not pool.engine.cleanup_failed
    asyncio.run(check())


@pytest.mark.parametrize('method', ['run.start', 'run.append', 'capabilities'])
def test_lost_worker_response_never_reports_input_as_safe_to_resend(method):
    async def check():
        client = RuntimeClient(FakeEngine(), RUNTIME, '/project')
        async def send(message):
            client.pending[message['id']].set_exception(AdapterError('E_RUNTIME_EXITED', 'test', 'Disconnected'))
        client.send = send
        with pytest.raises(AdapterError) as error: await client.exchange(method, {})
        assert error.value.code == ('E_RUNTIME_EXITED' if method == 'capabilities' else 'E_DELIVERY_UNKNOWN')
        assert not client.pending
    asyncio.run(check())
