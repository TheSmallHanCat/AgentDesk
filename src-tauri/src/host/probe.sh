set -eu
printf 'system='; uname -s
printf 'architecture='; uname -m
printf 'glibc='; getconf GNU_LIBC_VERSION 2>/dev/null || printf unknown
printf '\n'
for tool in uv node npm claude codex; do
  if command -v "$tool" >/dev/null 2>&1; then printf '%s=present\n' "$tool"; else printf '%s=missing\n' "$tool"; fi
done
if test -x "$HOME/.agentdesk/host/uv/uv"; then printf 'uv=managed\n'; fi
if test -x "$HOME/.agentdesk/host/current/venv/bin/python"; then
  printf 'python=managed\n'
  "$HOME/.agentdesk/host/current/venv/bin/python" -c 'import agentdesk_host; print("host="+agentdesk_host.__version__)'
  if "$HOME/.agentdesk/host/current/venv/bin/python" -c 'import claude_agent_sdk' >/dev/null 2>&1; then printf 'sdk=ready\n'; else printf 'sdk=missing\n'; fi
else
  printf 'python=missing\nhost=missing\nsdk=missing\n'
fi
if test -x "$HOME/.agentdesk/host/current/bin/codex-resources/bwrap"; then printf 'bwrap=ready\n'; else printf 'bwrap=missing\n'; fi
if timeout 5 systemctl --user show-environment >/dev/null 2>&1; then printf 'systemd=ready\n'; else printf 'systemd=unsupported\n'; fi
if test -S "$HOME/.agentdesk/run/host.sock"; then printf 'socket=present\n'; else printf 'socket=missing\n'; fi
