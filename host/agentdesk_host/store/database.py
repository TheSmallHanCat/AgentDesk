"""SQLite mirror and monotonic event cursor, owned by the asyncio Host thread."""
from __future__ import annotations

import json
import sqlite3
import uuid
from datetime import datetime, timezone, timedelta
from pathlib import Path
from typing import Any


def now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


SCHEMA_VERSION = 3
SCHEMA_SQL = """
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE projects(id TEXT PRIMARY KEY, kind TEXT NOT NULL DEFAULT 'remote', device_id TEXT, path TEXT NOT NULL, display_name TEXT NOT NULL, native_root TEXT, default_agent TEXT, created_at TEXT NOT NULL, archived_at TEXT, runtime_mode TEXT NOT NULL DEFAULT 'direct', runtime_id TEXT);
CREATE TABLE sessions(id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id), agent TEXT NOT NULL, native_id TEXT, native_root TEXT, title TEXT NOT NULL, model TEXT NOT NULL, permission_mode TEXT NOT NULL, effort TEXT, status TEXT NOT NULL, device_id TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT, archived_at TEXT, runtime_id TEXT);
CREATE TABLE runs(id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id), state TEXT NOT NULL, started_at TEXT NOT NULL, finished_at TEXT, stop_reason TEXT, origin_device_id TEXT NOT NULL);
CREATE TABLE events(seq INTEGER PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id), run_id TEXT, ts TEXT NOT NULL, type TEXT NOT NULL, item_id TEXT, payload_json TEXT NOT NULL);
CREATE INDEX events_session_seq ON events(session_id,seq);
CREATE TABLE event_blobs(seq INTEGER PRIMARY KEY, session_id TEXT NOT NULL, content BLOB NOT NULL);
CREATE TABLE approvals(id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), kind TEXT NOT NULL, payload_json TEXT NOT NULL, state TEXT NOT NULL, decision_json TEXT, decided_by_device TEXT, decided_at TEXT);
CREATE TABLE devices(id TEXT PRIMARY KEY, name TEXT NOT NULL, os TEXT NOT NULL, last_seen_at TEXT NOT NULL, online INTEGER NOT NULL DEFAULT 0);
CREATE TABLE client_msgs(client_msg_id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id), run_id TEXT NOT NULL REFERENCES runs(id), accepted_at TEXT NOT NULL);
"""


class SchemaVersionError(RuntimeError):
    pass


def database_version(connection: sqlite3.Connection) -> int:
    try:
        version = connection.execute("PRAGMA user_version").fetchone()[0]
        if version not in (0, SCHEMA_VERSION):
            raise SchemaVersionError("Host database format is incompatible with this version")
        if version == 0 and connection.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' LIMIT 1").fetchone():
            raise SchemaVersionError("Unversioned database is not empty")
        if version == SCHEMA_VERSION:
            reference = sqlite3.connect(':memory:')
            try:
                reference.executescript(SCHEMA_SQL)
                for (table,) in reference.execute("SELECT name FROM sqlite_master WHERE type='table'"):
                    query = f'PRAGMA table_info("{table}")'
                    expected = reference.execute(query).fetchall()
                    actual = [tuple(row) for row in connection.execute(query)]
                    if actual != expected:
                        raise SchemaVersionError("Host database structure is incompatible with this version")
            finally:
                reference.close()
        return version
    except sqlite3.Error:
        raise SchemaVersionError("Host database could not be validated") from None


def check_database(path: Path) -> int:
    """Validate an existing database without creating or migrating it."""
    path = Path(path).resolve()
    if not path.exists():
        return 0
    connection = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)
    try:
        return database_version(connection)
    finally:
        connection.close()


class Database:
    def __init__(self, path: Path):
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.connection = sqlite3.connect(path)
        try:
            self.connection.row_factory = sqlite3.Row
            version = database_version(self.connection)
            self.connection.execute("PRAGMA foreign_keys=ON")
            if version == 0:
                self.connection.executescript("BEGIN IMMEDIATE;\n" + SCHEMA_SQL + f"\nPRAGMA user_version={SCHEMA_VERSION};\nCOMMIT;")
            self.connection.execute("PRAGMA journal_mode=WAL")
            self.connection.execute("INSERT OR IGNORE INTO meta VALUES('host_id', ?)", (f"host_{uuid.uuid4().hex}",))
            self.connection.execute("INSERT OR IGNORE INTO meta VALUES('seq', '0')")
            self.connection.commit()
            # Reserve ranges before exposing events; restart skips unused values.
            self._seq = max(int(self.connection.execute("SELECT value FROM meta WHERE key='seq'").fetchone()[0]), self.connection.execute("SELECT COALESCE(MAX(seq),0) FROM events").fetchone()[0])
            self._reserved_seq = self._seq
            self._allocator = sqlite3.connect(path)
        except BaseException:
            self.connection.close()
            raise

    def reserve_seq(self, count: int = 1) -> None:
        if self._seq + count <= self._reserved_seq:
            return
        if self.connection.in_transaction:
            raise RuntimeError("Reserve event sequences before starting a business transaction")
        reserved = self._seq + max(count, 1024)
        # This separate transaction cannot be rolled back by a failed run.accept.
        self._allocator.execute("UPDATE meta SET value=? WHERE key='seq'", (str(reserved),))
        self._allocator.commit()
        self._reserved_seq = reserved

    @property
    def host_id(self) -> str:
        return self.connection.execute("SELECT value FROM meta WHERE key='host_id'").fetchone()[0]

    @property
    def current_seq(self) -> int:
        return self._seq

    def next_seq(self) -> int:
        self.reserve_seq()
        self._seq += 1
        return self._seq

    def execute(self, query: str, args: tuple = ()) -> sqlite3.Cursor:
        return self.connection.execute(query, args)

    def one(self, query: str, args: tuple = ()) -> dict[str, Any] | None:
        row = self.execute(query, args).fetchone()
        return dict(row) if row else None

    def all(self, query: str, args: tuple = ()) -> list[dict[str, Any]]:
        return [dict(row) for row in self.execute(query, args).fetchall()]

    def commit(self) -> None:
        self.connection.commit()

    def close(self) -> None:
        self.connection.close()
        self._allocator.close()

    def prune_events(self, retention_days: int = 180) -> None:
        if retention_days == 0:
            return
        cutoff = (datetime.now(timezone.utc) - timedelta(days=retention_days)).isoformat().replace("+00:00", "Z")
        predicate = "ts<? AND session_id NOT IN (SELECT session_id FROM runs WHERE finished_at IS NULL)"
        self.execute("DELETE FROM event_blobs WHERE seq IN (SELECT seq FROM events WHERE " + predicate + ")", (cutoff,))
        self.execute("DELETE FROM events WHERE " + predicate, (cutoff,))
        self.commit()

    @staticmethod
    def event(row: dict[str, Any]) -> dict[str, Any]:
        value = {"seq": row["seq"], "sessionId": row["session_id"], "ts": row["ts"], "type": row["type"], "payload": json.loads(row["payload_json"])}
        if row["run_id"]:
            value["runId"] = row["run_id"]
        if row["item_id"]:
            value["itemId"] = row["item_id"]
        return value
