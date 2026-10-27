"""Run M3 acceptance in a dedicated directory without changing the installed Host."""
from __future__ import annotations

import json
import os
import re
import shlex
import subprocess
import tarfile
import uuid

from e2e_common import ROOT, PRIVATE, REMOTE, configured, connect, run, save_result, upload

DAEMON = '''import asyncio,os,pathlib,signal,sys
from agentdesk_host.adapters.fake import FakeAdapter
from agentdesk_host.server.service import HostService
from agentdesk_host.server.transport import HostServer
async def main():
    home=pathlib.Path(sys.argv[1])
    home.mkdir(parents=True,exist_ok=True,mode=0o700)
    service=HostService(home,{'fake':FakeAdapter()})
    server=HostServer(service)
    stop=asyncio.Event()
    for sig in (signal.SIGTERM,signal.SIGINT):
        asyncio.get_running_loop().add_signal_handler(sig,stop.set)
    await server.start()
    (home/'run'/'host.pid').write_text(str(os.getpid()))
    try: await stop.wait()
    finally:
        await server.close()
        await service.close()
        (home/'run'/'host.pid').unlink(missing_ok=True)
os.umask(0o077)
asyncio.run(main())
'''


def main():
    if not configured():
        return 1
    identifier = uuid.uuid4().hex[:6]
    work = REMOTE + '/m7-' + identifier
    home = work + '/.agentdesk'
    archive = PRIVATE / ('m7-' + identifier + '.tar.gz')
    result = {'runId': identifier, 'nativeModelCalls': False, 'passed': []}
    with tarfile.open(archive, 'w:gz') as package:
        for path in sorted((ROOT / 'host' / 'agentdesk_host').rglob('*.py')):
            package.add(path, arcname=path.relative_to(ROOT).as_posix())
    daemon = PRIVATE / ('m7-' + identifier + '.py')
    daemon.write_text(DAEMON, encoding='utf-8')
    try:
        with connect() as client:
            run(client, 'umask 077\nmkdir ' + shlex.quote(work) + '\n')
            upload(client, archive, work + '/source.tar.gz')
            upload(client, daemon, work + '/fake_host.py')
            script = f'''set -eu
umask 077
cd {shlex.quote(work)}
tar -xzf source.tar.gz
mkdir -p {shlex.quote(home)} {shlex.quote(REMOTE + '/t')}
export PYTHONPATH={shlex.quote(work + '/host')}
export TMPDIR={shlex.quote(REMOTE + '/t')}
export PATH="$HOME/.agentdesk/acceptance/ssh-client/bin:$PATH"
nohup "$HOME/.agentdesk/host/current/venv/bin/python" fake_host.py {shlex.quote(home)} >daemon.log 2>&1 </dev/null &
for n in $(seq 1 100); do test ! -S {shlex.quote(home + '/run/host.sock')} || exit 0; sleep .1; done
exit 1
'''
            run(client, script)
        environment = os.environ.copy()
        environment.update(AGENTDESK_E2E_DEVICE_HOME=home, AGENTDESK_E2E_REUSE_HOST='1', AGENTDESK_E2E_SSH_CLIENT='1', PYTHONUTF8='1')
        environment.pop('RUST_LOG', None)
        local_temp = ROOT / '.tmp' / 't'
        local_temp.mkdir(exist_ok=True)
        environment.update(TEMP=str(local_temp), TMP=str(local_temp))
        process = subprocess.Popen(['cargo','test','--locked','--manifest-path','src-tauri/Cargo.toml','--test','real_device','--','--ignored','--nocapture','--test-threads=1'], cwd=ROOT, env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding='utf-8', errors='replace')
        assert process.stdout is not None
        for line in process.stdout:
            match = re.search(r'PASS: (m3_[a-z0-9_]+)\s*$', line)
            if match:
                result['passed'].append(match.group(1))
                print('PASS: ' + match.group(1), flush=True)
            elif 'OBSERVE: m3_raw_codex_read_only_socket_code=' in line:
                observation = re.search(r'socket_code=(-?[0-9]+) local_write=(true|false)', line)
                if observation:
                    result['readOnlyProbe'] = {'exitCode':int(observation.group(1)), 'localWrite':observation.group(2)=='true'}
            elif 'test result:' in line:
                summary = re.search(r'test result: (?:ok|FAILED)\. \d+ passed; \d+ failed; \d+ ignored', line)
                if summary: result['summary'] = summary.group()
            elif 'error[' in line:
                codes = re.findall(r'error\[([A-Z0-9]+)\]', line)
                result.setdefault('compilerCodes', []).extend(codes)
            elif 'panicked at' in line:
                result['panic'] = 'acceptance_assertion_failed'
                location = re.search(r'([a-z_]+[.]rs:[0-9]+:[0-9]+)', line)
                if location: result['panicLocation'] = location.group(1)
            elif re.search(r'E_[A-Z_]+', line):
                result.setdefault('failureCodes', []).extend(re.findall(r'E_[A-Z_]+', line))
                result.setdefault('failureStages', []).extend(re.findall(r'E_[A-Z_]+ at ([a-z.]+)', line))
            elif re.match(r'\s*(?:left|right): (?:true|false|\d+)\s*$', line):
                result.setdefault('assertionValues', []).append(line.strip())
        result['exitCode'] = process.wait()
        print(json.dumps(result), flush=True)
        return 0 if result['exitCode'] == 0 else 1
    except Exception:
        result['failure'] = 'isolated_m3_acceptance_failed_details_suppressed'
        print('FAIL: isolated M3 acceptance failed; details suppressed', flush=True)
        return 1
    finally:
        try:
            with connect() as client:
                script = f'''set -eu
pidfile={shlex.quote(home + '/run/host.pid')}
if test -f "$pidfile"; then
  pid=$(cat "$pidfile")
  case "$pid" in ''|*[!0-9]*) exit 1;; esac
  if test -r "/proc/$pid/cmdline" && grep -aF -- {shlex.quote(home)} "/proc/$pid/cmdline" >/dev/null; then
    kill -TERM "$pid"
    for n in $(seq 1 100); do test -e "$pidfile" || exit 0; sleep .1; done
    exit 1
  else
    exit 1
  fi
fi
'''
                run(client, script, timeout=20)
                result['isolatedHostStopped'] = True
        except Exception:
            result['isolatedHostStopped'] = False
        save_result('task07-m3-review2', result)


if __name__ == '__main__':
    raise SystemExit(main())
