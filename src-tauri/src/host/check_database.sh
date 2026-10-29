check_host_database() {
  if ! timeout 20 "$1" -m agentdesk_host --home "$HOME/.agentdesk" --check-database >/dev/null 2>&1; then
    printf '%s\n' 'E_DATABASE_SCHEMA: Host database is incompatible or cannot be checked' >&2
    return 78
  fi
}
