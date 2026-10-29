"""Owned rootless Podman containers with a fixed, inspected launch policy."""
from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
import re
import shutil
import uuid

from ..adapters.base import AdapterError
from ..adapters.native_common import command, native_environment

OWNER_LABEL = 'io.agentdesk.host'
RUNTIME_LABEL = 'io.agentdesk.runtime'
IMAGE = re.compile(r'^(?:[a-zA-Z0-9][a-zA-Z0-9._:/-]*@)?sha256:[a-f0-9]{64}$')
RUNTIME_ID = re.compile(r'^rt_[a-f0-9]{32}$')


def failure(code: str, message: str) -> AdapterError:
    return AdapterError(code, 'runtime', message, 'Check the isolated workspace configuration')


def validate_project(path: str, home: Path) -> Path:
    root = Path(path).resolve(strict=True)
    protected = [home.resolve(), Path.home().resolve() / '.ssh', Path('/proc'), Path('/sys'), Path('/dev'), Path('/run'), Path('/var/log')]
    if not root.is_dir() or root == root.parent:
        raise failure('E_RUNTIME_PATH', 'Choose a dedicated project directory')
    for private in protected:
        private = private.resolve()
        if root == private or root in private.parents or private in root.parents:
            raise failure('E_RUNTIME_PATH', 'Project overlaps a protected server directory')
    if any(char in str(root) for char in (',', '\n', '\r', '\0')):
        raise failure('E_RUNTIME_PATH', 'Project path is not supported by the isolated runtime')
    # Git worktrees can refer to metadata outside the mounted directory.
    if (root / '.git').is_file():
        raise failure('E_RUNTIME_PATH', 'External Git worktree metadata is not mounted')
    return root


class PodmanEngine:
    def __init__(self, home: Path, host_id: str, config: dict | None = None):
        self.home = Path(home).resolve()
        self.host_id = host_id
        self.config = config or {}
        self.lock = asyncio.Lock()
        self.recovered = False
        self.owned: set[str] = set()
        self.image_identity = None
        self.cleanup_failed = False

    def executable(self) -> str:
        if os.name != 'posix' or not Path('/proc/self/ns/pid').exists():
            raise failure('E_RUNTIME_PLATFORM', 'Isolated workspaces require a Linux server')
        value = shutil.which('podman')
        if not value:
            raise failure('E_RUNTIME_MISSING', 'Rootless Podman is not installed')
        return value

    def image(self) -> str:
        value = self.config.get('image')
        if not isinstance(value, str) or not IMAGE.fullmatch(value):
            raise failure('E_RUNTIME_IMAGE', 'Configure an image pinned by SHA256 digest')
        return value

    async def invoke(self, *args: str, timeout: float = 20):
        try:
            code, output = await command([self.executable(), '--remote=false', *args], timeout=timeout)
        except (OSError, TimeoutError, ValueError):
            raise failure('E_RUNTIME_UNAVAILABLE', 'Container runtime command failed') from None
        if code:
            raise failure('E_RUNTIME_UNAVAILABLE', 'Container runtime rejected the operation')
        return output

    async def status(self) -> dict:
        try:
            if self.cleanup_failed:
                raise failure('E_RUNTIME_UNAVAILABLE', 'Previous container cleanup was not confirmed')
            image = self.image()
            info = json.loads(await self.invoke('info', '--format', 'json'))
            host = info.get('host', {})
            if host.get('security', {}).get('rootless') is not True:
                raise failure('E_RUNTIME_ROOTLESS', 'Only rootless containers are supported')
            if host.get('cgroupVersion') != 'v2':
                raise failure('E_RUNTIME_LIMITS', 'Cgroup v2 is required for resource limits')
            images = json.loads(await self.invoke('image', 'inspect', image))
            identity = images[0].get('Id') if isinstance(images, list) and len(images) == 1 else None
            if not isinstance(identity, str) or not re.fullmatch(r'(?:sha256:)?[a-f0-9]{64}', identity):
                raise failure('E_RUNTIME_IMAGE', 'Image identity could not be verified')
            self.image_identity = identity.removeprefix('sha256:')
            return {'supported': True, 'state': 'configured', 'agents': ['claude']}
        except AdapterError as error:
            return {'supported': False, 'state': 'unavailable', 'reason': error.code, 'agents': ['claude']}
        except (ValueError, TypeError, AttributeError):
            return {'supported': False, 'state': 'unavailable', 'reason': 'E_RUNTIME_PROTOCOL', 'agents': ['claude']}

    def directory(self, runtime_id: str) -> Path:
        if not RUNTIME_ID.fullmatch(runtime_id):
            raise failure('E_TARGET', 'Invalid runtime identity')
        base = self.home / 'runtimes'
        if any(char in str(base) for char in (',', '\n', '\r', '\0')):
            raise failure('E_RUNTIME_PATH', 'Runtime data path is not supported')
        target = base / runtime_id / 'home'
        for parent in (base, target.parent, target):
            if parent.is_symlink():
                raise failure('E_RUNTIME_PATH', 'Runtime data directory cannot be a symlink')
            parent.mkdir(mode=0o700, exist_ok=True)
            if parent.resolve() != parent:
                raise failure('E_RUNTIME_PATH', 'Runtime data directory changed')
        return target

    def create_args(self, name: str, runtime_id: str, project: Path, data: Path) -> list[str]:
        return [
            'create', '--pull=never', '--interactive', '--init', '--name', name,
            '--label', f'{OWNER_LABEL}={self.host_id}', '--label', f'{RUNTIME_LABEL}={runtime_id}',
            '--userns=keep-id', '--user', f'{os.getuid()}:{os.getgid()}',
            '--network=private', '--pid=private', '--ipc=private', '--uts=private',
            '--cgroupns=private', '--privileged=false', '--http-proxy=false', '--env-host=false',
            '--cap-drop=all', '--security-opt=no-new-privileges', '--read-only',
            '--pids-limit=256', '--memory=2g', '--cpus=2', '--log-driver=none',
            '--mount', f'type=bind,src={project},dst=/workspace,rw',
            '--mount', f'type=bind,src={data},dst=/home/agent,rw',
            '--workdir=/workspace', '--env=HOME=/home/agent', '--env=PYTHONUNBUFFERED=1',
            '--env=AGENTDESK_ISOLATED_WORKER=1', '--entrypoint=/usr/local/bin/python',
            self.image(), '-m', 'agentdesk_host.runtime.worker',
        ]

    async def inspect(self, name: str) -> dict:
        rows = json.loads(await self.invoke('inspect', name))
        if not isinstance(rows, list) or len(rows) != 1 or not isinstance(rows[0], dict):
            raise failure('E_RUNTIME_PROTOCOL', 'Invalid container inspection')
        return rows[0]

    def verify(self, record: dict, runtime_id: str, project: Path, data: Path):
        config, host = record.get('Config', {}), record.get('HostConfig', {})
        if not self.image_identity or str(record.get('Image', '')).removeprefix('sha256:') != self.image_identity:
            raise failure('E_RUNTIME_IMAGE', 'Container image identity changed')
        labels = config.get('Labels', {})
        if labels.get(OWNER_LABEL) != self.host_id or labels.get(RUNTIME_LABEL) != runtime_id:
            raise failure('E_TARGET', 'Container belongs to a different runtime')
        if host.get('Privileged') is not False or host.get('ReadonlyRootfs') is not True:
            raise failure('E_RUNTIME_POLICY', 'Container filesystem or privilege policy changed')
        if host.get('NetworkMode') not in ('private', 'pasta', 'slirp4netns'):
            raise failure('E_RUNTIME_POLICY', 'Container must use private networking')
        if host.get('PidMode') not in ('', 'private') or host.get('IpcMode') != 'private' or host.get('UTSMode') not in ('', 'private'):
            raise failure('E_RUNTIME_POLICY', 'Container namespace policy changed')
        if host.get('UsernsMode') not in ('private', 'keep-id'):
            raise failure('E_RUNTIME_POLICY', 'Container user namespace policy changed')
        if 'no-new-privileges' not in host.get('SecurityOpt', []) or host.get('CapAdd'):
            raise failure('E_RUNTIME_POLICY', 'Container capability policy changed')
        if str(config.get('User')) != f'{os.getuid()}:{os.getgid()}':
            raise failure('E_RUNTIME_POLICY', 'Container user mapping changed')
        mounts = record.get('Mounts', [])
        expected = {'/workspace': str(project), '/home/agent': str(data)}
        actual = {m.get('Destination'): m.get('Source') for m in mounts if m.get('Type') == 'bind'}
        if actual != expected or sum(m.get('Type') == 'bind' for m in mounts) != 2 or any(m.get('Type') not in ('bind', 'tmpfs') for m in mounts):
            raise failure('E_RUNTIME_POLICY', 'Unexpected container mounts')
        if any(not m.get('RW') for m in mounts if m.get('Type') == 'bind'):
            raise failure('E_RUNTIME_POLICY', 'Project or runtime data is not writable')
        if host.get('PortBindings') or host.get('Devices') or host.get('VolumesFrom'):
            raise failure('E_RUNTIME_POLICY', 'Container exposes additional host resources')
        if host.get('PidsLimit') != 256 or host.get('Memory') != 2 * 1024**3:
            raise failure('E_RUNTIME_LIMITS', 'Container resource limits were not applied')
        quota, period, nano = host.get('CpuQuota', 0), host.get('CpuPeriod', 0), host.get('NanoCpus', 0)
        quota_valid = type(quota) is int and type(period) is int and period > 0 and quota == 2 * period
        if nano not in (0, 2_000_000_000) or (quota and not quota_valid) or not (nano == 2_000_000_000 or quota_valid):
            raise failure('E_RUNTIME_LIMITS', 'Container CPU limit was not applied')
        if host.get('LogConfig', {}).get('Type') != 'none':
            raise failure('E_RUNTIME_POLICY', 'Container output logging must be disabled')

    async def remove(self, name: str):
        try:
            await self._remove(name)
            if not self.owned:
                self.cleanup_failed = False
        except BaseException:
            self.cleanup_failed = True
            raise

    async def _remove(self, name: str):
        if name not in self.owned:
            return
        code, _ = await command([self.executable(), '--remote=false', 'container', 'exists', name])
        if code == 1:
            self.owned.discard(name)
            return
        if code != 0:
            raise failure('E_RUNTIME_UNAVAILABLE', 'Container ownership could not be checked')
        record = await self.inspect(name)
        if record.get('Config', {}).get('Labels', {}).get(OWNER_LABEL) != self.host_id:
            raise failure('E_TARGET', 'Refusing to remove an unowned container')
        await self.invoke('rm', '--force', name)
        self.owned.discard(name)

    async def launch(self, runtime_id: str, path: str):
        async with self.lock:
            status = await self.status()
            if not status['supported']:
                raise failure(status['reason'], 'Isolated runtime is unavailable')
            if not self.recovered:
                rows = json.loads(await self.invoke('ps', '--all', '--filter', f'label={OWNER_LABEL}={self.host_id}', '--format', 'json'))
                for row in rows:
                    names = row.get('Names', [])
                    name = names[0] if isinstance(names, list) and len(names) == 1 else None
                    if not isinstance(name, str) or not re.fullmatch(r'agentdesk-[a-f0-9]{32}', name):
                        raise failure('E_RUNTIME_POLICY', 'Unexpected owned container identity')
                    self.owned.add(name)
                    await self.remove(name)
                self.recovered = True
            project = validate_project(path, self.home)
            data = self.directory(runtime_id)
            name = 'agentdesk-' + uuid.uuid4().hex
            self.owned.add(name)
            try:
                await self.invoke(*self.create_args(name, runtime_id, project, data))
                self.verify(await self.inspect(name), runtime_id, project, data)
                process = await asyncio.create_subprocess_exec(
                    self.executable(), '--remote=false', 'start', '--attach', '--interactive', name,
                    stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.DEVNULL, limit=4 * 1024 * 1024,
                    env=native_environment(), start_new_session=True,
                )
                return name, process
            except BaseException:
                try:
                    await self.remove(name)
                except AdapterError:
                    pass
                raise

    async def close(self):
        errors = []
        for name in tuple(self.owned):
            try:
                await self.remove(name)
            except AdapterError as error:
                errors.append(error)
        if errors:
            raise errors[0]
