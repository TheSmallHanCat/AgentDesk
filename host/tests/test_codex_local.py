import asyncio

from agentdesk_host.adapters.base import RunContext
from agentdesk_host.adapters.codex import CodexAdapter


def test_local_permissions_and_instructions_are_passed_without_global_config(monkeypatch):
    from agentdesk_host.adapters import codex
    async def check():
        calls = []
        class Rpc:
            def __init__(self, executable, config):
                calls.append(('process', config))
                self.notifications = asyncio.Queue()
            async def __aenter__(self): return self
            async def __aexit__(self, *args): pass
            async def request(self, method, params):
                calls.append((method, params))
                if method == 'thread/start': return {'thread': {'id': 'thread'}}
                if method == 'turn/start':
                    await self.notifications.put({'method': 'turn/completed', 'params': {'threadId': 'thread', 'turnId': 'turn', 'turn': {'status': 'completed'}}})
                    return {'turn': {'id': 'turn'}}
        monkeypatch.setattr(codex, 'NativeRpc', Rpc)
        async def emit(*args): pass
        async def approve(*args): return {'behavior': 'deny'}
        async def save(*args): pass
        session = {'path': '/native/project', 'kind': 'local', 'deviceRoot': '/devices/one', 'deviceInstructions': 'Use stable session config', 'permissionMode': 'default'}
        await CodexAdapter('codex')._run(session, {'text': 'Inspect project'}, RunContext('run', emit, approve, save), {'threadId': None, 'turnId': None, 'stopping': False})
        config = calls[0][1]
        assert 'sandbox_mode="workspace-write"' in config
        assert 'sandbox_workspace_write.network_access=true' in config
        assert 'sandbox_workspace_write.writable_roots=["/devices/one"]' in config
        assert 'danger-full-access' not in str(calls)
        params = next(params for method, params in calls if method == 'thread/start')
        assert params['developerInstructions'] == session['deviceInstructions']
        assert params['sandbox'] == 'workspace-write'
        assert params['approvalPolicy'] == 'on-request'
    asyncio.run(check())
