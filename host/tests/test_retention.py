from agentdesk_host.store.database import Database, now


def test_retention_keeps_active_runs_and_indexes_but_prunes_old_mirrors(tmp_path):
    database=Database(tmp_path/'host.sqlite')
    database.execute("INSERT INTO projects(id,path,display_name,created_at) VALUES('p','/project','Project',?)",(now(),))
    for sid in ('idle','active'):
        database.execute("INSERT INTO sessions(id,project_id,agent,title,model,permission_mode,status,created_at,updated_at) VALUES(?,'p','fake','Session','fake-default','default','idle',?,?)",(sid,now(),now()))
    database.execute("INSERT INTO runs(id,session_id,state,started_at,origin_device_id) VALUES('r','active','running',?,'device')",(now(),))
    for seq,sid,ts in [(1,'idle','2000-01-01T00:00:00Z'),(2,'idle',now()),(3,'active','2000-01-01T00:00:00Z')]:
        database.execute("INSERT INTO events VALUES(?,?,NULL,?,'item.completed',NULL,'{}')",(seq,sid,ts))
        database.execute("INSERT INTO event_blobs VALUES(?,?,?)",(seq,sid,b'{}'))
    database.commit()
    database.prune_events(0)
    assert len(database.all('SELECT * FROM events'))==3
    database.prune_events(180)
    assert [r['seq'] for r in database.all('SELECT seq FROM events ORDER BY seq')]==[2,3]
    assert [r['seq'] for r in database.all('SELECT seq FROM event_blobs ORDER BY seq')]==[2,3]
    assert len(database.all('SELECT * FROM sessions'))==2
    database.close()
