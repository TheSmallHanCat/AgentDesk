"""Opt-in S6/S3/S4 experiments; isolated from every product build."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shlex
import sys
import tarfile
import threading
import time
import urllib.request
import uuid

try:
    import paramiko
    from paramiko.common import cMSG_CHANNEL_OPEN
except ModuleNotFoundError:
    paramiko = None

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts" / "e2e"))
from e2e_common import PRIVATE, REMOTE, configured, connect, run, save_result  # noqa: E402

SPIKES = Path(__file__).resolve().parent
UV_VERSION = "0.13.5"
CODEX_VERSION = "rust-v0.164.0"
# Release asset digests were read from the upstream GitHub release API.
ASSETS = {
    "uv": (
        "astral-sh/uv",
        UV_VERSION,
        "uv-x86_64-unknown-linux-musl.tar.gz",
        "57ef9d9ccb01fb1bd998df995b0dd221b52f7d7841fbea93f2d4463bf61fb7a9",
        "uv-x86_64-unknown-linux-musl/uv",
    ),
    "codex": (
        "openai/codex",
        CODEX_VERSION,
        "codex-x86_64-unknown-linux-musl.tar.gz",
        "90dd0b03b28afcb7fa733a4f73b413db2916a47347efd43fb669cc1c774ee8e4",
        "codex-x86_64-unknown-linux-musl",
    ),
    "bwrap": (
        "openai/codex",
        CODEX_VERSION,
        "bwrap-x86_64-unknown-linux-musl.tar.gz",
        "60cd1c139823eabac2e4544cc75081d02af5100cfd276efa378dd48a123e109d",
        "bwrap-x86_64-unknown-linux-musl",
    ),
}


def asset(name: str) -> Path:
    repository, version, filename, digest, member = ASSETS[name]
    cache = PRIVATE / "spike-assets"
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / (digest + ".tar.gz")
    if not archive.exists():
        url = f"https://github.com/{repository}/releases/download/{version}/{filename}"
        request = urllib.request.Request(url, headers={"User-Agent": "AgentDesk-Spike"})
        deadline = time.monotonic() + 240
        with urllib.request.urlopen(request, timeout=30) as response, archive.open("xb") as handle:
            while chunk := response.read(1024 * 1024):
                if time.monotonic() > deadline:
                    raise TimeoutError("Public release asset download timed out")
                handle.write(chunk)
    checksum = hashlib.sha256()
    with archive.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            checksum.update(chunk)
    if checksum.hexdigest() != digest:
        raise RuntimeError("Public release asset checksum mismatch")
    binary = cache / (digest + ".bin")
    if not binary.exists():
        # Extract exactly one regular member; never honor archive paths or links.
        with tarfile.open(archive, mode="r:gz") as package:
            entry = package.getmember(member)
            if not entry.isfile():
                raise RuntimeError("Public release asset is not a regular binary")
            source = package.extractfile(entry)
            assert source is not None
            with source, binary.open("xb") as destination:
                while chunk := source.read(1024 * 1024):
                    destination.write(chunk)
    return binary


def upload_new(client: paramiko.SSHClient, local: Path, remote: str, mode: int) -> None:
    if not remote.startswith("/home/agentdesk-test/") or "/../" in remote:
        raise ValueError("Upload must belong to the test user")
    with client.open_sftp() as session:
        session.get_channel().settimeout(30)
        # Paramiko requires both x and w to request O_EXCL plus write access.
        with session.open(remote, "wx") as destination, local.open("rb") as source:
            destination.set_pipelined(True)
            deadline = time.monotonic() + 180
            while chunk := source.read(256 * 1024):
                if time.monotonic() > deadline:
                    raise TimeoutError("Exclusive artifact upload timed out")
                destination.write(chunk)
        session.chmod(remote, mode)


def direct_streamlocal(client: paramiko.SSHClient, path: str) -> paramiko.Channel:
    """OpenSSH streamlocal wire format; Paramiko has no public helper for it.

    This bounded, spike-only adapter mirrors Paramiko Transport.open_channel.
    Product code continues to use russh, verified separately by the S1 suite.
    """
    if not path.startswith(REMOTE + "/") or "/../" in path:
        raise ValueError("Socket must belong to the isolated test directory")
    transport = client.get_transport()
    if transport is None or not transport.is_active():
        raise RuntimeError("SSH transport is unavailable")
    with transport.lock:
        identifier = transport._next_channel()
        channel = paramiko.Channel(identifier)
        transport._channels.put(identifier, channel)
        event = threading.Event()
        transport.channel_events[identifier] = event
        transport.channels_seen[identifier] = True
        channel._set_transport(transport)
        channel._set_window(transport.default_window_size, transport.default_max_packet_size)
        message = paramiko.Message()
        message.add_byte(cMSG_CHANNEL_OPEN)
        message.add_string("direct-streamlocal@openssh.com")
        message.add_int(identifier)
        message.add_int(transport.default_window_size)
        message.add_int(transport.default_max_packet_size)
        message.add_string(path)
        message.add_string("")
        message.add_int(0)
    transport._send_user_message(message)
    if not event.wait(15) or not transport.is_active() or transport._channels.get(identifier) is None:
        channel.close()
        raise RuntimeError("Streamlocal channel was not confirmed")
    channel.settimeout(10)
    return channel


def step(client: paramiko.SSHClient, work: str, name: str, script: str, timeout: int = 180) -> dict:
    log = shlex.quote(work + "/" + name + ".out")
    wrapper = f"""
umask 077
(
set -eu
{script}
) >{log} 2>&1
code=$?
glibc=false
network=false
grep -Eq 'GLIBC_|GLIBCXX_' {log} && glibc=true
grep -Eiq 'timed out|certificate|connection|dns|resolve|network' {log} && network=true
printf '{{"exitCode":%s,"glibcIncompatible":%s,"networkFailure":%s}}\\n' "$code" "$glibc" "$network"
"""
    _, output = run(client, wrapper, timeout=timeout + 15)
    value = json.loads(output)
    value["networkTextObserved"] = value["networkFailure"]
    value["networkFailure"] = value["exitCode"] != 0 and value["networkTextObserved"]
    value["status"] = "passed" if value["exitCode"] == 0 else "failed"
    return value


def install_asset(client: paramiko.SSHClient, work: str, install: str, name: str, *, download_remote: bool) -> dict:
    repository, version, filename, digest, member = ASSETS[name]
    archive = work + "/" + name + ".tar.gz"
    if download_remote:
        url = f"https://github.com/{repository}/releases/download/{version}/{filename}"
        fetch = f"""
test ! -e {shlex.quote(archive)}
(set -C; curl -q -fLsS --connect-timeout 15 --max-time 180 --retry 0 {shlex.quote(url)} >{shlex.quote(archive)})
"""
    else:
        # Upload the official compressed static binary, then unpack exclusively
        # inside BASE. This avoids hundreds of MB of uncompressed SSH uploads.
        asset(name)
        upload_new(client, PRIVATE / "spike-assets" / (digest + ".tar.gz"), archive, 0o600)
        fetch = ""
    return step(client, work, name + "-install", fetch + f"""
printf '%s  %s\\n' {shlex.quote(digest)} {shlex.quote(archive)} | sha256sum -c -
tar -xzf {shlex.quote(archive)} -C {shlex.quote(work)} -- {shlex.quote(member)}
(set -C; cat {shlex.quote(work + '/' + member)} >{shlex.quote(install + '/' + name)})
chmod 700 {shlex.quote(install + '/' + name)}
""", 200 if download_remote else 30)


def inspect_service(client: paramiko.SSHClient, path: str) -> bool:
    with client.open_sftp() as session:
        try:
            attributes = session.lstat(path + "/host.sock")
            return attributes.st_mode & 0o777 == 0o600
        except OSError:
            return False


def wait_service(client: paramiko.SSHClient, path: str, *, present: bool) -> None:
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if inspect_service(client, path) is present:
            return
        time.sleep(0.2)
    raise RuntimeError("Socket service did not reach the expected state")


def cleanup_service(client: paramiko.SSHClient, path: str, python: str) -> None:
    # Kill only the PID whose command line names this invocation's script and
    # directory. A stale or replaced PID must never kill a different process.
    script = f"""
import os, pathlib, signal, time
base = pathlib.Path({path!r})
pid_file = base / 'host.pid'
if pid_file.exists():
    pid = int(pid_file.read_text())
    try:
        arguments = pathlib.Path('/proc/' + str(pid) + '/cmdline').read_bytes().split(b'\\0')
        if bytes(str(base), 'utf-8') not in arguments or not any(value.endswith(b'/socket_service.py') for value in arguments):
            raise RuntimeError('Process ownership changed')
        os.kill(pid, signal.SIGTERM)
        deadline = time.monotonic() + 8
        while pid_file.exists() and time.monotonic() < deadline:
            time.sleep(0.1)
        if pid_file.exists():
            raise RuntimeError('Socket process cleanup timed out')
    except FileNotFoundError:
        raise RuntimeError('Socket process exited without cleanup')
assert not (base / 'host.sock').exists()
"""
    run(client, shlex.quote(python) + " - <<'PY'\n" + script + "\nPY\n", timeout=15)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-cli-install", action="store_true", help="Probe existing CLIs only")
    arguments = parser.parse_args()
    if not configured():
        return 0
    identifier = uuid.uuid4().hex[:12]
    work = REMOTE + "/spikes-" + identifier
    install = "/home/agentdesk-test/.agentdesk/spikes/" + identifier
    python = install + "/venv/bin/python"
    client = None
    services: list[str] = []
    result: dict = {
        "runId": identifier,
        "versions": {"uv": UV_VERSION, "codex": CODEX_VERSION},
        "remoteChanges": [work, install],
        "rootUsed": False,
        "lingerChanged": False,
        "modelCalls": False,
    }
    try:
        client = connect()
        _, output = run(client, """
test "$(id -un)" = agentdesk-test || exit 1
test "$(uname -m)" = x86_64 || exit 1
timeout 10 systemctl --user show-environment >/dev/null 2>&1
code=$?
if test "$code" = 0; then printf 'systemd=true\n'; else printf 'systemd=false\n'; fi
timeout 10 loginctl show-user "$(id -u)" --property=Linger 2>/dev/null | sed -n '/^Linger=/p'
command -v codex >/dev/null 2>&1 && printf 'codex=true\n' || printf 'codex=false\n'
""", timeout=20)
        flags = dict(line.split("=", 1) for line in output.splitlines() if "=" in line)
        result["systemdUserAvailable"] = flags.get("systemd") == "true"
        result["linger"] = flags.get("Linger") in ("yes", "true")
        run(client, f"umask 077\nmkdir -p {shlex.quote(REMOTE)} {shlex.quote('/home/agentdesk-test/.agentdesk/spikes')}\nmkdir {shlex.quote(work)} {shlex.quote(install)}\n")
        print("STAGE uv_upload_started", flush=True)
        result["uvInstall"] = install_asset(client, work, install, "uv", download_remote=False)
        if result["uvInstall"]["status"] != "passed":
            raise RuntimeError("Static runtime installation failed")
        print("STAGE uv_upload_completed", flush=True)
        environment = f"""
export UV_NO_CONFIG=1 UV_NO_PROGRESS=1 UV_HTTP_RETRIES=1 UV_HTTP_TIMEOUT=45
export UV_PYTHON_INSTALL_DIR={shlex.quote(install + '/python')}
export UV_PYTHON_BIN_DIR={shlex.quote(install + '/bin')}
export UV_CACHE_DIR={shlex.quote(work + '/uv-cache')}
export XDG_CACHE_HOME={shlex.quote(work + '/cache')}
export TMPDIR={shlex.quote(work)}
"""
        result["uv"] = step(client, work, "uv", shlex.quote(install + "/uv") + " --version", 20)
        print("STAGE uv_" + result["uv"]["status"], flush=True)
        print("STAGE python312_started", flush=True)
        result["python312"] = step(client, work, "python", environment + f"timeout 180 {shlex.quote(install + '/uv')} python install 3.12\ntimeout 30 {shlex.quote(install + '/uv')} venv --managed-python --python 3.12 {shlex.quote(install + '/venv')}\n{shlex.quote(python)} -c 'import sys; assert sys.version_info[:2] == (3, 12)'", 220)
        print("STAGE python312_" + result["python312"]["status"], flush=True)
        if result["python312"]["status"] != "passed":
            result["s6Socket"] = {"status": "skipped", "reason": "python312_unavailable"}
            result["agents"] = {"status": "skipped", "reason": "python312_unavailable"}
            return 1
        for name in ("socket_service.py", "agent_probe.py", "environment_probe.py"):
            upload_new(client, SPIKES / name, work + "/" + name, 0o600)
        # Resolve the official PyPI package version locally, then pin this run.
        request = urllib.request.Request("https://pypi.org/pypi/claude-agent-sdk/json", headers={"User-Agent": "AgentDesk-Spike"})
        with urllib.request.urlopen(request, timeout=20) as response:
            sdk_version = json.load(response)["info"]["version"]
        if not all(character.isdigit() or character == "." for character in sdk_version):
            raise RuntimeError("Unexpected official SDK version")
        result["versions"]["claudeAgentSdk"] = sdk_version
        print("STAGE claude_sdk_started", flush=True)
        result["claudeSdkInstall"] = step(client, work, "claude-sdk", environment + f"timeout 180 {shlex.quote(install + '/uv')} pip install --python {shlex.quote(python)} --only-binary :all: {shlex.quote('claude-agent-sdk==' + sdk_version)}", 195)
        print("STAGE claude_sdk_" + result["claudeSdkInstall"]["status"], flush=True)
        codex = None
        if flags.get("codex") != "true" and not arguments.skip_cli_install:
            print("STAGE codex_upload_started", flush=True)
            result["codexInstall"] = install_asset(client, work, install, "codex", download_remote=True)
            if result["codexInstall"]["status"] == "passed":
                codex = install + "/codex"
                run(client, "mkdir -m 700 " + shlex.quote(install + "/codex-resources") + "\n")
                result["codexResourcesInstall"] = install_asset(client, work, install + "/codex-resources", "bwrap", download_remote=True)
            print("STAGE codex_install_" + result["codexInstall"]["status"], flush=True)
        command_line = shlex.quote(python) + " " + shlex.quote(work + "/agent_probe.py")
        if codex:
            command_line += " " + shlex.quote(codex)
        print("STAGE agent_probe_started", flush=True)
        _, output = run(client, "cd " + shlex.quote(work) + "\n" + command_line + "\n", timeout=120)
        result["agents"] = json.loads(output)
        print("STAGE agent_probe_completed", flush=True)
        # The detached process has no inherited terminal or SSH standard streams.
        detached = work + "/detached"
        services.append(detached)
        print("STAGE detached_socket_started", flush=True)
        run(client, f"mkdir -m 700 {shlex.quote(detached)}\nsetsid nohup {shlex.quote(python)} {shlex.quote(work + '/socket_service.py')} {shlex.quote(detached)} </dev/null >{shlex.quote(detached + '/stdout')} 2>{shlex.quote(detached + '/stderr')} &\n", timeout=20)
        wait_service(client, detached, present=True)
        client.close()
        client = None
        time.sleep(2)
        client = connect()
        wait_service(client, detached, present=True)
        with direct_streamlocal(client, detached + "/host.sock") as channel:
            channel.sendall(b"agentdesk-s6")
            if channel.recv(256) != b"agentdesk-s6":
                raise RuntimeError("Detached socket echo did not match")
        result["s6Socket"] = {"status": "passed", "mode": "0600", "survivedSshDisconnect": True, "directStreamlocal": True}
        cleanup_service(client, detached, python)
        services.remove(detached)
        result["s6Socket"]["cleaned"] = True
        print("STAGE detached_socket_passed", flush=True)
        if result["systemdUserAvailable"]:
            unit = "agentdesk-spike-" + identifier
            managed = work + "/managed"
            services.append(managed)
            result["systemdUnit"] = unit
            run(client, f"mkdir -m 700 {shlex.quote(managed)}\ntimeout 15 systemd-run --user --unit={shlex.quote(unit)} {shlex.quote(python)} {shlex.quote(work + '/socket_service.py')} {shlex.quote(managed)} >/dev/null 2>&1\n", timeout=20)
            wait_service(client, managed, present=True)
            client.close()
            client = None
            time.sleep(2)
            client = connect()
            wait_service(client, managed, present=True)
            result["systemdService"] = {"status": "passed", "survivedSshDisconnect": True}
            run(client, "timeout 15 systemctl --user stop " + shlex.quote(unit) + " >/dev/null 2>&1\n", timeout=20)
            wait_service(client, managed, present=False)
            services.remove(managed)
        else:
            result["systemdService"] = {"status": "skipped", "reason": "user_manager_unavailable_linger_not_changed"}
        _, output = run(client, shlex.quote(python) + " " + shlex.quote(work + "/environment_probe.py") + "\n", timeout=30)
        result["audit"] = json.loads(output)
        result["versions"]["python"] = result["audit"]["pythonVersion"]
        return 0
    except Exception:
        # Do not include exception text: SSH/HTTP errors may contain endpoints.
        result["failure"] = "bounded_spike_failed_see_completed_stage_results"
        return 1
    finally:
        result["cleanup"] = "passed"
        if services and client is None:
            try:
                client = connect()
            except Exception:
                result["cleanup"] = "failed_reconnect_unavailable_process_has_180s_deadline"
        if client is not None:
            for service in services:
                try:
                    cleanup_service(client, service, python)
                except Exception:
                    result["cleanup"] = "failed_process_has_180s_deadline"
            client.close()
        save_result("spikes", result)
        print(json.dumps(result, ensure_ascii=True, indent=2))


if __name__ == "__main__":
    raise SystemExit(main())
