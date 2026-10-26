import asyncio
import json
import logging

import pytest

from agentdesk_host.adapters.base import AdapterError
from agentdesk_host.adapters.fake import FakeAdapter
from agentdesk_host.server.service import HostService, MAX_EVENT
from test_host import connected, finish, setup


def test_large_write_approval_is_complete_in_storage_and_blob(tmp_path):
    async def check():
        service, client, messages, sid = await setup(tmp_path / 'home', tmp_path)
        run = await service.request(client, 'run.send', {'sessionId': sid, 'clientMsgId': 'm', 'text': 'Write', 'script': [{'type': 'sleep', 'seconds': 60}]})
        payload = {'tool': 'Write', 'input': {'file_path': '/project/file', 'content': '中文diff\n' * 40000}}
        task = asyncio.create_task(service._approve(sid, run['runId'], 'tool', payload))
        await asyncio.sleep(0)
        approval = service.db.one('SELECT * FROM approvals')
        assert json.loads(approval['payload_json']) == payload
        request = next(message for message in messages if message.get('method') == 'approval.request')
        assert request['params']['payload']['tool'] == 'Write'
        assert request['params']['payload']['input']
        assert request['params']['payload']['blob']
        blob = service._blob({'sessionId': sid, 'seq': request['params']['seq'], 'limit': 512 * 1024})
        parts = [blob['text']]
        while blob['hasMore']:
            blob = service._blob({'sessionId': sid, 'seq': request['params']['seq'], 'offset': blob['nextOffset'], 'limit': 512 * 1024})
            parts.append(blob['text'])
        assert json.loads(''.join(parts))['input'] == payload['input']
        # Server requests derive their content from approvals, never the event preview.
        service.db.execute("UPDATE events SET payload_json=? WHERE seq=?", (json.dumps({'approvalId': approval['id']}), request['params']['seq']))
        service.db.commit()
        messages.clear()
        await service._server_approval(client, sid, approval['id'], 'tool', {})
        assert messages[-1]['params']['payload']['tool'] == 'Write'
        await service.request(client, 'approval.respond', {'sessionId': sid, 'approvalId': approval['id'], 'decision': {'behavior': 'deny'}})
        assert await task == {'behavior': 'deny'}
        await service.close()
    asyncio.run(check())


def test_questions_survive_event_preview_without_truncation(tmp_path):
    async def check():
        service, client, messages, sid = await setup(tmp_path / 'home', tmp_path)
        run = await service.request(client, 'run.send', {'sessionId': sid, 'clientMsgId': 'm', 'text': 'Question', 'script': [{'type': 'sleep', 'seconds': 60}]})
        questions = [{'question': 'q' * MAX_EVENT, 'header': 'Q', 'options': [{'label': 'Continue', 'description': 'Use this option'}], 'multiSelect': False}]
        task = asyncio.create_task(service._approve(sid, run['runId'], 'question', {'tool': 'AskUserQuestion', 'input': {'questions': questions}, 'questions': questions}))
        await asyncio.sleep(0)
        requests = [message for message in messages if message.get('method') == 'approval.request']
        assert requests[0]['params']['payload']['questions'] == questions
        event = service.db.one("SELECT * FROM events WHERE type='approval.request'")
        assert json.loads(event['payload_json'])['questions'] == questions
        await service.request(client, 'approval.respond', {'sessionId': sid, 'approvalId': service.db.one('SELECT id FROM approvals')['id'], 'decision': {'behavior': 'deny'}})
        await task
        await service.close()
    asyncio.run(check())


def test_append_is_persisted_and_idempotent_after_run_finishes(tmp_path):
    async def check():
        service, client, _, sid = await setup(tmp_path / 'home', tmp_path)
        run = await service.request(client, 'run.send', {'sessionId': sid, 'clientMsgId': 'm', 'text': 'Start', 'script': [{'type': 'wait_input'}]})
        append = {'sessionId': sid, 'runId': run['runId'], 'clientMsgId': 'a', 'text': 'Continue'}
        first = await service.request(client, 'run.append', append)
        await finish(service)
        assert await service.request(client, 'run.append', append) == first
        messages = service.db.all("SELECT payload_json FROM events WHERE type='user.message'")
        assert [json.loads(message['payload_json'])['clientMsgId'] for message in messages] == ['m', 'a']
        await service.close()
    asyncio.run(check())


def test_real_session_archive_restore_preserves_native_and_events(tmp_path):
    async def check():
        service, client, _, sid = await setup(tmp_path / 'home', tmp_path)
        class Native(FakeAdapter):
            async def native_exists(self, session): return True
        service.adapters['native'] = Native()
        service.db.execute("UPDATE sessions SET agent='native',native_id='native-record' WHERE id=?", (sid,))
        service.db.commit()
        service._record(sid, None, 'user.message', {'text': 'Preserve'})
        assert await service.request(client, 'session.delete', {'sessionId': sid}) == {'archived': True}
        assert await service.request(client, 'session.list', {'projectId': 'project'}) == []
        assert len(await service.request(client, 'session.list', {'projectId': 'project', 'archived': True})) == 1
        assert (await service.request(client, 'session.restore', {'sessionId': sid}))['nativeId'] == 'native-record'
        assert service.db.one('SELECT count(*) AS n FROM events')['n'] == 1
        await service.close()
    asyncio.run(check())


def test_capability_probe_is_coalesced_and_hello_does_not_wait(tmp_path):
    class Slow(FakeAdapter):
        def __init__(self): self.calls = 0; self.release = asyncio.Event()
        async def capabilities(self):
            self.calls += 1
            await self.release.wait()
            return await super().capabilities()
    async def check():
        adapter = Slow()
        service = HostService(tmp_path, {'fake': adapter})
        await asyncio.wait_for(connected(service), .2)
        tasks = [asyncio.create_task(service.capabilities()) for _ in range(4)]
        await asyncio.sleep(0)
        assert adapter.calls == 1
        adapter.release.set()
        await asyncio.gather(*tasks)
        await service.capabilities()
        assert adapter.calls == 1
        await service.close()
    asyncio.run(check())


def test_recovery_completes_unfinished_items_and_deltas_have_no_fake_blob(tmp_path):
    async def check():
        service, client, _, sid = await setup(tmp_path / 'home', tmp_path)
        run = await service.request(client, 'run.send', {'sessionId': sid, 'clientMsgId': 'm', 'text': 'Start', 'script': [{'type': 'sleep', 'seconds': 60}]})
        await service.emit(sid, run['runId'], 'item.started', {'kind': 'text'}, 'unfinished')
        event = service._record(sid, run['runId'], 'item.delta', {'kind': 'text', 'delta': 'x' * (MAX_EVENT + 1)}, 'unfinished')
        assert not event['payload'].get('blob')
        # Graceful close also persists completed items without snapshot flags.
        await service.close()
        reopened = HostService(tmp_path / 'home', {'fake': FakeAdapter()})
        completed = reopened.db.all("SELECT payload_json FROM events WHERE type='item.completed'")
        assert completed and all('snapshot' not in json.loads(row['payload_json']) for row in completed)
        # Simulate an unfinished persisted item followed by an abrupt restart.
        reopened.db.execute("UPDATE runs SET finished_at=NULL,state='running' WHERE id=?", (run['runId'],))
        reopened.db.commit()
        reopened._record(sid, run['runId'], 'item.started', {'kind': 'tool_call', 'name': 'Bash'}, 'orphan')
        reopened.db.close()
        recovered = HostService(tmp_path / 'home', {'fake': FakeAdapter()})
        item = recovered.db.one("SELECT payload_json FROM events WHERE item_id='orphan' AND type='item.completed'")
        assert json.loads(item['payload_json'])['interrupted']
        await recovered.close()
    asyncio.run(check())


def test_unsubscribe_and_unavailable_send(tmp_path):
    async def check():
        service, client, messages, sid = await setup(tmp_path / 'home', tmp_path)
        assert await service.request(client, 'sync.unsubscribe', {'sessionId': sid}) == {'unsubscribed': True}
        await service.emit(sid, None, 'session.updated', {'title': 'Changed'})
        assert messages == []
        service.capability_cache['fake']['available'] = False
        with pytest.raises(AdapterError) as error:
            await service.request(client, 'run.send', {'sessionId': sid, 'clientMsgId': 'm', 'text': 'Start'})
        assert error.value.code == 'E_CAPABILITY'
        await service.close()
    asyncio.run(check())


def test_late_unsubscribe_only_releases_its_own_subscription_token(tmp_path):
    async def check():
        service, client, messages, sid = await setup(tmp_path / 'home', tmp_path)
        await service.request(client, 'sync.unsubscribe', {'sessionId': sid})
        await service.request(client, 'sync.subscribe', {'sessionId': sid, 'subscriptionId': 'old'})
        await service.request(client, 'sync.subscribe', {'sessionId': sid, 'subscriptionId': 'new'})
        await service.request(client, 'sync.unsubscribe', {'sessionId': sid, 'subscriptionId': 'old'})
        await service.emit(sid, None, 'session.updated', {'title': 'Still subscribed'})
        assert len(messages) == 1 and sid in client.subscriptions
        # Repeated stale cleanup also cannot release the new subscription.
        await service.request(client, 'sync.unsubscribe', {'sessionId': sid, 'subscriptionId': 'old'})
        assert client.subscription_tokens[sid] == {'new'}
        await service.request(client, 'sync.unsubscribe', {'sessionId': sid, 'subscriptionId': 'new'})
        await service.emit(sid, None, 'session.updated', {'title': 'Released'})
        assert len(messages) == 1 and sid not in client.subscriptions
        await service.close()
    asyncio.run(check())


def test_transport_write_timeout_closes_connection_and_reports_reason(tmp_path, monkeypatch, caplog):
    from agentdesk_host.server import transport
    class Writer:
        closed = False
        def write(self, data): pass
        async def drain(self): await asyncio.sleep(60)
        def close(self): self.closed = True
        async def wait_closed(self): pass
    class Reader:
        sent = False
        async def readline(self):
            if not self.sent:
                self.sent = True
                return b'{invalid\n'
            await asyncio.sleep(60)
    async def check():
        service = HostService(tmp_path)
        server = transport.HostServer(service)
        writer = Writer()
        monkeypatch.setattr(transport, 'DRAIN_TIMEOUT', .01)
        with caplog.at_level(logging.WARNING):
            await asyncio.wait_for(server._accept(Reader(), writer), 3)
        assert writer.closed and 'write_timeout' in caplog.text
        assert not server.connections and not server.writers
        await service.close()
    asyncio.run(check())
