"""Read-only discovery, then explicit provisioning in a dedicated account."""

from __future__ import annotations

import argparse
import json
import os
import secrets
import subprocess

from e2e_common import PRIVATE, REMOTE, configured, connect, run, save_result

PROBE = r"""
set -eu
. /etc/os-release
printf 'distribution=%s %s\n' "$ID" "$VERSION_ID"
printf 'architecture='; uname -m
printf 'kernel='; uname -r
printf 'glibc='; getconf GNU_LIBC_VERSION || true
for tool in node npm claude codex python python3 getfacl fish tcsh systemctl loginctl setsid nohup tar gzip ss; do
  if command -v "$tool" >/dev/null 2>&1; then printf '%s=present\n' "$tool"; else printf '%s=absent\n' "$tool"; fi
done
if command -v node >/dev/null 2>&1; then node --version; fi
if command -v systemctl >/dev/null 2>&1; then
  if systemctl --user show-environment >/dev/null 2>&1; then printf 'root-user-manager=available\n'; else printf 'root-user-manager=unavailable\n'; fi
fi
if command -v sshd >/dev/null 2>&1; then
  sshd -T 2>/dev/null | while read -r key value; do
    case "$key" in allowstreamlocalforwarding|streamlocalbindmask|streamlocalbindunlink|pubkeyauthentication|passwordauthentication) printf '%s=%s\n' "$key" "$value";; esac
  done
fi
if id agentdesk-test >/dev/null 2>&1; then printf 'test-user=existing\n'; else printf 'test-user=absent\n'; fi
"""


def keys() -> None:
    PRIVATE.mkdir(parents=True, exist_ok=True)
    password = PRIVATE / "user_password"
    if not password.exists():
        password.write_text(secrets.token_urlsafe(36) + "\n", encoding="ascii")
    passphrase = PRIVATE / "key_passphrase"
    if not passphrase.exists():
        passphrase.write_text(secrets.token_urlsafe(32) + "\n", encoding="ascii")
    variants = [
        ("id_ed25519", ["-t", "ed25519"], ""),
        ("id_ed25519_encrypted", ["-t", "ed25519"], passphrase.read_text().strip()),
        ("id_rsa", ["-t", "rsa", "-b", "3072", "-m", "PEM"], ""),
        ("id_wrong", ["-t", "ed25519"], ""),
    ]
    for name, options, secret in variants:
        if not (PRIVATE / name).exists():
            completed = subprocess.run(
                ["ssh-keygen", *options, "-N", secret, "-C", "agentdesk-e2e", "-f", str(PRIVATE / name)],
                capture_output=True,
            )
            if completed.returncode:
                raise RuntimeError("Dedicated test key generation failed")


def provision() -> None:
    keys()
    public = "\n".join(
        (PRIVATE / name).read_text(encoding="ascii").strip()
        for name in ("id_ed25519.pub", "id_ed25519_encrypted.pub", "id_rsa.pub")
    )
    password = (PRIVATE / "user_password").read_text(encoding="ascii").strip()
    # Credentials travel solely over the authenticated encrypted channel stdin.
    script = """set -eu
if id agentdesk-test >/dev/null 2>&1; then
  test "$(getent passwd agentdesk-test | cut -d: -f6)" = /home/agentdesk-test
  test -f /home/agentdesk-test/.agentdesk-e2e-owned || exit 73
else
  useradd --create-home --shell /bin/bash agentdesk-test
  install -o agentdesk-test -g agentdesk-test -m 600 /dev/null /home/agentdesk-test/.agentdesk-e2e-owned
fi
install -d -o agentdesk-test -g agentdesk-test -m 700 /home/agentdesk-test/.ssh
"""
    import shlex

    # Append only missing dedicated keys; never replace an existing authorized_keys.
    for line in public.splitlines():
        script += f"grep -Fqx -- {shlex.quote(line)} /home/agentdesk-test/.ssh/authorized_keys 2>/dev/null || printf '%s\\n' {shlex.quote(line)} >> /home/agentdesk-test/.ssh/authorized_keys\n"
    script += "chown agentdesk-test:agentdesk-test /home/agentdesk-test/.ssh/authorized_keys\nchmod 600 /home/agentdesk-test/.ssh/authorized_keys\n"
    script += "printf '%s\\n' " + shlex.quote("agentdesk-test:" + password) + " | chpasswd\n"
    with connect(root=True) as client:
        run(client, script)
        _, public_key = run(client, "if test -r /etc/ssh/ssh_host_ecdsa_key.pub; then cat /etc/ssh/ssh_host_ecdsa_key.pub; fi")
        if public_key.strip():
            (PRIVATE / "server_ecdsa.pub").write_text(" ".join(public_key.split()[:2]) + "\n", encoding="ascii")
    with connect(password=True) as client:
        _, home = run(client, 'printf "%s" "$HOME"')
        if home != "/home/agentdesk-test":
            raise RuntimeError("Unexpected test user home directory")
        run(client, f"set -eu\nmkdir -p '{REMOTE}'\nchmod 700 '{REMOTE}'\n")
    print("PASS: dedicated user, three authorized test keys, random password")
    save_result("provision", {"user_created_or_owned": True, "authorized_keys": 3, "root_password_auth": True, "user_password_auth": True})


def fixtures() -> None:
    with connect() as client:
        script = f"""set -eu
mkdir -p '{REMOTE}/files' '{REMOTE}/大目录 CON' '{REMOTE}/空 格'\"'\"'中文' '{REMOTE}/no-access' '{REMOTE}/nonwritable'
chmod 700 '{REMOTE}'
chmod 700 '{REMOTE}/nonwritable'
chmod -R u+rwX '{REMOTE}/files'
"""
        run(client, script)
        with client.open_sftp() as sftp:
            for name, data in {
                "lf.txt": b"first\nsecond\n",
                "crlf.txt": b"first\r\nsecond\r\n",
                "mixed.txt": b"first\r\nsecond\n",
                "binary.bin": bytes([0xff, 0xfe, 0]),
                "limit.txt": b"a" * (2 * 1024 * 1024),
                "over-limit.txt": b"a" * (2 * 1024 * 1024 + 1),
                ".hidden": b"hidden\n",
                "readonly.txt": b"read only\n",
                "hardlink.txt": b"hard link\n",
                "conflict.txt": b"old\n",
            }.items():
                path = f"{REMOTE}/files/{name}"
                with sftp.open(path, "wb") as handle:
                    handle.write(data)
                sftp.chmod(path, 0o400 if name == "readonly.txt" else 0o600)
        run(client, f"""set -eu
cd '{REMOTE}'
ln -sf files/lf.txt link
ln -sf files/missing dangling
ln -f files/hardlink.txt files/hardlink-alias.txt
printf 'untouched\\n' > nonwritable/file.txt
chmod 500 nonwritable
chmod 000 no-access
mkdir -p default-acl
printf 'private\n' > default-acl/file.txt
chmod 600 default-acl/file.txt
setfacl -b default-acl/file.txt
setfacl -m d:u:65534:r-- default-acl
printf 'special\n' > files/special-mode.txt
chmod 4755 files/special-mode.txt
printf 'reserved\\n' > '大目录 CON/CON'
printf '中文内容\\n' > '大目录 CON/中文.txt'
if test ! -f '大目录 CON/payload.bin'; then dd if=/dev/urandom of='大目录 CON/payload.bin' bs=1048576 count=210 status=none; fi
mkdir -p bin
printf '#!/bin/sh\\nprintf path-ready\\n' > bin/agentdesk-e2e-path
chmod 700 bin/agentdesk-e2e-path
if ! grep -q '^# AgentDesk E2E PATH$' "$HOME/.bashrc"; then
  printf '\\n# AgentDesk E2E PATH\\nexport PATH="$HOME/agentdesk-e2e/bin:$PATH"\\n' >> "$HOME/.bashrc"
fi
""")
    print("PASS: scoped SFTP, PTY, 210 MiB incompressible download fixtures")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("probe", "provision", "fixtures"))
    args = parser.parse_args()
    if not configured():
        return
    if args.action == "probe":
        with connect(root=True) as client:
            _, output = run(client, PROBE)
        save_result("probe", {"observations": output.splitlines()})
        save_result("auth-server-policy", {"settings": [line for line in output.splitlines() if line.startswith(("pubkeyauthentication=", "passwordauthentication="))]})
        print(output.strip())
    elif args.action == "provision":
        provision()
    else:
        fixtures()


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print("FAIL:", type(error).__name__, "(details suppressed to protect server credentials)")
        raise SystemExit(1) from None
