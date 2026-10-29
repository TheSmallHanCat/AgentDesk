"""The first release initializes one format and never migrates incompatible data."""
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import sqlite3
import subprocess
import sys

import pytest

from agentdesk_host.store.database import Database, SCHEMA_VERSION, SchemaVersionError, check_database


def test_empty_database_is_created_in_current_format_and_survives_reopen(tmp_path):
    path = tmp_path / 'host.sqlite'
    database = Database(path)
    identity = database.host_id
    database.execute("INSERT INTO projects(id,path,display_name,created_at) VALUES('p','/project','Project','time')")
    database.execute("INSERT INTO sessions(id,project_id,agent,native_id,title,model,permission_mode,status,created_at,updated_at) VALUES('s','p','claude','native-id','Title','model','default','idle','time','time')")
    database.execute("INSERT INTO events(seq,session_id,ts,type,payload_json) VALUES(17,'s','time','item.completed','{}')")
    database.commit()
    database.close()
    assert check_database(path) == SCHEMA_VERSION
    reopened = Database(path)
    try:
        assert reopened.host_id == identity and reopened.current_seq == 17
        assert reopened.one("SELECT runtime_mode,runtime_id FROM projects WHERE id='p'") == {'runtime_mode': 'direct', 'runtime_id': None}
        assert reopened.one("SELECT native_id,runtime_id FROM sessions WHERE id='s'") == {'native_id': 'native-id', 'runtime_id': None}
        assert reopened.one('SELECT COUNT(*) AS total FROM events')['total'] == 1
    finally:
        reopened.close()


@pytest.mark.parametrize('version', [0, 1, 2, SCHEMA_VERSION + 1])
def test_incompatible_database_is_unchanged_and_handles_are_closed(tmp_path, version):
    path = tmp_path / 'host.sqlite'
    connection = sqlite3.connect(path)
    connection.executescript(f'CREATE TABLE retained(value TEXT); INSERT INTO retained VALUES("kept"); PRAGMA user_version={version};')
    connection.close()
    before = hashlib.sha256(path.read_bytes()).hexdigest()
    for action in (check_database, Database):
        with pytest.raises(SchemaVersionError):
            action(path)
        assert hashlib.sha256(path.read_bytes()).hexdigest() == before
    assert not path.with_name(path.name + '-wal').exists()
    path.rename(tmp_path / 'retained.sqlite')


def test_missing_database_preflight_does_not_create_directories(tmp_path):
    path = tmp_path / 'missing/state/host.sqlite'
    assert check_database(path) == 0
    assert not path.parent.exists()


@pytest.mark.parametrize('state', ['missing', 'current', 'incompatible', 'corrupt', 'empty_versioned', 'missing_table', 'missing_column'])
def test_database_check_cli_is_read_only_and_never_starts_host(tmp_path, state):
    home = tmp_path / 'host'
    path = home / 'state/host.sqlite'
    if state != 'missing':
        path.parent.mkdir(parents=True)
    if state in ('current', 'missing_table', 'missing_column'):
        database = Database(path)
        if state == 'missing_table':
            database.execute('DROP TABLE meta')
        if state == 'missing_column':
            database.execute('ALTER TABLE sessions DROP COLUMN archived_at')
        database.commit()
        database.close()
    elif state in ('incompatible', 'empty_versioned'):
        connection = sqlite3.connect(path)
        connection.execute(f'PRAGMA user_version={2 if state == "incompatible" else SCHEMA_VERSION}')
        connection.close()
    elif state == 'corrupt':
        path.write_bytes(b'invalid database')
    before = path.read_bytes() if path.exists() else None
    result = subprocess.run(
        [sys.executable, '-m', 'agentdesk_host', '--home', str(home), '--check-database'],
        capture_output=True, text=True, timeout=10,
        env={**os.environ, 'PYTHONPATH': str(Path(__file__).resolve().parents[1])},
    )
    assert result.returncode == (0 if state in ('missing', 'current') else 78)
    if result.returncode == 0:
        assert json.loads(result.stdout) == {'compatible': True, 'schemaVersion': SCHEMA_VERSION if state == 'current' else 0, 'supportedVersion': SCHEMA_VERSION}
    else:
        assert 'E_DATABASE_SCHEMA' in result.stderr
        with pytest.raises(SchemaVersionError):
            Database(path)
    assert (path.read_bytes() if path.exists() else None) == before
    assert not (home / 'run').exists()


@pytest.mark.parametrize('compatible', [False, True])
def test_installer_database_guard_rejects_before_following_actions(tmp_path, compatible):
    shell = shutil.which('bash') if os.name == 'nt' else '/bin/sh'
    if os.name == 'nt' and not shell:
        git = shutil.which('git')
        candidate = Path(git).resolve().parent.parent / 'bin/bash.exe' if git else None
        shell = str(candidate) if candidate and candidate.is_file() else None
    if not shell:
        pytest.skip('Git Bash is required on Windows')
    home = tmp_path / '.agentdesk'
    path = home / 'state/host.sqlite'
    database = Database(path)
    if not compatible:
        database.execute('PRAGMA user_version=2')
    database.close()
    before = path.read_bytes()
    root = Path(__file__).resolve().parents[2]
    guard = (root / 'src-tauri/src/host/check_database.sh').read_text(encoding='utf-8')
    script = 'set -eu\nPATH=/usr/bin:$PATH\n' + guard + '\ncheck_host_database ' + shlex.quote(Path(sys.executable).as_posix()) + '\nprintf "ACTIVATE\\n"\n'
    result = subprocess.run([shell, '-s'], input=script, capture_output=True, text=True, encoding='utf-8', timeout=25,
                            env={**os.environ, 'HOME': str(tmp_path), 'PYTHONPATH': str(root / 'host')})
    assert result.returncode == (0 if compatible else 78), result.stderr
    assert ('ACTIVATE' in result.stdout) is compatible
    assert path.read_bytes() == before and not (home / 'run').exists()
