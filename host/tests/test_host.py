import asyncio
import json
import os
from pathlib import Path

import pytest

from agentdesk_host.adapters.base import AdapterError, RunContext
from agentdesk_host.adapters.fake import FakeAdapter
from agentdesk_host.server.service import Client, HostService, MAX_FRAME, encoded


async def connected(service, device="device_1"):
    messages = []
    async def send(message):
        messages.append(message)
    client = Client(send)
    hello = await service.request(client, "hello", {"protocolVersion": "1.9", "client": "test", "deviceId": device, "deviceName": "Desktop", "os": "windows"})
    assert hello["protocolVersion"] == "1.0"
    return client, messages


async def setup(home, project_path):
    service = HostService(home, {"fake": FakeAdapter()})
    client, messages = await connected(service)
    await service.request(client, "project.upsert", {"id": "project", "path": str(project_path), "name": "Project"})
    session = await service.request(client, "session.create", {"projectId": "project", "agent": "fake", "model": "fake-default", "permissionMode": "default"})
    await service.request(client, "sync.subscribe", {"sessionId": session["id"]})
    return service, client, messages, session["id"]


async def finish(service):
    if service.watchers:
        await asyncio.wait_for(asyncio.gather(*tuple(service.watchers)), 5)


def test_handshake_required_and_major_rejected(tmp_path):
    async def check():
        service = HostService(tmp_path)
        async def send(_): pass
        client = Client(send)
        with pytest.raises(AdapterError, match="Handshake"):
            await service.request(client, "project.list", {})
        with pytest.raises(AdapterError) as error:
            await service.request(client, "hello", {"protocolVersion": "2.0"})
        assert error.value.code == "E_PROTOCOL_VERSION"
        one, _ = await connected(service)
        host_id = service.db.host_id
        await service.disconnect(one)
        await service.close()
        reopened = HostService(tmp_path)
        assert reopened.db.host_id == host_id
        await reopened.close()
    asyncio.run(check())


def test_idempotency_monotonic_events_and_no_persisted_deltas(tmp_path):
    async def check():
        service, client, messages, sid = await setup(tmp_path / "home", tmp_path)
        params = {"sessionId": sid, "clientMsgId": "msg", "text": "Hello", "script": [{"type": "text", "text": "streamed response", "chunks": 6}]}
        first, second = await asyncio.gather(service.request(client, "run.send", params), service.request(client, "run.send", params))
        assert first == second
        await finish(service)
        events = [item["params"] for item in messages]
        assert any(event["type"] == "item.delta" for event in events)
        seqs = [event["seq"] for event in events]
        assert seqs == sorted(set(seqs))
        assert len(service.db.all("SELECT * FROM runs")) == 1
        assert not service.db.all("SELECT * FROM events WHERE type='item.delta'")
        completed = [event for event in events if event["type"] == "item.completed"]
        assert completed[0]["payload"]["text"] == "streamed response"
        assert events[-1]["payload"]["state"] == "completed"
        await service.close()
    asyncio.run(check())


def test_snapshot_reconnect_pagination_and_no_omitted_events(tmp_path):
    async def check():
        service, client, _, sid = await setup(tmp_path / "home", tmp_path)
        ready, release = asyncio.Event(), asyncio.Event()
        broadcast = service._broadcast
        async def pause_after_second_delta(event):
            await broadcast(event)
            if event["type"] == "item.delta" and event["payload"].get("delta") == "b":
                ready.set()
                await release.wait()
        service._broadcast = pause_after_second_delta
        await service.request(client, "run.send", {"sessionId": sid, "clientMsgId": "msg", "text": "Hello", "script": [{"type": "text", "text": "abcdefghij", "chunks": 10, "delay": 0.02}]})
        await asyncio.wait_for(ready.wait(), 5)
        synced = await service.request(client, "sync.since", {"sessionId": sid, "lastSeq": 0})
        assert synced["snapshots"][0]["payload"]["text"].startswith("ab")
        assert synced["snapshots"][0]["payload"]["snapshot"]
        watermark = synced["currentSeq"]
        release.set()
        await service.disconnect(client)
        await finish(service)
        new_client, _ = await connected(service, "device_2")
        resumed = await service.request(new_client, "sync.since", {"sessionId": sid, "lastSeq": watermark})
        assert any(event["type"] == "item.completed" for event in resumed["events"])
        assert not resumed["snapshots"]
        cursor, seqs = 0, []
        while True:
            page = await service.request(new_client, "sync.since", {"sessionId": sid, "lastSeq": cursor, "limit": 1})
            seqs.extend(event["seq"] for event in page["events"])
            assert page["currentSeq"] >= cursor
            cursor = page["currentSeq"]
            if not page["hasMore"]: break
        assert seqs == [row["seq"] for row in service.db.all("SELECT seq FROM events ORDER BY seq")]
        history = await service.request(new_client, "session.history", {"sessionId": sid, "limit": 2})
        assert history["hasMore"]
        assert history["events"][0]["seq"] < history["events"][1]["seq"]
        older = await service.request(new_client, "session.history", {"sessionId": sid, "beforeSeq": history["nextBeforeSeq"]})
        assert all(event["seq"] < history["nextBeforeSeq"] for event in older["events"])
        await service.close()
    asyncio.run(check())


def test_first_approval_wins_and_device_cannot_be_impersonated(tmp_path):
    async def check():
        service, first, _, sid = await setup(tmp_path / "home", tmp_path)
        second, _ = await connected(service, "device_2")
        await service.request(first, "run.send", {"sessionId": sid, "clientMsgId": "msg", "text": "Hello", "script": [{"type": "question"}]})
        await asyncio.sleep(0.01)
        approval_id = service.db.one("SELECT id FROM approvals")["id"]
        decision = {"behavior": "allow", "answers": {"choice": ["one", "two"]}}
        results = await asyncio.gather(service.request(first, "approval.respond", {"sessionId": sid, "approvalId": approval_id, "decision": decision, "deviceId": "forged"}), service.request(second, "approval.respond", {"sessionId": sid, "approvalId": approval_id, "decision": {"behavior": "deny"}}), return_exceptions=True)
        assert results[0]["decision"] == decision
        assert isinstance(results[1], AdapterError)
        assert results[1].code == "E_ALREADY_RESOLVED"
        assert results[1].decision == decision
        assert service.db.one("SELECT decided_by_device FROM approvals")["decided_by_device"] == "device_1"
        await finish(service)
        await service.close()
    asyncio.run(check())


def test_stop_resume_append_and_busy(tmp_path):
    async def check():
        service, client, _, sid = await setup(tmp_path / "home", tmp_path)
        run = await service.request(client, "run.send", {"sessionId": sid, "clientMsgId": "msg", "text": "Hello", "script": [{"type": "wait_input"}, {"type": "background", "seconds": 60}]})
        with pytest.raises(AdapterError) as busy:
            await service.request(client, "run.send", {"sessionId": sid, "clientMsgId": "msg2", "text": "Hello"})
        assert busy.value.code == "E_SESSION_BUSY"
        with pytest.raises(AdapterError) as deletion:
            await service.request(client, "session.delete", {"sessionId": sid})
        assert deletion.value.code == "E_SESSION_BUSY"
        await service.request(client, "run.append", {"sessionId": sid, "runId": run["runId"], "clientMsgId": "append", "text": "appended"})
        await asyncio.sleep(0.03)
        await service.request(client, "run.stop", {"sessionId": sid, "runId": run["runId"]})
        await finish(service)
        assert service.db.one("SELECT state FROM runs")["state"] == "interrupted"
        native_id = service._session(sid)["nativeId"]
        await service.request(client, "run.send", {"sessionId": sid, "clientMsgId": "msg3", "text": "Resume", "script": []})
        await finish(service)
        assert service._session(sid)["nativeId"] == native_id
        assert service.db.one("SELECT count(*) AS n FROM runs")["n"] == 2
        await service.request(client, "session.delete", {"sessionId": sid})
        assert not service.db.all("SELECT * FROM sessions")
        await service.close()
    asyncio.run(check())


def test_large_output_truncation_blob_unicode_and_frame_bound(tmp_path):
    async def check():
        service, client, messages, sid = await setup(tmp_path / "home", tmp_path)
        await service.request(client, "run.send", {"sessionId": sid, "clientMsgId": "msg", "text": "Hello", "script": [{"type": "text", "text": "中文🙂" * 400000, "chunks": 1, "delay": 0.05}]})
        await asyncio.sleep(0.01)
        snapshot = await service.request(client, "sync.since", {"sessionId": sid, "lastSeq": 0})
        assert len(encoded(snapshot)) < MAX_FRAME
        assert not service.db.all("SELECT * FROM event_blobs")
        await finish(service)
        completed = next(message["params"] for message in messages if message["params"]["type"] == "item.completed")
        assert completed["payload"]["truncated"]
        assert all(len(encoded(message)) < MAX_FRAME for message in messages)
        offset, chunks = 0, []
        while True:
            blob = await service.request(client, "event.blob", {"sessionId": sid, "seq": completed["seq"], "offset": offset, "limit": 100003})
            chunks.append(blob["text"])
            offset = blob["nextOffset"]
            if not blob["hasMore"]: break
        assert json.loads("".join(chunks))["text"] == "中文🙂" * 400000
        assert service.db.one("SELECT count(*) AS n FROM event_blobs")["n"] == 1
        await service.close()
    asyncio.run(check())


def test_restart_interruption_and_sequence_survives(tmp_path):
    async def check():
        service, client, _, sid = await setup(tmp_path / "home", tmp_path)
        run = await service.request(client, "run.send", {"sessionId": sid, "clientMsgId": "msg", "text": "Hello", "script": [{"type": "sleep", "seconds": 60}]})
        seq = service.db.current_seq
        # Simulate abrupt daemon death without cleanup of the persisted run.
        for _, handle in service.handles.values(): handle.task.cancel()
        for watcher in service.watchers: watcher.cancel()
        await asyncio.gather(*tuple(service.watchers), return_exceptions=True)
        service.db.execute("UPDATE runs SET finished_at=NULL,state='running' WHERE id=?", (run["runId"],))
        service.db.commit()
        service.db.close()
        reopened = HostService(tmp_path / "home", {"fake": FakeAdapter()})
        assert reopened.db.current_seq > seq
        assert reopened.db.one("SELECT state FROM runs")["state"] == "interrupted"
        assert reopened._session(sid)["status"] == "idle"
        await reopened.close()
    asyncio.run(check())


@pytest.mark.skipif(os.name == "nt", reason="Unix socket requires the Linux test server")
def test_real_unix_socket_ndjson_and_permissions(socket_home):
    async def check():
        from agentdesk_host.server.transport import HostServer
        service = HostService(socket_home / "home", {"fake": FakeAdapter()})
        server = HostServer(service)
        await server.start()
        assert server.socket_path.stat().st_mode & 0o777 == 0o600
        reader, writer = await asyncio.open_unix_connection(server.socket_path)
        writer.write(b'{"jsonrpc":"2.0","id":1,"method":"hello","params":{"protocolVersion":"1.0","client":"test","deviceId":"dev","deviceName":"Desktop","os":"linux"}}\n')
        await writer.drain()
        reply = json.loads(await reader.readline())
        assert reply["result"]["hostId"].startswith("host_")
        writer.write(b'{invalid\n')
        await writer.drain()
        assert json.loads(await reader.readline())["error"]["code"] == -32700
        writer.close()
        await writer.wait_closed()
        await server.close()
        await service.close()
    asyncio.run(check())


def test_active_run_snapshot_restores_stop_target_at_current_cursor(tmp_path):
    async def check():
        service, client, _, sid = await setup(tmp_path / "home", tmp_path)
        run = await service.request(client, "run.send", {"sessionId": sid, "clientMsgId": "active", "text": "Work", "script": [{"type": "background", "seconds": 60}]})
        await asyncio.sleep(0.01)
        cursor = service.db.current_seq
        await service.disconnect(client)
        new_client, _ = await connected(service, "new_window")
        snapshot = await service.request(new_client, "sync.since", {"sessionId": sid, "lastSeq": cursor})
        assert snapshot["events"] == []
        assert snapshot["snapshots"][-1]["type"] == "run.state"
        assert snapshot["snapshots"][-1]["seq"] == cursor
        assert snapshot["snapshots"][-1]["runId"] == run["runId"]
        assert snapshot["snapshots"][-1]["payload"] == {"state": "awaiting_background", "snapshot": True}
        await service.request(new_client, "run.stop", {"sessionId": sid, "runId": run["runId"]})
        await finish(service)
        assert service.db.one("SELECT state FROM runs")["state"] == "interrupted"
        await service.close()
    asyncio.run(check())


def test_server_approval_request_response_is_bound_to_receiving_device(tmp_path):
    async def check():
        service, first, messages, sid = await setup(tmp_path / "home", tmp_path)
        second, _ = await connected(service, "other")
        await service.request(first, "run.send", {"sessionId": sid, "clientMsgId": "question", "text": "Question", "script": [{"type": "question"}]})
        await asyncio.sleep(0.01)
        request = next(message for message in messages if message.get("method") == "approval.request")
        notification = next(message for message in messages if message.get("method") == "event" and message["params"]["type"] == "approval.request")
        assert request["params"] == notification["params"]
        decision = {"behavior": "allow", "answers": {"choice": "continue"}}
        await service.handle_response(second, {"id": request["id"], "result": {"decision": decision}})
        assert service.db.one("SELECT state FROM approvals")["state"] == "pending"
        await service.handle_response(first, {"id": request["id"], "result": {"decision": decision}})
        await finish(service)
        assert service.db.one("SELECT decided_by_device FROM approvals")["decided_by_device"] == "device_1"
        assert service.db.one("SELECT state FROM runs")["state"] == "completed"
        await service.close()
    asyncio.run(check())


def test_fake_long_output_size_is_total_characters_not_repeat_count(tmp_path):
    async def check():
        events = []
        async def emit(kind, payload, item_id=None):
            events.append((kind, payload))
        async def approve(kind, payload):
            return {"behavior": "allow"}
        async def save_native(value):
            pass
        handle = await FakeAdapter().start_run({}, {"script": [{"type": "long_output", "text": "abcd", "size": 17}]}, RunContext("run", emit, approve, save_native))
        await handle.task
        completed = next(payload for kind, payload in events if kind == "item.completed")
        assert completed["text"] == "abcdabcdabcdabcda"
    asyncio.run(check())


def test_legacy_login_is_removed_and_logout_refreshes_status(tmp_path):
    class AccountAdapter(FakeAdapter):
        def __init__(self):
            self.logout_calls = 0
        async def login_command(self):
            return ["/home/user/Agent Host/bin/codex", "login", "--device-auth"]
        async def logout(self):
            self.logout_calls += 1
        async def capabilities(self):
            return {**await super().capabilities(), "loggedIn": self.logout_calls == 0, "authStatus": "loggedIn" if self.logout_calls == 0 else "loggedOut"}

    async def check():
        adapter = AccountAdapter()
        service = HostService(tmp_path / "home", {"account": adapter})
        client, _ = await connected(service)
        with pytest.raises(AdapterError):
            await service.request(client, "agent.login.start", {"agent": "account"})
        assert adapter.logout_calls == 0
        result = await service.request(client, "agent.logout", {"agent": "account"})
        assert result["loggedOut"] is True
        assert result["capability"]["authStatus"] == "loggedOut"
        assert adapter.logout_calls == 1
        service.adapters["fake"] = FakeAdapter()
        with pytest.raises(AdapterError) as error:
            await service.request(client, "login.start", {"agent": "fake"})
        assert error.value.code == "E_CAPABILITY"
        await service.close()
    asyncio.run(check())


def test_run_acceptance_is_atomic_when_event_persistence_fails(tmp_path):
    async def check():
        service, client, messages, sid = await setup(tmp_path / "home", tmp_path)
        original = service._record
        def record(session_id, run_id, event_type, payload, item_id=None, **kwargs):
            if event_type == "run.started":
                raise RuntimeError("Simulated storage failure")
            return original(session_id, run_id, event_type, payload, item_id, **kwargs)
        service._record = record
        with pytest.raises(RuntimeError, match="storage failure"):
            await service.request(client, "run.send", {"sessionId": sid, "clientMsgId": "atomic", "text": "Hello", "script": []})
        assert not service.db.all("SELECT * FROM runs")
        assert not service.db.all("SELECT * FROM client_msgs")
        assert not service.db.all("SELECT * FROM events")
        assert service._session(sid)["status"] == "idle"
        assert not messages
        service._record = original
        await service.request(client, "run.send", {"sessionId": sid, "clientMsgId": "atomic", "text": "Hello", "script": []})
        await finish(service)
        assert len(service.db.all("SELECT * FROM runs")) == 1
        await service.close()
    asyncio.run(check())


def test_run_acceptance_commits_message_and_started_before_first_push(tmp_path):
    async def check():
        service, client, _, sid = await setup(tmp_path / "home", tmp_path)
        observed = []
        async def send(message):
            if message.get("method") == "event" and message["params"]["type"] == "user.message":
                import sqlite3
                connection = sqlite3.connect(tmp_path / "home/state/host.sqlite")
                try:
                    observed.append(tuple(connection.execute(f"SELECT count(*) FROM {table}").fetchone()[0] for table in ("runs", "client_msgs", "events")))
                finally:
                    connection.close()
        client.send = send
        await service.request(client, "run.send", {"sessionId": sid, "clientMsgId": "atomic", "text": "Hello", "script": []})
        await finish(service)
        assert observed == [(1, 1, 2)]
        await service.close()
    asyncio.run(check())
