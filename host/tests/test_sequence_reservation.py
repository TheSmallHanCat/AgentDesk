from agentdesk_host.store.database import Database


def test_online_sequence_allocation_avoids_per_delta_writes_and_never_reuses(tmp_path):
    path=tmp_path/'host.sqlite'
    database=Database(path)
    before=database.connection.total_changes
    numbers=[]
    for _ in range(2500):
        numbers.append(database.next_seq())
        database.commit()
    assert numbers==sorted(set(numbers))
    assert database.connection.total_changes-before==0
    assert database._allocator.total_changes==3
    database.close()
    reopened=Database(path)
    assert reopened.next_seq()>numbers[-1]
    reopened.commit()
    reopened.close()


def test_failed_acceptance_rollback_cannot_undo_reserved_range(tmp_path):
    database = Database(tmp_path / 'host.sqlite')
    exposed = database.next_seq()
    database.execute("INSERT INTO meta VALUES('rollback', 'pending')")
    database.connection.rollback()
    assert database.one("SELECT value FROM meta WHERE key='seq'")["value"] == '1024'
    database.close()
    reopened = Database(tmp_path / 'host.sqlite')
    assert reopened.next_seq() > exposed
    reopened.close()


def test_startup_uses_maximum_persisted_event_even_if_metadata_is_old(tmp_path):
    database = Database(tmp_path / 'host.sqlite')
    # This fixture represents a database written by the previous allocator.
    database.execute("INSERT INTO projects(id,path,display_name,created_at) VALUES('p','/project','P','now')")
    database.execute("INSERT INTO sessions(id,project_id,agent,title,model,permission_mode,status,created_at,updated_at) VALUES('s','p','fake','S','m','default','idle','now','now')")
    database.execute("INSERT INTO events VALUES(9000,'s',NULL,'now','user.message',NULL,'{}')")
    database.commit()
    database.close()
    reopened = Database(tmp_path / 'host.sqlite')
    assert reopened.next_seq() > 9000
    reopened.close()
