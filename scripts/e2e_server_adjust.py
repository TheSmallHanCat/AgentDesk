"""Authorized task-03 server changes, with backups and endpoint-free results."""

from __future__ import annotations

import json
import shlex
import sys
import time
from pathlib import Path

from e2e_common import PRIVATE, REMOTE, ROOT, configured, connect, run, save_result, upload


def main() -> int:
    if not configured():
        return 0
    stamp = time.strftime("%Y%m%d-%H%M%S", time.gmtime())
    result: dict = {"stamp": stamp, "loginAttempted": False, "grubChanged": False, "rebooted": False}
    try:
        with connect(root=True) as root:
            _, output = run(root, """
set -eu
printf 'namespaceLimit='; sysctl -n user.max_user_namespaces
printf 'namespaceBootFlags='; tr ' ' '\\n' </proc/cmdline | grep -E '^(user_namespace.enable|namespace.unpriv_enable)(=|$)' || true
""")
            result["before"] = dict(line.split("=", 1) for line in output.splitlines() if "=" in line)
            old_limit = int(result["before"]["namespaceLimit"])
            userns = "/etc/sysctl.d/90-agentdesk-userns.conf"
            backup = userns + ".agentdesk-bak-" + stamp
            _, existed = run(root, f"set -eu\ntest ! -L {userns}\nif test -e {userns}; then cp -p -- {userns} {backup}; printf yes; else printf no; fi\n")
            result["userns"] = {"previousLimit": old_limit, "backup": backup if existed == "yes" else None}
            run(root, f"set -eu\nprintf '%s\\n' 'user.max_user_namespaces=15000' >{userns}\nchmod 644 {userns}\nsysctl -p {userns} >/dev/null\ntest \"$(sysctl -n user.max_user_namespaces)\" = 15000\n")
            result["userns"]["applied"] = True
            result["userns"]["rollback"] = ((f"cp -p -- {backup} {userns}" if existed == "yes" else f"rm -- {userns}") + f"; sysctl -w user.max_user_namespaces={old_limit}")
            print("PASS: namespace sysctl backed up and applied", flush=True)

            config = "/etc/ssh/sshd_config"
            ssh_backup = config + ".agentdesk-bak-" + stamp
            result["sshd"] = {"backup": ssh_backup, "rollback": f"cp -p -- {ssh_backup} {config}; sshd -t && systemctl reload sshd"}
            # Preserve inode/mode/owner; modify only PubkeyAuthentication. Prepend
            # a global setting when the original file has no active directive.
            script = f"""
set -eu
test ! -L {config}
test ! -e {ssh_backup}
cp -p -- {config} {ssh_backup}
tmp=$(mktemp /etc/ssh/.agentdesk-sshd.XXXXXX)
trap 'rm -f -- "$tmp"' EXIT
awk 'BEGIN {{ print "PubkeyAuthentication yes" }} /^[[:space:]]*PubkeyAuthentication[[:space:]]/ {{ sub(/PubkeyAuthentication[[:space:]]+[^[:space:]]+/, "PubkeyAuthentication yes") }} {{ print }}' {ssh_backup} >"$tmp"
cat "$tmp" >{config}
if ! sshd -t; then cp -p -- {ssh_backup} {config}; exit 71; fi
if ! systemctl reload sshd; then cp -p -- {ssh_backup} {config}; sshd -t && systemctl reload sshd; exit 72; fi
"""
            try:
                run(root, script)
                # This MUST be a fresh password connection, immediately after reload.
                with connect(root=True) as fresh:
                    run(fresh, "test \"$(id -u)\" = 0\n")
                result["sshd"]["freshRootPasswordConnection"] = True
            except Exception:
                run(root, result["sshd"]["rollback"] + "\n")
                result["sshd"]["rolledBack"] = True
                raise
            _, output = run(root, "sshd -T 2>/dev/null | awk '$1 == \"pubkeyauthentication\" || $1 == \"passwordauthentication\" {print $1 \"=\" $2}'\n")
            policy = dict(line.split("=", 1) for line in output.splitlines())
            assert policy == {"pubkeyauthentication": "yes", "passwordauthentication": "yes"}
            save_result("auth-server-policy", policy)
            result["sshd"]["effectivePolicy"] = policy
            print("PASS: sshd validated and reloaded; fresh root password connection passed", flush=True)

        with connect(password=True) as ordinary:
            _, output = run(ordinary, """
set -eu
test "$(id -un)" = agentdesk-test
base=/home/agentdesk-test/.agentdesk/spikes/9397564fc3e3
test -x "$base/venv/bin/python" && test -x "$base/codex"
printf '%s' "$base"
""")
            base = output.strip()
            work = REMOTE + "/task03-" + stamp
            run(ordinary, "umask 077\nmkdir " + shlex.quote(work) + "\n")
            upload(ordinary, ROOT / "spikes/agent_probe.py", work + "/agent_probe.py")
            _, output = run(ordinary, shlex.quote(base + "/venv/bin/python") + " " + shlex.quote(work + "/agent_probe.py") + " " + shlex.quote(base + "/codex") + "\n", timeout=150)
            probe = json.loads(output)
            result["agents"] = probe
            sandbox = probe.get("codex", {}).get("sandboxLocalProcess", {})
            if sandbox.get("exitCode") == 0:
                upload(ordinary, ROOT / "spikes/sandbox_socket_probe.py", work + "/sandbox_socket_probe.py")
                _, output = run(ordinary, shlex.join([base + "/venv/bin/python", work + "/sandbox_socket_probe.py", base + "/codex", base + "/venv/bin/python"]) + "\n", timeout=60)
                result["permissionMatrix"] = json.loads(output)
            else:
                result["permissionMatrix"] = {"status": "skipped", "reason": "namespace_initialization_still_fails_no_grub_or_reboot_authorized"}
            print("PASS: ordinary-user CLI probes completed; sandbox exit=" + str(sandbox.get("exitCode")), flush=True)
        return 0
    except Exception:
        result["failure"] = "task03_server_adjustment_failed_details_suppressed"
        print("FAIL: task03 server adjustment; inspect completed structured stages", flush=True)
        return 1
    finally:
        save_result("task03-server", result)
        print(json.dumps(result, ensure_ascii=True, indent=2))


if __name__ == "__main__":
    sys.exit(main())
