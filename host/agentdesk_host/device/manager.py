"""Per-control-connection execution credentials and stable device entry points."""
from __future__ import annotations

import asyncio
import hashlib
import ntpath
import os
import re
import shlex
import shutil
import stat
import sys
import uuid
from pathlib import Path

from ..adapters.base import AdapterError
from ..adapters.native_common import close_process


def windows_path(path: str) -> str:
    path = path.replace("/", "\\")
    if path.startswith("\\\\?\\UNC\\"):
        path = "\\\\" + path[8:]
    elif path.startswith("\\\\?\\"):
        path = path[4:]
    return ntpath.normpath(path)


def project_key(device_id: str, path: str, device_os: str) -> str:
    normalized = ntpath.normcase(windows_path(path)) if device_os == "windows" else os.path.normpath(path)
    return hashlib.sha256((device_id + "\0" + normalized).encode()).hexdigest()


def private_write(path: Path, text: str) -> None:
    temporary = path.with_name(path.name + "." + uuid.uuid4().hex)
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as output:
            output.write(text)
        os.replace(temporary, path)
        os.chmod(path, 0o600)
    finally:
        temporary.unlink(missing_ok=True)


class DeviceManager:
    def __init__(self, home: Path, state_changed, interrupt, *, wait_timeout=60, grace=300):
        self.home = home
        self.entries: dict[str, dict] = {}
        self.state_changed = state_changed
        self.interrupt = interrupt
        self.wait_timeout = wait_timeout
        self.grace = grace
        self.timers: dict[str, asyncio.Task] = {}

    def entry(self, device_id: str) -> Path:
        if not re.fullmatch(r"[A-Za-z0-9_-]{1,80}", device_id):
            raise AdapterError("E_INVALID_PARAMS", "device.register", "Invalid device identifier")
        path = self.home / "devices" / device_id
        if len(os.fsencode(path / "tunnel.sock")) > 100:
            raise AdapterError("E_DEVICE_PATH", "device.register", "Device socket path exceeds the platform limit")
        return path

    async def register(self, device_id: str, owner: object, device_os: str) -> dict:
        if device_os not in {"windows", "linux", "macos"}:
            raise AdapterError("E_INVALID_PARAMS", "device.register", "Unsupported device OS")
        previous = self.entries.get(device_id)
        if previous:
            # The caller has already authenticated over SSH and hello binds the
            # device identity. A stale control connection cannot block reconnect.
            await self.offline(device_id, previous["owner"])
        root = self.entry(device_id)
        root.mkdir(parents=True, exist_ok=True, mode=0o700)
        if root.is_symlink():
            raise AdapterError("E_DEVICE_PATH", "device.register", "Device entry cannot be a symlink")
        os.chmod(root, 0o700)
        self.cleanup(root)
        self.entries[device_id] = {"owner": owner, "os": device_os, "state": "preparing", "generation": uuid.uuid4().hex, "projects": set(), "sessions": {}}
        await self.state_changed(device_id, "preparing", None)
        return {"tunnelSocket": str(root / "tunnel.sock"), "hostKeyPinRequired": True, "generation": self.entries[device_id]["generation"]}

    def owned(self, device_id: str, owner: object) -> dict:
        entry = self.entries.get(device_id)
        if not entry or entry["owner"] is not owner:
            raise AdapterError("E_TARGET", "device.tunnel", "Execution channel belongs to another control connection")
        return entry

    async def ready(self, device_id: str, owner: object, params: dict) -> dict:
        entry = self.owned(device_id, owner)
        if not entry["projects"]:
            raise AdapterError("E_DEVICE_UNAUTHORIZED", "device.tunnel", "Enable a local project before providing execution credentials")
        if params.get("generation") != entry["generation"]:
            raise AdapterError("E_TARGET", "device.tunnel", "Stale execution channel generation")
        if entry["state"] == "ready":
            return {"state": "ready", "generation": entry["generation"]}
        key, host_key, user, command = (params.get(name) for name in ("privateKey", "hostPublicKey", "user", "scriptCommand"))
        if not all(isinstance(value, str) and value and "\0" not in value for value in (key, host_key, user, command)) or any("\n" in value or "\r" in value for value in (host_key, user, command)):
            raise AdapterError("E_INVALID_PARAMS", "device.tunnel", "Invalid execution channel parameters")
        if not key.startswith("-----BEGIN OPENSSH PRIVATE KEY-----") or not host_key.startswith("ssh-ed25519 "):
            raise AdapterError("E_INVALID_PARAMS", "device.tunnel", "Ed25519 OpenSSH keys are required")
        root = self.entry(device_id)
        keys = root / "keys"
        keys.mkdir(mode=0o700)
        private_write(keys / "id", key)
        private_write(keys / "known_hosts", "agentdesk-device " + host_key + "\n")
        proxy = shlex.join([sys.executable, "-m", "agentdesk_host.device.proxy", "--socket", str(root / "tunnel.sock"), "--ready", str(root / "ready"), "--timeout", str(self.wait_timeout)])
        config = f'Host device device-script\n  HostName agentdesk-device\n  User "{user.replace(chr(34), chr(92)+chr(34))}"\n  ProxyCommand {proxy}\n  IdentityFile "{keys / "id"}"\n  UserKnownHostsFile "{keys / "known_hosts"}"\n  StrictHostKeyChecking yes\n  IdentitiesOnly yes\n  BatchMode yes\n  RequestTTY no\n  ServerAliveInterval 10\n  ServerAliveCountMax 2\nHost device-script\n  RemoteCommand {command}\n'
        private_write(root / "ssh.conf", config)
        private_write(root / "ready", entry["generation"])
        entry["state"] = "probing"
        await self.state_changed(device_id, "probing", None)
        try:
            process = await asyncio.create_subprocess_exec("ssh", "-F", str(root / "ssh.conf"), "device-script", stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, start_new_session=True)
            try:
                output, diagnostic = await asyncio.wait_for(process.communicate(b"echo ok\n"), 20)
            finally:
                await close_process(process)
            if process.returncode != 0 or output.strip() != b"ok":
                if b"bad configuration option: remotecommand" in (diagnostic or b"").lower():
                    raise AdapterError("E_DEVICE_SSH_VERSION", "device.probe", "Host OpenSSH does not support RemoteCommand", "Install OpenSSH 7.6 or newer on the Host before enabling local execution")
                raise AdapterError("E_DEVICE_PROBE", "device.probe", "Device script probe failed", "Inspect the bundled sshd and PowerShell availability")
            if self.entries.get(device_id) is not entry or not entry["projects"]:
                raise AdapterError("E_TARGET", "device.probe", "Execution channel changed during probing")
        except (OSError, TimeoutError, AdapterError) as error:
            if self.entries.get(device_id) is entry:
                entry["state"] = "error"
                self.cleanup(root)
                await self.state_changed(device_id, "error", error.code if isinstance(error, AdapterError) else "E_DEVICE_PROBE")
            if isinstance(error, AdapterError):
                raise
            raise AdapterError("E_DEVICE_PROBE", "device.probe", "Device script probe failed", "Inspect the local executor and reverse SSH channel") from None
        entry["state"] = "ready"
        timer = self.timers.pop(device_id, None)
        if timer:
            timer.cancel()
        await self.state_changed(device_id, "ready", None)
        return {"state": "ready", "generation": entry["generation"]}

    def enable_project(self, device_id: str, owner: object, key: str) -> None:
        self.owned(device_id, owner)["projects"].add(key)

    async def revoke_project(self, device_id: str, owner: object, key: str) -> None:
        entry = self.owned(device_id, owner)
        entry["projects"].discard(key)
        for session_id, project in list(entry["sessions"].items()):
            if project == key:
                (self.home / "session-ssh" / (session_id + ".conf")).unlink(missing_ok=True)
                del entry["sessions"][session_id]
        if not entry["projects"]:
            await self.offline(device_id, owner)

    def require_ready(self, device_id: str, owner: object, key: str) -> None:
        entry = self.owned(device_id, owner)
        if key not in entry["projects"]:
            raise AdapterError("E_DEVICE_UNAUTHORIZED", "local.run", "This project is not authorized for local execution")
        if entry["state"] != "ready":
            raise AdapterError("E_DEVICE_NOT_READY", "run.send", "Device execution channel is not ready")

    def session_config(self, session_id: str, device_id: str, owner: object, key: str) -> Path:
        self.require_ready(device_id, owner, key)
        if not re.fullmatch(r"[A-Za-z0-9_-]{1,100}", session_id):
            raise AdapterError("E_TARGET", "device.session", "Invalid session identifier")
        root = self.home / "session-ssh"
        root.mkdir(parents=True, exist_ok=True, mode=0o700)
        path = root / (session_id + ".conf")
        private_write(path, 'Include "' + str(self.entry(device_id) / "ssh.conf") + '"\n')
        self.owned(device_id, owner)["sessions"][session_id] = key
        return path

    async def offline(self, device_id: str, owner: object, generation: str | None = None) -> None:
        entry = self.entries.get(device_id)
        if not entry or entry["owner"] is not owner or generation is not None and generation != entry["generation"]:
            return
        self.entries.pop(device_id)
        self.cleanup(self.entry(device_id))
        await self.state_changed(device_id, "closed", "device_offline")
        async def expire():
            await asyncio.sleep(self.grace)
            if self.entries.get(device_id, {}).get("state") != "ready":
                await self.state_changed(device_id, "closed", "grace_expired")
                await self.interrupt(device_id)
        timer = self.timers.get(device_id)
        if timer is None or timer.done():
            self.timers[device_id] = asyncio.create_task(expire())

    async def disconnect(self, owner: object) -> None:
        for device_id, entry in list(self.entries.items()):
            if entry["owner"] is owner:
                await self.offline(device_id, owner)

    @staticmethod
    def cleanup(root: Path) -> None:
        if root.is_symlink() or root.parent.is_symlink():
            raise AdapterError("E_DEVICE_PATH", "device.cleanup", "Device entry cannot be a symlink")
        (root / "ready").unlink(missing_ok=True)
        socket_path = root / "tunnel.sock"
        if socket_path.exists() or socket_path.is_symlink():
            if not stat.S_ISSOCK(socket_path.lstat().st_mode):
                raise AdapterError("E_DEVICE_PATH", "device.cleanup", "Device socket path is occupied by another file")
            socket_path.unlink()
        keys = root / "keys"
        if keys.is_symlink():
            raise AdapterError("E_DEVICE_PATH", "device.cleanup", "Device key directory cannot be a symlink")
        if keys.exists():
            shutil.rmtree(keys)

    async def close(self):
        for device_id, entry in list(self.entries.items()):
            await self.offline(device_id, entry["owner"])
        for task in self.timers.values():
            task.cancel()
        await asyncio.gather(*self.timers.values(), return_exceptions=True)


def instructions(config: Path, device_os: str, project_path: str) -> str:
    if device_os == "windows":
        usage = "Send a complete PowerShell script using a quoted here-document:\nssh -F " + shlex.quote(str(config)) + " device-script <<'AGENTDESK_SCRIPT'\nSet-Location -LiteralPath '" + project_path.replace("'", "''") + "'\n# PowerShell commands\nAGENTDESK_SCRIPT"
        location = "The project is on the user's Windows computer."
    else:
        usage = "Send a shell script through stdin to ssh -F " + shlex.quote(str(config)) + " device-script."
        location = "The project is on the user's " + device_os + " computer."
    return location + " Native file tools operate on the server working directory. Use the device-script SSH alias for project operations; the device alias provides direct commands.\n" + usage
