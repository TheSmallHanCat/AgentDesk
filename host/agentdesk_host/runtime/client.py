"""Host-side worker transport. Every callback remains bound to its original run."""
from __future__ import annotations

import asyncio
import json
import os
import re

from ..adapters.base import AdapterError
from ..adapters.native_common import MAX_FRAME, close_process
from .engine import failure


class RuntimeClient:
    def __init__(self, engine, runtime_id: str, path: str, launcher=None):
        self.engine, self.runtime_id, self.path = engine, runtime_id, path
        self.process = None
        self.name = None
        self.reader = None
        self.cleanup = None
        self.lock = asyncio.Lock()
        self.writes = asyncio.Lock()
        self.pending = {}
        self.handlers = set()
        self.runs = {}
        self.logins = {}
        self.counter = 0
        self.closing = False
        self.launcher = launcher
        self.cleanup_error = None

    def start_cleanup(self):
        if self.cleanup is None:
            self.cleanup = asyncio.create_task(self.dispose())
            def finished(task):
                if not task.cancelled():
                    self.cleanup_error = task.exception()
            self.cleanup.add_done_callback(finished)

    def handler_finished(self, task):
        self.handlers.discard(task)
        if not task.cancelled() and task.exception() is not None and self.reader:
            self.reader.cancel()

    async def ensure(self):
        async with self.lock:
            if self.closing:
                raise failure('E_RUNTIME_EXITED', 'Workspace worker has closed')
            if self.process is not None:
                if self.process.returncode is not None or self.reader.done():
                    raise failure('E_RUNTIME_EXITED', 'Workspace worker has exited')
                return
            self.name, self.process = await (self.launcher or self.engine.launch)(self.runtime_id, self.path)
            self.reader = asyncio.create_task(self.read())
            try:
                report = await self.exchange('probe', {})
                if report.get('protocol') != 1 or report.get('rootReadonly') is not True or report.get('uid') != os.getuid():
                    raise failure('E_RUNTIME_PROBE', 'Workspace did not confirm its execution boundary')
                if not all(report.get(key) is True for key in ('noNewPrivileges', 'capabilitiesDropped', 'seccomp', 'limits')):
                    raise failure('E_RUNTIME_POLICY', 'Workspace privilege or resource policy was not applied')
                for namespace in ('pid', 'net', 'mnt', 'ipc', 'uts', 'user', 'cgroup'):
                    actual = report.get('namespaces', {}).get(namespace)
                    if not isinstance(actual, str) or actual == os.readlink('/proc/self/ns/' + namespace):
                        raise failure('E_RUNTIME_PROBE', 'Workspace namespace is not isolated')
            except BaseException:
                await self.close()
                raise

    async def send(self, value):
        data = json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode() + b'\n'
        if len(data) > MAX_FRAME:
            raise failure('E_INPUT', 'Worker request exceeds the frame limit')
        async with self.writes:
            if not self.process or self.process.returncode is not None or not self.process.stdin:
                raise failure('E_RUNTIME_EXITED', 'Workspace worker is disconnected')
            self.process.stdin.write(data)
            await asyncio.wait_for(self.process.stdin.drain(), 15)

    async def exchange(self, method, params):
        if len(self.pending) >= 32:
            raise failure('E_RUNTIME_BUSY', 'Workspace has too many pending operations')
        self.counter += 1
        identifier = self.counter
        future = asyncio.get_running_loop().create_future()
        self.pending[identifier] = future
        try:
            await self.send({'id': identifier, 'method': method, 'params': params})
            return await asyncio.wait_for(future, 45)
        except (TimeoutError, OSError):
            raise failure('E_DELIVERY_UNKNOWN' if method in ('run.start', 'run.append') else 'E_RUNTIME_TIMEOUT', 'Workspace response was not confirmed') from None
        except AdapterError as error:
            if method in ('run.start', 'run.append') and error.code == 'E_RUNTIME_EXITED':
                raise failure('E_DELIVERY_UNKNOWN', 'Workspace input delivery was not confirmed') from error
            raise
        finally:
            self.pending.pop(identifier, None)

    async def request(self, method, params):
        await self.ensure()
        return await self.exchange(method, params)

    async def host_request(self, message):
        params = message.get('params', {})
        entry = self.runs.get(params.get('runId'))
        try:
            if entry is None:
                raise failure('E_TARGET', 'Worker callback has no active owner')
            context = entry['context']
            if message['method'] == 'host.approve' and isinstance(params.get('payload'), dict):
                result = await context.approve(params.get('kind'), params['payload'])
            elif message['method'] == 'host.saveNative' and isinstance(params.get('nativeId'), str) and 0 < len(params['nativeId']) <= 256:
                await context.save_native(params['nativeId'])
                result = {'saved': True}
            else:
                raise failure('E_RUNTIME_PROTOCOL', 'Unknown worker callback')
            await self.send({'id': message['id'], 'result': result})
        except Exception:
            await self.send({'id': message['id'], 'error': {'code': 'E_RUNTIME_CONTROL'}})

    async def notification(self, message):
        method, params = message.get('method'), message.get('params', {})
        if not isinstance(params, dict):
            raise ValueError('Invalid worker notification')
        if method in ('login.prompt', 'login.state'):
            owner = self.logins.get(params.get('agent'))
            if owner is None:
                return
            identifier = params.get('loginId')
            if not isinstance(identifier, str) or owner['id'] not in (None, identifier):
                raise ValueError('Login ownership mismatch')
            owner['id'] = identifier
            await owner['client'].send({'jsonrpc': '2.0', 'method': method, 'params': params})
            if method == 'login.state' and params.get('state') not in ('starting', 'waiting'):
                self.logins.pop(params['agent'], None)
            return
        entry = self.runs.get(params.get('runId'))
        if entry is None:
            return
        if method == 'run.event':
            if params.get('type') not in ('item.started', 'item.delta', 'item.completed', 'usage', 'run.state') or not isinstance(params.get('payload'), dict):
                raise ValueError('Invalid worker event')
            await entry['context'].emit(params['type'], params['payload'], params.get('itemId'))
        elif method == 'run.finished':
            delivered = params.get('delivered', [])
            if not isinstance(delivered, list) or any(not isinstance(value, str) for value in delivered):
                raise ValueError('Invalid input delivery state')
            for identifier in delivered:
                envelope = entry['native'].get('pendingInputs', {}).get(identifier)
                if envelope:
                    envelope['delivered'] = True
            future = entry['future']
            if not future.done():
                if params.get('state') == 'completed':
                    future.set_result(None)
                elif params.get('state') == 'interrupted':
                    future.cancel()
                else:
                    future.set_exception(failure('E_AGENT', 'Isolated Agent execution failed'))

    async def read(self):
        try:
            while line := await self.process.stdout.readline():
                if len(line) > MAX_FRAME or not line.endswith(b'\n'):
                    raise ValueError('Worker frame limit')
                message = json.loads(line)
                if not isinstance(message, dict):
                    raise ValueError('Invalid worker response')
                if 'method' in message and 'id' in message:
                    if len(self.handlers) >= 64 or not isinstance(message['id'], str) or not message['id'].startswith('worker:'):
                        raise ValueError('Invalid worker callback identity')
                    task = asyncio.create_task(self.host_request(message))
                    self.handlers.add(task)
                    task.add_done_callback(self.handler_finished)
                elif 'method' in message:
                    await self.notification(message)
                else:
                    future = self.pending.get(message.get('id'))
                    if future and not future.done():
                        if 'error' in message:
                            code = message['error'].get('code') if isinstance(message['error'], dict) else None
                            code = code if isinstance(code, str) and re.fullmatch(r'E_[A-Z_]{1,48}', code) else 'E_RUNTIME_WORKER'
                            future.set_exception(failure(code, 'Workspace operation failed'))
                        else:
                            future.set_result(message.get('result'))
        except (ValueError, TypeError, KeyError, OSError, ConnectionError, AdapterError, asyncio.CancelledError):
            pass
        finally:
            for future in self.pending.values():
                if not future.done():
                    future.set_exception(failure('E_RUNTIME_EXITED', 'Workspace worker exited'))
            for entry in self.runs.values():
                for envelope in entry['native'].get('pendingInputs', {}).values():
                    if not envelope.get('delivered'):
                        envelope['deliveryUnknown'] = True
                if not entry['future'].done():
                    entry['future'].set_exception(failure('E_RUNTIME_EXITED', 'Workspace worker exited'))
            for agent, owner in tuple(self.logins.items()):
                if owner['id']:
                    try:
                        await owner['client'].send({'jsonrpc': '2.0', 'method': 'login.state', 'params': {'agent': agent, 'loginId': owner['id'], 'state': 'failed', 'reason': 'E_RUNTIME_EXITED'}})
                    except Exception:
                        pass
            self.logins.clear()
            self.start_cleanup()

    async def login_start(self, client, agent):
        if agent in self.logins:
            raise failure('E_LOGIN_BUSY', 'Agent already has an active login')
        owner = {'client': client, 'id': None}
        self.logins[agent] = owner
        try:
            result = await self.request('login.start', {'agent': agent})
            if not isinstance(result, dict) or not isinstance(result.get('loginId'), str) or result.get('agent') != agent:
                raise failure('E_RUNTIME_PROTOCOL', 'Invalid login response')
            if self.logins.get(agent) is owner:
                owner['id'] = result['loginId']
            return result
        except BaseException:
            self.logins.pop(agent, None)
            await self.close()
            raise

    def login_owner(self, login_id):
        return next((owner for owner in self.logins.values() if owner['id'] == login_id), None)

    async def disconnect(self, client):
        for agent, owner in tuple(self.logins.items()):
            if owner['client'] is client:
                try:
                    if owner['id']:
                        await self.request('login.cancel', {'loginId': owner['id']})
                    else:
                        await self.close()
                except Exception:
                    await self.close()
                finally:
                    self.logins.pop(agent, None)

    async def dispose(self):
        try:
            if self.name:
                await self.engine.remove(self.name)
        finally:
            if self.process:
                if self.process.stdin:
                    self.process.stdin.close()
                await close_process(self.process)

    async def close(self):
        self.closing = True
        for task in self.handlers:
            task.cancel()
        if self.handlers:
            await asyncio.gather(*self.handlers, return_exceptions=True)
        self.start_cleanup()
        await self.cleanup
        if self.reader and self.reader is not asyncio.current_task():
            self.reader.cancel()
            await asyncio.gather(self.reader, return_exceptions=True)
