"""Linux-only boundary smoke test using a preinstalled image and owned temporary data."""
import argparse
import asyncio
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'host'))
from agentdesk_host.runtime.client import RuntimeClient
from agentdesk_host.runtime.engine import PodmanEngine


async def check(image):
    root = Path(tempfile.mkdtemp(prefix='agentdesk-isolation-')).resolve()
    home, project = root / 'host', root / 'project'
    home.mkdir(mode=0o700); project.mkdir(mode=0o700)
    marker = home / 'control-marker'
    marker.write_text('owned-boundary-probe', encoding='utf-8')
    engine = PodmanEngine(home, 'isolation-check-' + uuid.uuid4().hex, {'image': image})
    client = RuntimeClient(engine, 'rt_' + uuid.uuid4().hex, str(project))
    try:
        await client.ensure()
        code = 'import json,os; print(json.dumps({"hostMarkerHidden":not os.path.exists(' + repr(str(marker)) + '),"projectWritable":os.access("/workspace",os.W_OK)}))'
        result = json.loads(await engine.invoke('exec', client.name, '/usr/local/bin/python', '-c', code))
        assert result == {'hostMarkerHidden': True, 'projectWritable': True}
    finally:
        try:
            try:
                await client.close()
            finally:
                await engine.close()
        finally:
            # Keep mounted data if container removal could not be confirmed.
            if not engine.owned:
                shutil.rmtree(root)
    assert not engine.owned
    print(json.dumps({'boundaryProbe': True, **result, 'containerRemoved': True}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', required=True)
    args = parser.parse_args()
    if os.name != 'posix' or not Path('/proc/self/ns/pid').exists():
        parser.error('This check requires a Linux server with rootless Podman')
    asyncio.run(check(args.image))


if __name__ == '__main__':
    main()
