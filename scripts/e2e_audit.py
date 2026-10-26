"""Read-only inventory of test-owned processes, sockets and retained files."""

from __future__ import annotations

import json

from e2e_common import configured, connect, run, save_result

AUDIT = r"""python - <<'PY'
from __future__ import print_function
import json, os, stat
home = '/home/agentdesk-test'
base = home + '/agentdesk-e2e'
uid = os.getuid()
processes = []
for entry in os.listdir('/proc'):
    if not entry.isdigit():
        continue
    path = '/proc/' + entry
    try:
        if os.stat(path).st_uid != uid:
            continue
        args = open(path + '/cmdline', 'rb').read().split(chr(0).encode('ascii'))
        category = None
        if any(b'agentdesk-archive-' in arg for arg in args):
            category = 'archive'
        elif any(arg.endswith(b'/echo.py') and arg.startswith(base.encode()) for arg in args):
            category = 's1-echo'
        elif any(arg.endswith(b'/socket_service.py') and arg.startswith(base.encode()) for arg in args):
            category = 's6-socket'
        elif len(args) > 1 and args[1] == b'app-server' and args[0].startswith((home + '/.agentdesk/').encode()):
            category = 'codex-app-server'
        if category:
            processes.append(category)
    except (IOError, OSError):
        pass
sockets = []
for directory, names, files in os.walk(base, followlinks=False):
    for name in files:
        path = directory + '/' + name
        try:
            if stat.S_ISSOCK(os.lstat(path).st_mode):
                sockets.append(os.path.relpath(path, base))
        except OSError:
            pass
archives = 0
for name in os.listdir('/tmp'):
    if name.startswith('agentdesk-download.'):
        try:
            if os.lstat('/tmp/' + name).st_uid == uid:
                archives += 1
        except OSError:
            pass
print(json.dumps({
    'testOwnedProcesses': processes,
    'testOwnedSockets': sockets,
    'remoteTemporaryArchives': archives,
    'homeEntries': sorted(os.listdir(home)),
    'homeMode': oct(os.stat(home).st_mode & 0o777),
    'authorizedKeysMode': oct(os.stat(home + '/.ssh/authorized_keys').st_mode & 0o777),
}))
PY
"""


def main() -> int:
    if not configured():
        return 0
    with connect() as client:
        _, output = run(client, AUDIT)
        result = json.loads(output)
        _, linger = run(client, 'loginctl show-user "$(id -u)" --property=Linger 2>/dev/null || true')
        result["lingerEnabled"] = "Linger=yes" in linger
    result["passed"] = not result["testOwnedProcesses"] and not result["testOwnedSockets"] and result["remoteTemporaryArchives"] == 0
    result["firewallChanges"] = "none_used_local_proxy"
    save_result("audit", result)
    print(json.dumps(result, ensure_ascii=True, indent=2))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception:
        print("FAIL: test server audit failed (details suppressed)")
        raise SystemExit(1) from None
