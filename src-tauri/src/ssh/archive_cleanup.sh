set -eu
marker=__MARKER__

# Only the unique, still-running archive group leader can authorize killing a
# process group. The cleanup shell receives this script through stdin and never
# has the marker in its own command line.
groups=$(ps -eww -o pid=,pgid=,args= | awk -v marker="$marker" '
    $1 == $2 && $3 == "/bin/sh" && $4 == "-c" && index($0, marker) { print $1 }
')

active() {
    ps -e -o pgid=,stat= | awk -v group="$1" '
        $1 == group && $2 !~ /^Z/ { found = 1 }
        END { exit !found }
    '
}

for group in $groups; do
    /bin/kill -TERM -- "-$group" 2>/dev/null || true
    attempt=0
    while active "$group" && test "$attempt" -lt 40; do
        sleep 0.1
        attempt=$((attempt + 1))
    done
    if active "$group"; then
        /bin/kill -KILL -- "-$group" 2>/dev/null || true
    fi
    attempt=0
    while active "$group" && test "$attempt" -lt 40; do
        sleep 0.1
        attempt=$((attempt + 1))
    done
    if active "$group"; then
        exit 1
    fi
done
