set -eu

# Do not start tar until the client has registered this process group. A close
# before authorization makes read fail, so an early cancellation cannot race a
# late archive process into existence.
printf 'AGENTDESK_ARCHIVE_READY:%s\n' "$0" >&2
IFS= read -r authorization
test "$authorization" = "$0" || exit 125

# Keep this group leader alive until tar and gzip finish. Archive bytes go
# directly to stdout; no temporary remote archive or size header exists.
tar -czf - -C "$1" -- "$2"
