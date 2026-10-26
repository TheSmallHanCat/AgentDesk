"""Shared transport for opt-in server checks; never emit endpoint or credentials."""

from __future__ import annotations

import json
import logging
import os
from pathlib import Path

try:
    import paramiko
except ImportError:
    paramiko = None

ROOT = Path(__file__).resolve().parents[1]
PRIVATE = ROOT / ".tmp" / "e2e"
REMOTE = "/home/agentdesk-test/agentdesk-e2e"
REQUIRED = (
    "AGENTDESK_E2E_HOST",
    "AGENTDESK_E2E_PORT",
    "AGENTDESK_E2E_ROOT_USER",
    "AGENTDESK_E2E_ROOT_PASSWORD",
)
logging.getLogger("paramiko").disabled = True
logging.getLogger("paramiko").addHandler(logging.NullHandler())
logging.getLogger("paramiko").propagate = False


def configured() -> bool:
    missing = [name for name in REQUIRED if not os.environ.get(name)]
    if missing:
        print("SKIP: missing " + ", ".join(missing))
        return False
    if paramiko is None:
        print("SKIP: install the optional Paramiko E2E dependency")
        return False
    return True


def save_result(name: str, value: dict) -> None:
    PRIVATE.mkdir(parents=True, exist_ok=True)
    text = json.dumps(value, ensure_ascii=False, indent=2)
    for variable in ("AGENTDESK_E2E_HOST", "AGENTDESK_E2E_ROOT_PASSWORD"):
        secret = os.environ.get(variable)
        if secret and secret in text:
            raise RuntimeError("Result contains a protected environment value")
    (PRIVATE / (name + ".json")).write_text(text + "\n", encoding="utf-8")


class PinnedKey(paramiko.MissingHostKeyPolicy if paramiko is not None else object):
    def missing_host_key(self, client, hostname, key):
        # Endpoint-free pin, shared by provisioning and subsequent test sessions.
        pin = PRIVATE / "server_key.pub"
        encoded = key.get_name() + " " + key.get_base64()
        if pin.exists():
            if pin.read_text(encoding="ascii").strip() != encoded:
                raise RuntimeError("Test server host key changed")
        else:
            PRIVATE.mkdir(parents=True, exist_ok=True)
            pin.write_text(encoded + "\n", encoding="ascii")


def connect(*, root: bool = False, password: bool = False):
    client = paramiko.SSHClient()
    client.set_missing_host_key_policy(PinnedKey())
    kwargs = {
        "hostname": os.environ["AGENTDESK_E2E_HOST"],
        "port": int(os.environ["AGENTDESK_E2E_PORT"]),
        "username": os.environ["AGENTDESK_E2E_ROOT_USER"] if root else "agentdesk-test",
        "look_for_keys": False,
        "allow_agent": False,
        "timeout": 15,
        "auth_timeout": 15,
        "banner_timeout": 15,
    }
    policy_path = PRIVATE / "auth-server-policy.json"
    key_disabled = policy_path.exists() and "pubkeyauthentication=no" in policy_path.read_text(encoding="utf-8")
    if root:
        kwargs["password"] = os.environ["AGENTDESK_E2E_ROOT_PASSWORD"]
    elif password or key_disabled:
        kwargs["password"] = (PRIVATE / "user_password").read_text(encoding="ascii").strip()
    else:
        kwargs["key_filename"] = str(PRIVATE / "id_ed25519")
    try:
        client.connect(**kwargs)
        return client
    except Exception:
        client.close()
        raise RuntimeError("SSH connection to test server failed") from None


def run(client, script: str, *, timeout: int = 60, check: bool = True) -> tuple[int, str]:
    channel = client.get_transport().open_session(timeout=15)
    channel.settimeout(timeout)
    channel.exec_command("exec /bin/sh -s")
    channel.sendall(script.encode("utf-8"))
    channel.shutdown_write()
    output = bytearray()
    errors = bytearray()
    # Drain both streams to avoid an stderr window deadlock.
    import time

    deadline = time.monotonic() + timeout
    try:
        while True:
            if channel.recv_ready():
                output.extend(channel.recv(65536))
            if channel.recv_stderr_ready():
                errors.extend(channel.recv_stderr(65536))
            if channel.exit_status_ready() and not channel.recv_ready() and not channel.recv_stderr_ready():
                break
            if time.monotonic() >= deadline:
                raise RuntimeError("Test server script timed out")
            time.sleep(0.01)
        code = channel.recv_exit_status()
        if check and code:
            # Raw server errors may include endpoint, account data, or secrets.
            raise RuntimeError(f"Test server script failed (exit {code})")
        return code, output.decode("utf-8", errors="replace")
    finally:
        channel.close()


def upload(client, local: Path, remote: str, mode: int = 0o600) -> None:
    if not remote.startswith("/home/agentdesk-test/"):
        raise ValueError("Uploads must belong to the dedicated test user")
    with client.open_sftp() as session:
        session.put(str(local), remote)
        session.chmod(remote, mode)
