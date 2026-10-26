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
    assert database.connection.total_changes-before==3
    database.close()
    reopened=Database(path)
    assert reopened.next_seq()>numbers[-1]
    reopened.commit()
    reopened.close()
