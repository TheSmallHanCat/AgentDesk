"""Host application service. Native agent records remain owned by their CLIs."""
from __future__ import annotations

import asyncio
import json
import os
import platform
import shlex
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Awaitable, Callable

from .. import __version__
from ..adapters.base import AdapterError, AgentAdapter, RunContext, RunHandle
from ..store.database import Database, now

MAX_FRAME = 4 * 1024 * 1024
MAX_EVENT = 96 * 1024
MAX_PAGE = 2 * 1024 * 1024


def encoded(value: Any) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def fail(code: str, stage: str, message: str, hint: str = "") -> AdapterError:
    return AdapterError(code, stage, message, hint)


def required(params: dict[str, Any], key: str) -> str:
    value = params.get(key)
    if not isinstance(value, str) or not value or len(value) > 2_000_000:
        raise fail("E_INVALID_PARAMS", "request.validate", f"Invalid {key}", "Provide a nonempty string")
    return value


def integer(params: dict[str, Any], key: str, default: int, maximum: int) -> int:
    value = params.get(key, default)
    if type(value) is not int or value < 0 or value > maximum:
        raise fail("E_INVALID_PARAMS", "request.validate", f"Invalid {key}", f"Use an integer from 0 to {maximum}")
    return value


@dataclass(eq=False)
class Client:
    send: Callable[[dict[str, Any]], Awaitable[None]]
    device_id: str | None = None
    subscriptions: set[str] = field(default_factory=set)
    server_requests: dict[str, tuple[str, str]] = field(default_factory=dict)


class HostService:
    def __init__(self, home: Path, adapters: dict[str, AgentAdapter] | None = None):
        self.home = Path(home)
        self.home.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.db = Database(self.home / "state" / "host.sqlite")
        if os.name != "nt":
            os.chmod(self.home / "state", 0o700)
            os.chmod(self.home / "state" / "host.sqlite", 0o600)
        self.adapters = adapters or {}
        self.clients: set[Client] = set()
        self.handles: dict[str, tuple[AgentAdapter, RunHandle]] = {}
        self.watchers: set[asyncio.Task] = set()
        self.approval_waiters: dict[str, asyncio.Future] = {}
        self.snapshots: dict[tuple[str, str], dict[str, Any]] = {}
        self.mutation_lock = asyncio.Lock()
        self.draining_until = 0.0
        self.draining_device: str | None = None
        self.last_activity = asyncio.get_event_loop().time()
        self._recover()

    def _recover(self) -> None:
        for run in self.db.all("SELECT id,session_id FROM runs WHERE finished_at IS NULL"):
            self.db.execute("UPDATE runs SET state='interrupted',finished_at=?,stop_reason='host_restart' WHERE id=?", (now(), run["id"]))
            self.db.execute("UPDATE sessions SET status='idle',updated_at=? WHERE id=?", (now(), run["session_id"]))
            self._record(run["session_id"], run["id"], "run.finished", {"state": "interrupted", "reason": "host_restart"})
        self.db.execute("UPDATE approvals SET state='resolved',decision_json=? WHERE state='pending'", (json.dumps({"behavior": "deny", "reason": "host_restart"}),))
        self.db.execute("UPDATE devices SET online=0")
        self.db.commit()

    async def capabilities(self) -> dict[str, Any]:
        result = {}
        for agent, adapter in self.adapters.items():
            try:
                result[agent] = await adapter.capabilities()
            except Exception:
                result[agent] = {"installed": False, "available": False, "loggedIn": False, "streaming": False, "reason": "Agent detection failed"}
        return result

    def _session(self, session_id: str) -> dict[str, Any]:
        row = self.db.one("SELECT s.*,p.path FROM sessions s JOIN projects p ON p.id=s.project_id WHERE s.id=? AND s.deleted_at IS NULL", (session_id,))
        if not row:
            raise fail("E_NOT_FOUND", "session.lookup", "Session does not exist", "Refresh the session list")
        return {"id": row["id"], "projectId": row["project_id"], "agent": row["agent"], "nativeId": row["native_id"], "title": row["title"], "model": row["model"], "permissionMode": row["permission_mode"], "status": row["status"], "createdAt": row["created_at"], "updatedAt": row["updated_at"], "path": row["path"]}

    def _project(self, project_id: str) -> dict[str, Any]:
        row = self.db.one("SELECT * FROM projects WHERE id=?", (project_id,))
        if not row:
            raise fail("E_NOT_FOUND", "project.lookup", "Project does not exist", "Register the remote project")
        return {"id": row["id"], "kind": row["kind"], "path": row["path"], "name": row["display_name"], "defaultAgent": row["default_agent"], "createdAt": row["created_at"], "archived": row["archived_at"] is not None}

    def _record(self, session_id: str, run_id: str | None, event_type: str, payload: dict[str, Any], item_id: str | None = None, *, seq: int | None = None, commit: bool = True) -> dict[str, Any]:
        seq = self.db.next_seq() if seq is None else seq
        blob = encoded(payload)
        if len(blob) > MAX_EVENT:
            original = payload
            payload = {"kind": original.get("kind", "text"), "truncated": True, "originalBytes": len(blob), "blob": True}
            for key in ("text", "delta", "output", "name", "approvalId"):
                if key in original:
                    value = original[key]
                    payload[key] = value[:16000] if isinstance(value, str) else encoded(value)[:16000].decode("utf-8", errors="ignore")
            if event_type != "item.delta":
                self.db.execute("INSERT INTO event_blobs VALUES(?,?,?)", (seq, session_id, blob))
        event = {"seq": seq, "sessionId": session_id, "ts": now(), "type": event_type, "payload": payload}
        if run_id:
            event["runId"] = run_id
        if item_id:
            event["itemId"] = item_id
        if event_type != "item.delta":
            self.db.execute("INSERT INTO events VALUES(?,?,?,?,?,?,?)", (seq, session_id, run_id, event["ts"], event_type, item_id, json.dumps(payload, ensure_ascii=False)))
        if commit:
            self.db.commit()
        return event

    async def emit(self, session_id: str, run_id: str | None, event_type: str, payload: dict[str, Any], item_id: str | None = None) -> None:
        if event_type == "run.state" and run_id and isinstance(payload.get("state"), str):
            self.db.execute("UPDATE runs SET state=? WHERE id=?", (payload["state"], run_id))
            self.db.execute("UPDATE sessions SET status=?,updated_at=? WHERE id=?", (payload["state"], now(), session_id))
        event = self._record(session_id, run_id, event_type, payload, item_id)
        if item_id:
            key = (session_id, item_id)
            if event_type == "item.started":
                self.snapshots[key] = {**event, "payload": {**payload, "text": payload.get("text", ""), "snapshot": True}}
            elif event_type == "item.delta":
                previous = self.snapshots.get(key, {**event, "type": "item.started", "payload": {"kind": payload.get("kind", "text"), "text": "", "snapshot": True}})
                previous["seq"] = event["seq"]
                previous["payload"]["text"] += str(payload.get("delta", ""))
                self.snapshots[key] = previous
            elif event_type == "item.completed":
                self.snapshots.pop(key, None)
        await self._broadcast(event)

    async def _broadcast(self, event: dict[str, Any]) -> None:
        for client in tuple(self.clients):
            if event["sessionId"] in client.subscriptions:
                try:
                    await client.send({"jsonrpc": "2.0", "method": "event", "params": event})
                except ConnectionError:
                    self.clients.discard(client)
        self.last_activity = asyncio.get_event_loop().time()

    async def disconnect(self, client: Client) -> None:
        self.clients.discard(client)
        if client.device_id and not any(other.device_id == client.device_id for other in self.clients):
            self.db.execute("UPDATE devices SET online=0,last_seen_at=? WHERE id=?", (now(), client.device_id))
            self.db.commit()
        self.last_activity = asyncio.get_event_loop().time()

    async def request(self, client: Client, method: str, params: dict[str, Any]) -> Any:
        self.last_activity = asyncio.get_event_loop().time()
        if method == "hello":
            if client.device_id:
                raise fail("E_PROTOCOL_VERSION", "hello", "Handshake already completed", "Open a new connection")
            version = required(params, "protocolVersion")
            if version.split(".", 1)[0] != "1":
                raise fail("E_PROTOCOL_VERSION", "hello", "Incompatible protocol major version", "Upgrade the desktop or Host")
            device_id = required(params, "deviceId")
            name = required(params, "deviceName")
            client_os = required(params, "os")
            required(params, "client")
            if client_os not in {"windows", "macos", "linux"}:
                raise fail("E_INVALID_PARAMS", "hello", "Unsupported client OS", "Use windows, macos or linux")
            client.device_id = device_id
            self.clients.add(client)
            self.db.execute("INSERT INTO devices VALUES(?,?,?,?,1) ON CONFLICT(id) DO UPDATE SET name=excluded.name,os=excluded.os,last_seen_at=excluded.last_seen_at,online=1", (device_id, name, client_os, now()))
            self.db.commit()
            return {"protocolVersion": "1.0", "host": __version__, "hostId": self.db.host_id, "capabilities": await self.capabilities(), "currentSeq": self.db.current_seq}
        if not client.device_id:
            raise fail("E_PROTOCOL_VERSION", "hello", "Handshake is required", "Call hello before business methods")
        if method == "host.prepareShutdown":
            async with self.mutation_lock:
                if self.handles or self.db.one("SELECT id FROM runs WHERE finished_at IS NULL LIMIT 1"):
                    raise fail("E_SESSION_BUSY", method, "Host has active runs", "Stop active runs before maintenance")
                clock = asyncio.get_running_loop().time()
                if self.draining_until > clock and self.draining_device != client.device_id:
                    raise fail("E_HOST_DRAINING", method, "Another device is preparing maintenance", "Wait for maintenance to finish")
                self.draining_device = client.device_id
                self.draining_until = clock + 60
                return {"prepared": True, "leaseSeconds": 60}
        if method == "host.cancelShutdown":
            async with self.mutation_lock:
                if self.draining_until > asyncio.get_running_loop().time() and self.draining_device != client.device_id:
                    raise fail("E_TARGET", method, "Maintenance belongs to another device", "Wait for the maintenance lease to expire")
                self.draining_until = 0.0
                self.draining_device = None
                return {"cancelled": True}
        if method == "agent.status":
            return await self.capabilities()
        if method == "env.probe":
            return {"layers": [{"name": "system", "status": "ready", "detail": f"{platform.system()} {platform.machine()}"}, {"name": "host", "status": "ready", "detail": __version__}], "agents": await self.capabilities(), "host": {"installed": True, "running": True, "version": __version__, "hostId": self.db.host_id, "activeRuns": len(self.handles)}}
        if method in {"agent.login.start", "agent.logout"}:
            agent = required(params, "agent")
            adapter = self.adapters.get(agent)
            if not adapter:
                raise fail("E_AGENT_NOT_INSTALLED", method, "Agent is not configured", "Install the Agent from Environment")
            caps = await adapter.capabilities()
            if not caps.get("installed", False):
                raise fail("E_AGENT_NOT_INSTALLED", method, "Agent is not installed", "Install the Agent from Environment")
            if method == "agent.login.start":
                command = getattr(adapter, "login_command", None)
                if command is None:
                    raise fail("E_CAPABILITY", method, "Agent has no interactive login workflow")
                args = await command()
                if not isinstance(args, list) or not args or any(not isinstance(arg, str) or not arg for arg in args):
                    raise fail("E_CAPABILITY", method, "Agent login command is unavailable")
                return {"agent": agent, "transport": "pty", "command": shlex.join(args)}
            logout = getattr(adapter, "logout", None)
            if logout is None:
                raise fail("E_CAPABILITY", method, "Agent does not support logout")
            if self.db.one("SELECT r.id FROM runs r JOIN sessions s ON s.id=r.session_id WHERE s.agent=? AND r.finished_at IS NULL LIMIT 1", (agent,)):
                raise fail("E_SESSION_BUSY", method, "Agent has active runs", "Finish or stop all runs for this Agent before logging out")
            await logout()
            return {"loggedOut": True}
        if method == "project.list":
            return [self._project(row["id"]) for row in self.db.all("SELECT id FROM projects ORDER BY created_at")]
        if method == "project.upsert":
            return self._upsert_project(params)
        if method == "project.archive":
            project_id = required(params, "projectId")
            self._project(project_id)
            if type(params.get("archived")) is not bool:
                raise fail("E_INVALID_PARAMS", "project.archive", "Invalid archived flag")
            self.db.execute("UPDATE projects SET archived_at=? WHERE id=?", (now() if params["archived"] else None, project_id))
            self.db.commit()
            return self._project(project_id)
        if method == "session.list":
            project_id = required(params, "projectId")
            self._project(project_id)
            return [self._session(row["id"]) for row in self.db.all("SELECT id FROM sessions WHERE project_id=? AND deleted_at IS NULL ORDER BY updated_at DESC", (project_id,))]
        if method == "session.create":
            return await self._create_session(params)
        if method in {"session.rename", "session.delete"}:
            session = self._session(required(params, "sessionId"))
            if method == "session.rename":
                title = required(params, "title").strip()
                if not title or len(title) > 256:
                    raise fail("E_INVALID_PARAMS", "session.rename", "Invalid session title")
                self.db.execute("UPDATE sessions SET title=?,updated_at=? WHERE id=?", (title, now(), session["id"]))
                await self.emit(session["id"], None, "session.updated", {"title": title})
                return self._session(session["id"])
            return await self._delete_session(session)
        if method == "session.history":
            session_id = required(params, "sessionId")
            self._session(session_id)
            before = integer(params, "beforeSeq", self.db.current_seq + 1, 2**63 - 1)
            limit = max(1, integer(params, "limit", 50, 200))
            rows = self.db.all("SELECT * FROM events WHERE session_id=? AND seq<? ORDER BY seq DESC LIMIT ?", (session_id, before, limit + 1))
            events = self._bounded_events(rows[:limit])
            events.reverse()
            return {"events": events, "hasMore": len(rows) > len(events), **({"nextBeforeSeq": events[0]["seq"]} if events else {})}
        if method == "run.send":
            async with self.mutation_lock:
                return await self._send(client, params)
        if method in {"run.append", "run.stop"}:
            session_id = required(params, "sessionId")
            run_id = required(params, "runId")
            self._session(session_id)
            row = self.db.one("SELECT * FROM runs WHERE id=? AND session_id=?", (run_id, session_id))
            if not row or run_id not in self.handles:
                raise fail("E_NOT_FOUND", method, "Active run does not exist", "Refresh the session")
            adapter, handle = self.handles[run_id]
            if method == "run.append":
                await adapter.append_input(handle, required(params, "text"))
            else:
                await self.emit(session_id, run_id, "run.state", {"state": "stopping"})
                await adapter.stop(handle)
            return {"accepted": True}
        if method == "approval.respond":
            return await self._respond(client, params)
        if method == "sync.subscribe":
            session_id = required(params, "sessionId")
            self._session(session_id)
            client.subscriptions.add(session_id)
            for approval in self.db.all("SELECT a.id,a.kind,a.payload_json FROM approvals a JOIN runs r ON r.id=a.run_id WHERE r.session_id=? AND a.state='pending'", (session_id,)):
                await self._server_approval(client, session_id, approval["id"], approval["kind"], json.loads(approval["payload_json"]))
            return {"subscribed": True}
        if method == "sync.since":
            return self._sync(params)
        if method == "event.blob":
            return self._blob(params)
        if method in {"device.register", "device.tunnel.ready", "device.list", "session.fork", "run.stopTask", "run.setModel", "run.setPermissionMode", "run.setEffort", "run.compact"}:
            raise fail("E_CAPABILITY", method, "This capability is unavailable", "Use the supported desktop or native CLI workflow")
        raise fail("E_METHOD_NOT_FOUND", "request.dispatch", "Unknown method", "Use a method supported by this Host")

    def _bounded_events(self, rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
        events = []
        size = 0
        for row in rows:
            event = self.db.event(row)
            length = len(encoded(event))
            if events and size + length > MAX_PAGE:
                break
            size += length
            events.append(event)
        return events

    def _upsert_project(self, params: dict[str, Any]) -> dict[str, Any]:
        project_id = required(params, "id")
        path = Path(required(params, "path")).expanduser()
        if params.get("kind", "remote") != "remote":
            raise fail("E_CAPABILITY", "project.upsert", "Local projects require an execution device")
        if not path.is_absolute() or not path.is_dir():
            raise fail("E_NOT_FOUND", "project.upsert", "Remote project directory does not exist", "Create or select an existing directory")
        path_text = str(path.resolve())
        name = required(params, "name")
        if len(name) > 256:
            raise fail("E_INVALID_PARAMS", "project.upsert", "Project name is too long")
        existing = self.db.one("SELECT path FROM projects WHERE id=?", (project_id,))
        if existing and existing["path"] != path_text:
            raise fail("E_CONFLICT", "project.upsert", "Project path is immutable", "Register a separate project")
        self.db.execute("INSERT INTO projects(id,path,display_name,default_agent,created_at) VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET display_name=excluded.display_name,default_agent=excluded.default_agent", (project_id, path_text, name, params.get("defaultAgent"), now()))
        self.db.commit()
        return self._project(project_id)

    async def _create_session(self, params: dict[str, Any]) -> dict[str, Any]:
        project = self._project(required(params, "projectId"))
        if project["archived"]:
            raise fail("E_CONFLICT", "session.create", "Project is archived", "Restore the project before creating a session")
        agent = required(params, "agent")
        adapter = self.adapters.get(agent)
        if not adapter:
            raise fail("E_AGENT_NOT_INSTALLED", "session.create", "Agent is not configured", "Install or enable the Agent")
        capabilities = await adapter.capabilities()
        if not capabilities.get("installed", capabilities.get("available")):
            raise fail("E_AGENT_NOT_INSTALLED", "session.create", "Agent is not installed", "Install the Agent from Environment")
        if not capabilities.get("loggedIn", False):
            raise fail("E_NOT_LOGGED_IN", "session.create", "Agent is not logged in", "Complete official CLI login from Environment")
        if not capabilities.get("available", False):
            raise fail("E_CAPABILITY", "session.create", "Agent is unavailable", str(capabilities.get("reason", "Check Environment")))
        mode = required(params, "permissionMode")
        if mode not in capabilities.get("permissionModes", []):
            raise fail("E_CAPABILITY", "session.create", "Unsupported permission mode", "Choose an available mode")
        model = required(params, "model")
        models = [value.get("id") if isinstance(value, dict) else value for value in capabilities.get("models", [])]
        if models and model not in models:
            raise fail("E_INVALID_PARAMS", "session.create", "Unknown model", "Refresh the native model list")
        title = params.get("title", "New session")
        if not isinstance(title, str) or not title.strip() or len(title) > 256:
            raise fail("E_INVALID_PARAMS", "session.create", "Invalid title")
        session_id = f"ses_{uuid.uuid4().hex}"
        timestamp = now()
        self.db.execute("INSERT INTO sessions(id,project_id,agent,title,model,permission_mode,status,created_at,updated_at) VALUES(?,?,?,?,?,?,'idle',?,?)", (session_id, project["id"], agent, title, model, mode, timestamp, timestamp))
        self.db.commit()
        return self._session(session_id)

    async def _delete_session(self, session: dict[str, Any]) -> dict[str, Any]:
        if self.db.one("SELECT id FROM runs WHERE session_id=? AND finished_at IS NULL", (session["id"],)):
            raise fail("E_SESSION_BUSY", "session.delete", "Session has an active run", "Stop the run before deleting")
        adapter = self.adapters.get(session["agent"])
        delete = getattr(adapter, "delete_session", None)
        if session.get("nativeId"):
            if delete is None:
                raise fail("E_CAPABILITY", "session.delete", "Native session deletion is unsupported", "Delete the native record through the official CLI")
            await delete(session)
        session_id = session["id"]
        self.db.execute("DELETE FROM event_blobs WHERE session_id=?", (session_id,))
        self.db.execute("DELETE FROM events WHERE session_id=?", (session_id,))
        self.db.execute("DELETE FROM approvals WHERE run_id IN (SELECT id FROM runs WHERE session_id=?)", (session_id,))
        self.db.execute("DELETE FROM client_msgs WHERE session_id=?", (session_id,))
        self.db.execute("DELETE FROM runs WHERE session_id=?", (session_id,))
        self.db.execute("DELETE FROM sessions WHERE id=?", (session_id,))
        self.db.commit()
        return {"deleted": True}

    async def _send(self, client: Client, params: dict[str, Any]) -> dict[str, Any]:
        session_id = required(params, "sessionId")
        client_msg_id = required(params, "clientMsgId")
        text = required(params, "text")
        session = self._session(session_id)
        existing = self.db.one("SELECT * FROM client_msgs WHERE client_msg_id=?", (client_msg_id,))
        if existing:
            if existing["session_id"] != session_id:
                raise fail("E_CONFLICT", "run.send", "Message belongs to another session", "Generate a new clientMsgId")
            return {"runId": existing["run_id"], "clientMsgId": client_msg_id, "accepted": True}
        if self.draining_until > asyncio.get_running_loop().time():
            raise fail("E_HOST_DRAINING", "run.send", "Host is preparing maintenance", "Retry after maintenance completes")
        if self.db.one("SELECT id FROM runs WHERE session_id=? AND finished_at IS NULL", (session_id,)):
            raise fail("E_SESSION_BUSY", "run.send", "Session has an active run", "Append input or stop the current run")
        adapter = self.adapters.get(session["agent"])
        if not adapter:
            raise fail("E_AGENT_NOT_INSTALLED", "run.send", "Agent is not installed")
        capabilities = await adapter.capabilities()
        if not capabilities.get("loggedIn", False):
            raise fail("E_NOT_LOGGED_IN", "run.send", "Agent is not logged in", "Complete official CLI login")
        if session.get("nativeId") and not await adapter.native_exists(session):
            raise fail("E_NATIVE_MISSING", "run.resume", "Native session record is missing", "Start a new session; UI history cannot restore native context")
        if "script" in params and session["agent"] != "fake":
            raise fail("E_INVALID_PARAMS", "run.send", "Scripts are supported only by FakeAdapter")
        run_id = f"run_{uuid.uuid4().hex}"
        message_seq = self.db.next_seq()
        started_seq = self.db.next_seq()
        try:
            self.db.execute("INSERT INTO runs(id,session_id,state,started_at,origin_device_id) VALUES(?,?,'running',?,?)", (run_id, session_id, now(), client.device_id))
            self.db.execute("INSERT INTO client_msgs VALUES(?,?,?,?)", (client_msg_id, session_id, run_id, now()))
            self.db.execute("UPDATE sessions SET status='running',updated_at=? WHERE id=?", (now(), session_id))
            message_event = self._record(session_id, run_id, "user.message", {"text": text, "clientMsgId": client_msg_id}, seq=message_seq, commit=False)
            started_event = self._record(session_id, run_id, "run.started", {"state": "running"}, seq=started_seq, commit=False)
            self.db.commit()
        except Exception:
            self.db.connection.rollback()
            raise
        await self._broadcast(message_event)
        await self._broadcast(started_event)

        async def emit(event_type: str, payload: dict[str, Any], item_id: str | None = None) -> None:
            await self.emit(session_id, run_id, event_type, payload, item_id)

        async def approve(kind: str, payload: dict[str, Any]) -> dict[str, Any]:
            return await self._approve(session_id, run_id, kind, payload)

        async def save_native(native_id: str) -> None:
            self.db.execute("UPDATE sessions SET native_id=?,updated_at=? WHERE id=?", (native_id, now(), session_id))
            self.db.commit()

        try:
            handle = await adapter.start_run(session, {key: params[key] for key in ("text", "script") if key in params}, RunContext(run_id, emit, approve, save_native))
            self.handles[run_id] = (adapter, handle)
            watcher = asyncio.create_task(self._watch(session_id, run_id, handle.task))
            self.watchers.add(watcher)
            watcher.add_done_callback(self.watchers.discard)
        except Exception as error:
            await self._finish(session_id, run_id, "failed", error)
            raise
        return {"runId": run_id, "clientMsgId": client_msg_id, "accepted": True}

    async def _watch(self, session_id: str, run_id: str, task: asyncio.Task) -> None:
        try:
            await task
        except asyncio.CancelledError:
            await self._finish(session_id, run_id, "interrupted")
        except Exception as error:
            await self._finish(session_id, run_id, "failed", error)
        else:
            await self._finish(session_id, run_id, "completed")

    async def _finish(self, session_id: str, run_id: str, state: str, error: Exception | None = None) -> None:
        self.handles.pop(run_id, None)
        self.db.execute("UPDATE runs SET state=?,finished_at=? WHERE id=?", (state, now(), run_id))
        self.db.execute("UPDATE sessions SET status='idle',updated_at=? WHERE id=?", (now(), session_id))
        pending = self.db.all("SELECT id FROM approvals WHERE run_id=? AND state='pending'", (run_id,))
        for approval in pending:
            approval_id = approval["id"]
            decision = {"behavior": "deny"}
            self.db.execute("UPDATE approvals SET state='resolved',decision_json=?,decided_at=? WHERE id=?", (json.dumps(decision), now(), approval_id))
            waiter = self.approval_waiters.pop(approval_id, None)
            if waiter and not waiter.done():
                waiter.cancel()
            await self.emit(session_id, run_id, "approval.resolved", {"approvalId": approval_id, "decision": decision, "reason": "run_finished"})
        for key in tuple(self.snapshots):
            if key[0] == session_id:
                snapshot = self.snapshots.pop(key)
                await self.emit(session_id, run_id, "item.completed", {**snapshot["payload"], "interrupted": state != "completed"}, key[1])
        if error:
            await self.emit(session_id, run_id, "item.completed", {"kind": "error", "code": getattr(error, "code", "E_AGENT"), "stage": getattr(error, "stage", "adapter.run"), "text": str(error) if isinstance(error, AdapterError) else "Agent execution failed", "hint": getattr(error, "hint", "Check the Agent environment")}, f"itm_{uuid.uuid4().hex}")
        await self.emit(session_id, run_id, "run.finished", {"state": state})

    async def _approve(self, session_id: str, run_id: str, kind: str, payload: dict[str, Any]) -> dict[str, Any]:
        approval_id = f"apr_{uuid.uuid4().hex}"
        future = asyncio.get_running_loop().create_future()
        self.approval_waiters[approval_id] = future
        self.db.execute("INSERT INTO approvals(id,run_id,kind,payload_json,state) VALUES(?,?,?,?,'pending')", (approval_id, run_id, kind, json.dumps(payload)))
        self.db.execute("UPDATE runs SET state='awaiting_approval' WHERE id=?", (run_id,))
        self.db.execute("UPDATE sessions SET status='awaiting_approval' WHERE id=?", (session_id,))
        await self.emit(session_id, run_id, "run.state", {"state": "awaiting_approval"})
        await self.emit(session_id, run_id, "approval.request", {**payload, "approvalId": approval_id, "kind": kind})
        for client in tuple(self.clients):
            if session_id in client.subscriptions:
                await self._server_approval(client, session_id, approval_id, kind, payload)
        try:
            return await future
        finally:
            self.approval_waiters.pop(approval_id, None)

    async def _server_approval(self, client: Client, session_id: str, approval_id: str, kind: str, payload: dict[str, Any]) -> None:
        request_id = "approval:" + approval_id
        client.server_requests[request_id] = (session_id, approval_id)
        row = self.db.one("SELECT * FROM events WHERE session_id=? AND type='approval.request' AND json_extract(payload_json,'$.approvalId')=? ORDER BY seq DESC LIMIT 1", (session_id, approval_id))
        if not row:
            return
        params = self.db.event(row)
        try:
            await client.send({"jsonrpc": "2.0", "id": request_id, "method": "approval.request", "params": params})
        except ConnectionError:
            self.clients.discard(client)

    async def handle_response(self, client: Client, response: dict[str, Any]) -> None:
        request_id = response.get("id")
        if not isinstance(request_id, str):
            return
        owner = client.server_requests.pop(request_id, None)
        if not owner or "error" in response:
            return
        result = response.get("result")
        if not isinstance(result, dict):
            return
        decision = result.get("decision", result)
        try:
            await self._respond(client, {"sessionId": owner[0], "approvalId": owner[1], "decision": decision})
        except AdapterError:
            # JSON-RPC responses have no response; the canonical resolved event wins.
            return

    async def _respond(self, client: Client, params: dict[str, Any]) -> dict[str, Any]:
        session_id = required(params, "sessionId")
        approval_id = required(params, "approvalId")
        self._session(session_id)
        decision = params.get("decision")
        if not isinstance(decision, dict) or decision.get("behavior") not in {"allow", "deny"}:
            raise fail("E_INVALID_PARAMS", "approval.respond", "Invalid approval decision")
        if set(decision) - {"behavior", "answers"}:
            raise fail("E_INVALID_PARAMS", "approval.respond", "Unexpected approval fields")
        if "answers" in decision:
            answers = decision["answers"]
            if not isinstance(answers, dict) or any(not isinstance(key, str) or not (isinstance(value, str) or isinstance(value, list) and all(isinstance(item, str) for item in value)) for key, value in answers.items()):
                raise fail("E_INVALID_PARAMS", "approval.respond", "Invalid question answers")
        row = self.db.one("SELECT a.* FROM approvals a JOIN runs r ON r.id=a.run_id WHERE a.id=? AND r.session_id=?", (approval_id, session_id))
        if not row:
            raise fail("E_NOT_FOUND", "approval.respond", "Approval does not exist")
        if row["state"] != "pending":
            error = fail("E_ALREADY_RESOLVED", "approval.respond", "Approval already resolved", "Use the decision returned by the Host")
            error.decision = json.loads(row["decision_json"])
            raise error
        for subscriber in tuple(self.clients):
            subscriber.server_requests.pop("approval:" + approval_id, None)
        self.db.execute("UPDATE approvals SET state='resolved',decision_json=?,decided_by_device=?,decided_at=? WHERE id=? AND state='pending'", (json.dumps(decision), client.device_id, now(), approval_id))
        self.db.execute("UPDATE runs SET state='running' WHERE id=?", (row["run_id"],))
        self.db.execute("UPDATE sessions SET status='running' WHERE id=?", (session_id,))
        await self.emit(session_id, row["run_id"], "approval.resolved", {"approvalId": approval_id, "decision": decision, "deviceId": client.device_id})
        await self.emit(session_id, row["run_id"], "run.state", {"state": "running"})
        waiter = self.approval_waiters.get(approval_id)
        if waiter and not waiter.done():
            waiter.set_result(decision)
        return {"decision": decision}

    def _sync(self, params: dict[str, Any]) -> dict[str, Any]:
        last_seq = integer(params, "lastSeq", 0, 2**63 - 1)
        limit = max(1, integer(params, "limit", 100, 200))
        session_id = params.get("sessionId")
        if session_id is not None:
            self._session(required(params, "sessionId"))
        query = "SELECT * FROM events WHERE seq>?"
        args: tuple = (last_seq,)
        if session_id:
            query += " AND session_id=?"
            args += (session_id,)
        rows = self.db.all(query + " ORDER BY seq LIMIT ?", args + (limit + 1,))
        events = self._bounded_events(rows[:limit])
        has_more = len(rows) > len(events)
        watermark = events[-1]["seq"] if has_more and events else self.db.current_seq
        snapshots = []
        size = len(encoded(events))
        if not has_more:
            for (sid, _), event in self.snapshots.items():
                if session_id and sid != session_id:
                    continue
                snapshot = {**event, "payload": dict(event["payload"])}
                text = str(snapshot["payload"].get("text", ""))
                if len(encoded(snapshot)) > MAX_EVENT:
                    snapshot["payload"].update(text=text[:16000], truncated=True, originalBytes=len(text.encode("utf-8")))
                length = len(encoded(snapshot))
                if size + length > 3 * 1024 * 1024:
                    raise fail("E_CAPABILITY", "sync.since", "Too many active item snapshots", "Synchronize one session at a time")
                snapshots.append(snapshot)
                size += length
            run_query = "SELECT id,session_id,state FROM runs WHERE finished_at IS NULL"
            run_args: tuple = ()
            if session_id:
                run_query += " AND session_id=?"
                run_args = (session_id,)
            for run in self.db.all(run_query, run_args):
                snapshot = {"seq": watermark, "sessionId": run["session_id"], "runId": run["id"], "ts": now(), "type": "run.state", "payload": {"state": run["state"], "snapshot": True}}
                size += len(encoded(snapshot))
                if size > 3 * 1024 * 1024:
                    raise fail("E_CAPABILITY", "sync.since", "Too many active run snapshots", "Synchronize one session at a time")
                snapshots.append(snapshot)
        return {"events": events, "snapshots": snapshots, "currentSeq": watermark, "hasMore": has_more}

    def _blob(self, params: dict[str, Any]) -> dict[str, Any]:
        session_id = required(params, "sessionId")
        self._session(session_id)
        seq = integer(params, "seq", 0, 2**63 - 1)
        offset = integer(params, "offset", 0, 2**63 - 1)
        limit = max(1, integer(params, "limit", 128 * 1024, 512 * 1024))
        row = self.db.one("SELECT content FROM event_blobs WHERE seq=? AND session_id=?", (seq, session_id))
        if not row:
            raise fail("E_NOT_FOUND", "event.blob", "Event blob does not exist")
        content = row["content"]
        if offset > len(content) or offset < len(content) and content[offset] & 0xC0 == 0x80:
            raise fail("E_INVALID_PARAMS", "event.blob", "Offset must be a UTF-8 byte boundary")
        end = min(len(content), offset + limit)
        while end > offset and end < len(content) and content[end] & 0xC0 == 0x80:
            end -= 1
        if end == offset and end < len(content):
            end = min(len(content), offset + 4)
            while end < len(content) and content[end] & 0xC0 == 0x80:
                end += 1
        return {"text": content[offset:end].decode("utf-8"), "offset": offset, "nextOffset": end, "total": len(content), "hasMore": end < len(content)}

    async def close(self) -> None:
        for _, handle in tuple(self.handles.values()):
            handle.task.cancel()
        if self.watchers:
            await asyncio.gather(*tuple(self.watchers), return_exceptions=True)
        self.db.close()
