"""Replay the documented 0.2.174 block/message contract without model access.

Sequence source: https://code.claude.com/docs/en/agent-sdk/streaming-output
SDK signatures: anthropics/claude-agent-sdk-python v0.2.174, client.py/types.py.
These fixtures model the public wire sequence; they are not model responses.
"""
import asyncio
from pathlib import Path
import sys
import types

import pytest

from agentdesk_host.adapters.base import RunContext, RunHandle
from agentdesk_host.adapters.claude import ClaudeAdapter
from agentdesk_host.adapters.native_common import ClaudeProcessGroup


def object_of(class_name, **fields):
    value = type(class_name, (), {})()
    value.__dict__.update(fields)
    return value


def sdk_stub(monkeypatch, client):
    sdk = types.ModuleType('claude_agent_sdk')
    sdk.ClaudeAgentOptions = lambda **kwargs: types.SimpleNamespace(**kwargs)
    sdk.ClaudeSDKClient = lambda options: client.set_options(options)
    sdk_types = types.ModuleType('claude_agent_sdk.types')
    sdk_types.HookMatcher = lambda **kwargs: kwargs
    sdk_types.PermissionResultAllow = lambda **kwargs: kwargs
    sdk_types.PermissionResultDeny = lambda **kwargs: kwargs
    monkeypatch.setitem(sys.modules, 'claude_agent_sdk', sdk)
    monkeypatch.setitem(sys.modules, 'claude_agent_sdk.types', sdk_types)
    monkeypatch.setattr(ClaudeAdapter, 'executable', staticmethod(lambda: 'recorded-cli'))
    monkeypatch.setattr(ClaudeProcessGroup, 'prepare', lambda self: self.executable)


class ReplayClient:
    def __init__(self, messages):
        self.messages = messages
        self.queries = []
        self.disconnected = False
    def set_options(self, options):
        self.options = options
        return self
    async def connect(self): pass
    async def disconnect(self): self.disconnected = True
    async def query(self, text, session_id='default'): self.queries.append((text, session_id))
    async def receive_messages(self):
        for message in self.messages:
            yield message
    async def receive_response(self):
        raise AssertionError('Multi-input runs must consume receive_messages')
        yield


def stream(event_type, **fields):
    return object_of('StreamEvent', event={'type': event_type, **fields})


def assistant(mid, *blocks):
    return object_of('AssistantMessage', message_id=mid, content=list(blocks))


def result():
    return object_of('ResultMessage', session_id='native-session', usage={'output_tokens': 1}, is_error=False)


async def context(events):
    async def emit(kind, payload, item_id=None): events.append((kind, payload, item_id))
    async def approve(*args): return {'behavior': 'allow'}
    async def save(*args): pass
    return RunContext('run', emit, approve, save)


@pytest.mark.parametrize('blocks', [
    [('text', 'TextBlock', {'text': 'Read the project'}), ('tool_use', 'ToolUseBlock', {'id': 'read', 'name': 'Read', 'input': {'file_path': 'README.md'}})],
    [('tool_use', 'ToolUseBlock', {'id': 'one', 'name': 'Read', 'input': {'file_path': 'a'}}), ('tool_use', 'ToolUseBlock', {'id': 'two', 'name': 'Write', 'input': {'file_path': 'b', 'content': 'value'}})],
    [('thinking', 'ThinkingBlock', {'thinking': 'Inspect available files', 'signature': 'signature'}), ('text', 'TextBlock', {'text': 'The files are ready'})],
    [('text', 'TextBlock', {'text': 'Same text'}), ('text', 'TextBlock', {'text': 'Same text'})],
])
def test_split_shared_message_id_preserves_every_block_and_stream_identity(monkeypatch, tmp_path, blocks):
    async def check():
        messages = [stream('message_start', message={'id': 'message'})]
        expected = []
        for index, (block_type, sdk_name, fields) in enumerate(blocks):
            native = {'type': block_type, **fields}
            item_id = fields.get('id', 'message:' + str(index))
            expected.append(item_id)
            messages.append(stream('content_block_start', index=index, content_block=native))
            complete = assistant('message', object_of(sdk_name, **fields))
            messages.extend([complete, complete, stream('content_block_stop', index=index)])
        messages.append(result())
        client = ReplayClient(messages)
        sdk_stub(monkeypatch, client)
        events = []
        await ClaudeAdapter()._run({'path': str(tmp_path), 'deviceInstructions': 'Stable session SSH config'}, {'text': 'Inspect project'}, await context(events), {'client': None, 'stopping': False})
        completed = [item_id for kind, _, item_id in events if kind == 'item.completed']
        started = [item_id for kind, _, item_id in events if kind == 'item.started']
        assert completed == expected == started
        assert client.options.system_prompt == {'type': 'preset', 'preset': 'claude_code', 'append': 'Stable session SSH config'}
        assert client.disconnected
    asyncio.run(check())


def test_aggregate_messages_and_replayed_native_tool_ids(monkeypatch, tmp_path):
    async def check():
        text = object_of('TextBlock', text='Project summary')
        tool = object_of('ToolUseBlock', id='tool', name='Bash', input={'command': 'pwd'})
        complete = assistant('message', text, tool)
        client = ReplayClient([complete, complete, result()])
        sdk_stub(monkeypatch, client)
        events = []
        await ClaudeAdapter()._run({'path': str(tmp_path)}, {'text': 'Inspect'}, await context(events), {'client': None, 'stopping': False})
        assert len([kind for kind, _, _ in events if kind == 'item.completed']) == 2
    asyncio.run(check())


def test_append_keeps_receiving_after_first_result(monkeypatch, tmp_path):
    async def check():
        ready, appended = asyncio.Event(), asyncio.Event()
        class Client(ReplayClient):
            async def receive_messages(self):
                ready.set()
                await appended.wait()
                yield assistant('first', object_of('TextBlock', text='First answer'))
                yield result()
                yield assistant('second', object_of('TextBlock', text='Second answer'))
                yield result()
        client = Client([])
        sdk_stub(monkeypatch, client)
        events = []
        native = {'client': None, 'stopping': False}
        adapter = ClaudeAdapter()
        task = asyncio.create_task(adapter._run({'path': str(tmp_path)}, {'text': 'First input'}, await context(events), native))
        await ready.wait()
        await adapter.append_input(RunHandle('run', task, native), 'Second input')
        appended.set()
        await asyncio.wait_for(task, 2)
        assert [payload['text'] for kind, payload, _ in events if kind == 'item.completed'] == ['First answer', 'Second answer']
        assert len(client.queries) == 2
        assert client.queries[0][1] == client.queries[1][1]
    asyncio.run(check())


@pytest.mark.skipif(sys.platform != 'linux', reason='setsid tool process cleanup requires Linux')
def test_sdk_wrapper_reaps_background_tool_after_cli_crash(tmp_path):
    async def check():
        script = tmp_path / 'cli.py'
        script.write_text("#!/usr/bin/env python3\nimport subprocess,sys\np=subprocess.Popen([sys.executable,'-c','import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); time.sleep(60)'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)\nprint(p.pid,flush=True)\n", encoding='utf-8')
        script.chmod(0o700)
        group = ClaudeProcessGroup(str(script))
        leader = await asyncio.create_subprocess_exec(group.prepare(), stdout=asyncio.subprocess.PIPE)
        child = int(await leader.stdout.readline())
        await asyncio.wait_for(leader.wait(), 5)
        await group.close()
        status = Path('/proc') / str(child) / 'stat'
        for _ in range(100):
            if not status.exists() or status.read_text().split()[2] == 'Z': break
            await asyncio.sleep(.01)
        assert not status.exists() or status.read_text().split()[2] == 'Z'
    asyncio.run(check())
