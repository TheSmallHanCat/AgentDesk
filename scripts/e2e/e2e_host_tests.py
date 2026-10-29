"""Run Host tests on the dedicated Unix server without native model access."""
from __future__ import annotations

import io
import json
import shlex
import tarfile
import uuid

from e2e_common import ROOT, PRIVATE, REMOTE, configured, connect, run, save_result, upload


def main():
    if not configured():
        return 0
    identifier = uuid.uuid4().hex[:12]
    work = REMOTE + "/host-tests-" + identifier
    archive = PRIVATE / ("host-tests-" + identifier + ".tar.gz")
    with tarfile.open(archive, "w:gz") as package:
        for name in ("host/pyproject.toml", "host/uv.lock"):
            package.add(ROOT / name, arcname=name)
        for directory in ("host/agentdesk_host", "host/tests", "protocol"):
            for path in sorted((ROOT / directory).rglob("*")):
                if path.is_file() and "__pycache__" not in path.parts and path.suffix != ".pyc":
                    package.add(path, arcname=path.relative_to(ROOT).as_posix())
    result = {"runId": identifier, "nativeModelCalls": False}
    try:
        with connect() as client:
            run(client, "umask 077\nmkdir " + shlex.quote(work) + "\n")
            upload(client, archive, work + "/source.tar.gz")
            script = f"""
set -eu
umask 077
cd {shlex.quote(work)}
tar -xzf source.tar.gz
mkdir -p {shlex.quote(REMOTE + '/t')}
export TMPDIR={shlex.quote(REMOTE + '/t')}
export UV_NO_CONFIG=1 UV_NO_PROGRESS=1 UV_HTTP_TIMEOUT=45
export UV_CACHE_DIR="$HOME/.agentdesk/host/cache"
export UV_PYTHON_INSTALL_DIR="$HOME/.agentdesk/runtime"
export UV_DEFAULT_INDEX=https://pypi.org/simple
unset AGENTDESK_NATIVE_CONTRACT AGENTDESK_CODEX_PATH AGENTDESK_CLAUDE_PATH
"$HOME/.agentdesk/host/uv/uv" run --project host --frozen --extra claude --python "$HOME/.agentdesk/host/current/venv/bin/python" python -m pytest host/tests -q >result.txt 2>&1
"""
            code, _ = run(client, script, timeout=300, check=False)
            _, output = run(client, "tail -n 1 " + shlex.quote(work + "/result.txt") + "\n")
            # Only pytest's numeric summary is retained, never failure payloads.
            import re
            summary = re.search(r"(?:(?:\d+ (?:failed|passed|skipped|xfailed|xpassed|warnings?)),? ?)+in [\d.]+s", output)
            result.update(exitCode=code, summary=summary.group() if summary else "pytest_failed_summary_unavailable")
            print(json.dumps(result))
            return 0 if code == 0 else 1
    except Exception:
        result["failure"] = "bounded_linux_host_tests_failed_details_suppressed"
        return 1
    finally:
        save_result("task04-host-tests", result)


if __name__ == "__main__":
    raise SystemExit(main())
