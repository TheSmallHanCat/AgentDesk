"""Bounded stdio bridge inside a workspace; never opens the Host database/socket."""
from __future__ import annotations

import argparse
import asyncio
import json
import os
from pathlib import Path
import sys
import tempfile
import types
import uuid

from ..adapters.base import AdapterError, RunContext
from ..adapters.claude import ClaudeAdapter
from ..adapters.fake import FakeAdapter
from ..adapters.native_common import MAX_FRAME
from ..server.login import LoginManager


def probe() -> dict:
    namespaces = {name: os.readlink('/proc/self/ns/' + name) for name in ('pid', 'net', 'mnt', 'ipc', 'uts', 'user', 'cgroup')}
    for directory in ('/workspace', '/home/agent'):
        with tempfile.TemporaryFile(dir=directory) as file:
            file.write(b'probe')
    root_readonly = any(row.split()[4] == '/' and 'ro' in row.split()[5].split(',') for row in Path('/proc/self/mountinfo').read_text().splitlines())
    status = dict(line.split(':', 1) for line in Path('/proc/self/status').read_text().splitlines() if ':' in line)
    cpu = Path('/sys/fs/cgroup/cpu.max').read_text().split()
    limits = Path('/sys/fs/cgroup/memory.max').read_text().strip() == str(2 * 1024**3) and Path('/sys/fs/cgroup/pids.max').read_text().strip() == '256' and cpu[0] != 'max' and 0 < int(cpu[0]) <= 2 * int(cpu[1])
    return {'protocol': 1, 'namespaces': namespaces, 'rootReadonly': root_readonly, 'uid': os.getuid(), 'noNewPrivileges': status.get('NoNewPrivs', '').strip() == '1', 'capabilitiesDropped': int(status.get('CapEff', '0'), 16) == 0, 'seccomp': status.get('Seccomp', '').strip() == '2', 'limits': limits}


def text(params, name, limit=2_000_000):
    value = params.get(name)
    if not isinstance(value, str) or not value or len(value.encode('utf-8')) > limit:
        raise AdapterError('E_INVALID_PARAMS', 'runtime.worker', 'Invalid request')
    return value


class Worker:
    def __init__(self, send, *, fake=False):
        self.send = send
        self.adapters = {'fake': FakeAdapter()} if fake else {'claude': ClaudeAdapter()}
        self.fake = fake
        self.handles = {}
        self.inputs = {}
        self.tasks = set()
        self.pending = {}
        self.mutation_lock = asyncio.Lock()
        self.logins = LoginManager(self.refresh_capability)
        self.owner = types.SimpleNamespace(send=send)

    def finished(self, task):
        self.tasks.discard(task)
        if not task.cancelled() and task.exception() is not None:
            for _, handle in tuple(self.handles.values()):
                handle.task.cancel()

    async def refresh_capability(self, agent):
        return await self.capability(agent, refresh=True)

    async def capability(self, agent, refresh=False):
        adapter = self.adapters.get(agent)
        if adapter is None:
            raise AdapterError('E_RUNTIME_AGENT', 'runtime.worker', 'Agent is not supported in this runtime')
        if refresh and hasattr(adapter, '_caps'):
            adapter._caps = None
        result = await adapter.capabilities()
        return {**result, 'name': getattr(adapter, 'name', agent), 'nativeTerminal': False}

    async def host_request(self, method, params):
        identifier = 'worker:' + uuid.uuid4().hex
        future = asyncio.get_running_loop().create_future()
        self.pending[identifier] = future
        try:
            await self.send({'id': identifier, 'method': method, 'params': params})
            return await future
        finally:
            self.pending.pop(identifier, None)

    def session(self, value):
        if not isinstance(value, dict) or set(value) - {'id', 'agent', 'nativeId', 'model', 'permissionMode'}:
            raise AdapterError('E_TARGET', 'runtime.worker', 'Invalid session scope')
        result = {key: text(value, key, 256) for key in ('id', 'agent', 'model', 'permissionMode')}
        if value.get('nativeId') is not None:
            result['nativeId'] = text(value, 'nativeId', 256)
        result.update(path='/workspace', kind='remote')
        return result

    async def request(self, method, params):
        if method == 'probe':
            return {'protocol': 1} if self.fake else probe()
        if method == 'capabilities':
            return await self.capability(text(params, 'agent', 64))
        if method in ('login.start', 'logout'):
            agent = text(params, 'agent', 64)
            adapter = self.adapters.get(agent)
            if adapter is None:
                raise AdapterError('E_RUNTIME_AGENT', 'runtime.worker', 'Agent is not supported')
            if self.handles:
                raise AdapterError('E_SESSION_BUSY', 'runtime.worker', 'Wait for workspace runs to finish')
            if method == 'login.start':
                return self.logins.start(self.owner, agent, adapter)
            if self.logins.sessions:
                raise AdapterError('E_LOGIN_BUSY', 'runtime.worker', 'Login is active')
            await adapter.logout()
            return await self.refresh_capability(agent)
        if method == 'login.respond':
            return await self.logins.respond(self.owner, text(params, 'loginId', 128), text(params, 'text', 2048))
        if method == 'login.cancel':
            return await self.logins.cancel(self.owner, text(params, 'loginId', 128))
        if method == 'native.exists':
            session = self.session(params.get('session'))
            adapter = self.adapters.get(session['agent'])
            if adapter is None:
                raise AdapterError('E_RUNTIME_AGENT', 'runtime.worker', 'Agent is not supported')
            return {'exists': await adapter.native_exists(session)}
        if method == 'run.start':
            session = self.session(params.get('session'))
            run_id = text(params, 'runId', 128)
            if run_id in self.handles:
                raise AdapterError('E_CONFLICT', 'runtime.worker', 'Run identity is already active')
            adapter = self.adapters.get(session['agent'])
            if adapter is None:
                raise AdapterError('E_RUNTIME_AGENT', 'runtime.worker', 'Agent is not supported')
            if self.logins.sessions:
                raise AdapterError('E_LOGIN_BUSY', 'runtime.worker', 'Login is active')
            async def emit(kind, payload, item_id=None):
                await self.send({'method': 'run.event', 'params': {'runId': run_id, 'type': kind, 'payload': payload, 'itemId': item_id}})
            async def approve(kind, payload):
                return await self.host_request('host.approve', {'runId': run_id, 'kind': kind, 'payload': payload})
            async def save(native_id):
                await self.host_request('host.saveNative', {'runId': run_id, 'nativeId': native_id})
            input = {'text': text(params, 'text')}
            if self.fake:
                input['script'] = params.get('script', [])
                session['path'] = os.getcwd()
            handle = await adapter.start_run(session, input, RunContext(run_id, emit, approve, save))
            self.handles[run_id] = (adapter, handle)
            self.inputs[run_id] = []
            task = asyncio.create_task(self.watch(run_id, handle.task))
            self.tasks.add(task)
            task.add_done_callback(self.finished)
            return {'accepted': True}
        if method in ('run.append', 'run.stop'):
            run_id = text(params, 'runId', 128)
            active = self.handles.get(run_id)
            if not active:
                raise AdapterError('E_APPEND_CLOSED', 'runtime.worker', 'Run is no longer active')
            adapter, handle = active
            if method == 'run.stop':
                await adapter.stop(handle)
            else:
                envelope = {'text': text(params, 'text'), 'clientMsgId': text(params, 'clientMsgId', 128), 'delivered': False}
                async with self.mutation_lock:
                    if run_id not in self.handles:
                        raise AdapterError('E_APPEND_CLOSED', 'runtime.worker', 'Run is no longer active')
                    await adapter.append_input(handle, envelope)
                    self.inputs[run_id].append(envelope)
            return {'accepted': True}
        raise AdapterError('E_CAPABILITY', 'runtime.worker', 'Unsupported worker operation')

    async def watch(self, run_id, task):
        state, code = 'completed', None
        try:
            await task
        except asyncio.CancelledError:
            state = 'interrupted'
        except Exception as error:
            state, code = 'failed', error.code if isinstance(error, AdapterError) else 'E_AGENT'
        finally:
            async with self.mutation_lock:
                delivered = [value['clientMsgId'] for value in self.inputs.pop(run_id, []) if value['delivered']]
                self.handles.pop(run_id, None)
                await self.send({'method': 'run.finished', 'params': {'runId': run_id, 'state': state, 'code': code, 'delivered': delivered}})

    async def close(self):
        await self.logins.close()
        for future in self.pending.values():
            future.cancel()
        for _, handle in tuple(self.handles.values()):
            handle.task.cancel()
        for adapter in self.adapters.values():
            close = getattr(adapter, 'close', None)
            if close:
                await close()
        if self.tasks:
            await asyncio.gather(*tuple(self.tasks), return_exceptions=True)


async def serve(fake=False):
    lock = asyncio.Lock()
    async def send(message):
        data = json.dumps(message, ensure_ascii=False, separators=(',', ':')).encode() + b'\n'
        if len(data) > MAX_FRAME:
            raise ConnectionError('Worker frame exceeds limit')
        async with lock:
            await asyncio.to_thread(write, data)
    def write(data):
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
    worker = Worker(send, fake=fake)
    requests = set()
    serving = asyncio.current_task()
    def finished(task):
        requests.discard(task)
        if not task.cancelled() and task.exception() is not None:
            serving.cancel()
    async def dispatch(message):
        try:
            result = await worker.request(message['method'], message.get('params', {}))
            await send({'id': message['id'], 'result': result})
        except Exception as error:
            await send({'id': message['id'], 'error': {'code': error.code if isinstance(error, AdapterError) else 'E_RUNTIME_WORKER'}})
    try:
        while line := await asyncio.to_thread(sys.stdin.buffer.readline, MAX_FRAME + 1):
            if len(line) > MAX_FRAME or not line.endswith(b'\n'):
                break
            message = json.loads(line)
            if not isinstance(message, dict):
                break
            if 'method' not in message:
                future = worker.pending.get(message.get('id'))
                if future and not future.done():
                    if 'error' in message:
                        future.set_exception(AdapterError('E_RUNTIME_CONTROL', 'runtime.worker', 'Control request failed'))
                    else:
                        future.set_result(message.get('result'))
                continue
            if type(message.get('id')) is not int or not isinstance(message['method'], str) or not isinstance(message.get('params', {}), dict) or len(requests) >= 32:
                break
            task = asyncio.create_task(dispatch(message))
            requests.add(task)
            task.add_done_callback(finished)
    finally:
        for task in requests:
            task.cancel()
        await asyncio.gather(*requests, return_exceptions=True)
        await worker.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--fake', action='store_true')
    args = parser.parse_args()
    if not args.fake and os.environ.get('AGENTDESK_ISOLATED_WORKER') != '1':
        parser.error('Worker requires an isolated workspace')
    asyncio.run(serve(args.fake))


if __name__ == '__main__':
    main()
