"""Adapter facade preserving Host lifecycle and durable native session identity."""
from __future__ import annotations

import asyncio

from ..adapters.base import RunHandle
from ..adapters.native_common import capability
from .client import RuntimeClient
from .engine import PodmanEngine, failure


class IsolatedAdapter:
    name = 'Claude Code'

    def __init__(self, client, agent):
        self.client, self.agent = client, agent

    async def capabilities(self):
        if self.agent != 'claude':
            return capability(name=self.agent, reason='E_RUNTIME_AGENT', nativeTerminal=False)
        try:
            return await self.client.request('capabilities', {'agent': self.agent})
        except Exception as error:
            return capability(name=self.name, reason=getattr(error, 'code', 'E_RUNTIME_UNAVAILABLE'), nativeTerminal=False)

    async def native_exists(self, session):
        if not session.get('nativeId'):
            return True
        result = await self.client.request('native.exists', {'session': self.session(session)})
        return result.get('exists') is True

    def session(self, session):
        if session.get('runtimeId') != self.client.runtime_id:
            raise failure('E_TARGET', 'Session belongs to another runtime')
        return {key: session[key] for key in ('id', 'agent', 'nativeId', 'model', 'permissionMode') if key in session}

    async def start_run(self, session, input, context):
        await self.client.ensure()
        future = asyncio.get_running_loop().create_future()
        native = {'pendingInputs': {}}
        entry = {'future': future, 'context': context, 'native': native}
        self.client.runs[context.run_id] = entry
        async def wait():
            try:
                await future
            finally:
                self.client.runs.pop(context.run_id, None)
        task = asyncio.create_task(wait())
        try:
            await self.client.request('run.start', {'runId': context.run_id, 'session': self.session(session), 'text': input['text']})
        except BaseException:
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
            await self.client.close()
            raise
        return RunHandle(context.run_id, task, native)

    async def append_input(self, handle, envelope):
        await self.client.request('run.append', {'runId': handle.run_id, 'text': envelope['text'], 'clientMsgId': envelope['clientMsgId']})

    async def stop(self, handle):
        await self.client.request('run.stop', {'runId': handle.run_id})
        try:
            await asyncio.wait_for(asyncio.shield(handle.task), 10)
        except asyncio.CancelledError:
            if not handle.task.done():
                raise
        except TimeoutError:
            await self.client.close()

    async def respond_approval(self, *args):
        # The worker's pending request is resolved by RunContext.approve.
        return None


class RuntimePool:
    def __init__(self, home, host_id, config=None):
        self.engine = PodmanEngine(home, host_id, config)
        self.clients = {}
        self.launch_lock = asyncio.Lock()
        self.closing = False

    async def launch(self, runtime_id, path):
        async with self.launch_lock:
            if self.closing:
                raise failure('E_RUNTIME_EXITED', 'Host is closing its workspaces')
            active = [client for client in self.clients.values() if client.process and client.process.returncode is None]
            if len(active) >= 8:
                idle = next((client for client in active if not client.runs and not client.logins and not client.pending and not client.handlers and not client.lock.locked()), None)
                if idle is None:
                    raise failure('E_RUNTIME_LIMIT', 'All workspace workers are busy')
                await idle.close()
                self.clients.pop(idle.runtime_id, None)
            return await self.engine.launch(runtime_id, path)

    def client(self, project):
        identifier = project['runtimeId']
        if not identifier:
            raise failure('E_TARGET', 'Isolated project has no runtime identity')
        current = self.clients.get(identifier)
        if current and (current.closing or current.reader and current.reader.done()):
            if current.runs or current.logins:
                raise failure('E_RUNTIME_EXITED', 'Wait for interrupted operations to finish')
            if current.cleanup is None or not current.cleanup.done():
                raise failure('E_RUNTIME_BUSY', 'Workspace cleanup is still in progress')
            if current.cleanup.cancelled() or current.cleanup.exception() is not None:
                raise failure('E_RUNTIME_UNAVAILABLE', 'Workspace cleanup was not confirmed')
            self.clients.pop(identifier)
            current = None
        if current is None:
            if self.closing:
                raise failure('E_RUNTIME_EXITED', 'Host is closing its workspaces')
            current = RuntimeClient(self.engine, identifier, project['path'], self.launch)
            self.clients[identifier] = current
        if current.path != project['path']:
            raise failure('E_TARGET', 'Workspace path changed')
        return current

    def adapter(self, project, agent):
        return IsolatedAdapter(self.client(project), agent)

    def login(self, identifier, owner):
        for client in self.clients.values():
            entry = client.login_owner(identifier)
            if entry:
                if entry['client'] is not owner:
                    raise failure('E_TARGET', 'Login belongs to another client')
                return client
        return None

    def busy(self):
        return any(client.logins or client.runs for client in self.clients.values())

    async def disconnect(self, owner):
        async with self.launch_lock:
            pass
        await asyncio.gather(*(client.disconnect(owner) for client in self.clients.values()), return_exceptions=True)

    async def close(self):
        self.closing = True
        async with self.launch_lock:
            pass
        await asyncio.gather(*(client.close() for client in self.clients.values()), return_exceptions=True)
        await self.engine.close()
