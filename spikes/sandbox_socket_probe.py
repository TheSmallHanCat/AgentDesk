"""No-model sandbox/socket checks. Never inspect authentication material."""

import json
import os
from pathlib import Path
import shlex
import socket
import subprocess
import sys
import threading
import uuid


def main():
    codex, python = sys.argv[1:3]
    base = Path.home() / ".agentdesk/devices" / ("agentdesk-e2e-" + uuid.uuid4().hex)
    base.mkdir(parents=True, mode=0o700)
    path = base / "tunnel.sock"
    proxy = base / "probe_proxy.py"
    proxy.write_text("import socket,os,sys\ns=socket.socket(socket.AF_UNIX)\ns.connect(sys.argv[1])\ns.sendall(os.read(0,1024))\nsys.stdout.buffer.write(s.recv(1024))\nsys.stdout.buffer.flush()\ns.close()\n")
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(str(path))
    os.chmod(path, 0o600)
    listener.listen(4)
    listener.settimeout(0.2)
    observed = []
    stopped = threading.Event()

    def serve():
        while not stopped.is_set():
            try:
                peer, _ = listener.accept()
            except socket.timeout:
                continue
            with peer:
                peer.settimeout(3)
                try:
                    data = peer.recv(1024)
                    observed.append(data.startswith(b"SSH-2.0-"))
                    peer.sendall(b"SSH-2.0-AgentDeskProbe\r\n")
                except OSError:
                    pass

    worker = threading.Thread(target=serve)
    worker.start()
    result = {"socketMode": "0600", "nc": {"status": "skipped", "reason": "not_installed"}, "modelCalls": False}
    ssh = ["/usr/bin/ssh", "-F", "/dev/null", "-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes", "-o", "IdentityAgent=none", "-o", "IdentityFile=none", "-o", "PreferredAuthentications=none", "-o", "ConnectTimeout=5", "-o", "StrictHostKeyChecking=yes", "-o", "UserKnownHostsFile=/dev/null", "-o", "ProxyCommand=" + shlex.join([python, str(proxy), str(path)]), "agentdesk-socket-probe"]
    try:
        config = ["-c", 'sandbox_mode="workspace-write"', "-c", "sandbox_workspace_write.network_access=true", "-c", "sandbox_workspace_write.writable_roots=" + json.dumps([str(base)])]
        before = len(observed)
        r = subprocess.run([codex, "sandbox", *config, "--", *ssh], capture_output=True, timeout=20)
        result["workspaceWrite"] = {"socketReachedBySsh": any(observed[before:]), "exitCode": r.returncode, "networkAccess": True, "writableRoot": "dedicated_device_probe_directory", "sshProxy": "python_AF_UNIX_nc_equivalent"}
        # A closed key exchange is intentional: the socket is not an SSH server.
        if not result["workspaceWrite"]["socketReachedBySsh"]:
            before = len(observed)
            r = subprocess.run([codex, "sandbox", "-c", 'sandbox_mode="danger-full-access"', "--", *ssh], capture_output=True, timeout=20)
            result["dangerFullAccess"] = {"socketReachedBySsh": any(observed[before:]), "exitCode": r.returncode}
        else:
            result["dangerFullAccess"] = {"status": "skipped", "reason": "least_privilege_tier_succeeded"}
    finally:
        stopped.set()
        worker.join(timeout=5)
        listener.close()
        path.unlink()
        proxy.unlink()
        base.rmdir()
    print(json.dumps(result))


if __name__ == "__main__":
    main()
