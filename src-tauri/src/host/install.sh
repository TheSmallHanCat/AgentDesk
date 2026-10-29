set -eu
umask 077
install_stage=platform
phase() {
  install_stage="$1"
  printf '\nAGENTDESK_STAGE:%s\n' "$install_stage"
}
on_exit() {
  status=$?
  if test "$status" -ne 0; then
    printf '\nAGENTDESK_FAILED:%s:%s\n' "$install_stage" "$status" >&2
    if command -v restore_activation >/dev/null 2>&1; then restore_activation; fi
  fi
}
trap on_exit EXIT
trap 'exit 130' HUP INT TERM
phase platform
test "$(uname -s)" = Linux && test "$(uname -m)" = x86_64
phase prepare.paths
base="$HOME/.agentdesk"
stage="$base/host/staging/$AGENTDESK_PLAN_ID"
version="$base/host/versions/$AGENTDESK_BUILD_ID"
mkdir -p "$base/host"
exec 9>"$base/host/install.lock"
flock -n 9 || exit 76
test -d "$stage" && test ! -e "$version"
managed_link() {
  if test -L "$base/host/$1"; then
    path=$(readlink "$base/host/$1")
    case "$path" in /*) ;; versions/*) path="$base/host/$path";; *) exit 73;; esac
    path=$(readlink -m -- "$path")
    case "$path" in "$base"/host/versions/*) printf '%s' "$path";; *) exit 73;; esac
  elif test -e "$base/host/$1"; then exit 73; fi
}
old_current=$(managed_link current)
old_previous=$(managed_link previous)
if test "${AGENTDESK_EXPECT_LINKS:-0}" = 1; then
  test "$old_current" = "$AGENTDESK_EXPECT_CURRENT" && test "$old_previous" = "$AGENTDESK_EXPECT_PREVIOUS" || exit 77
fi
printf '%s\n' "$old_current" >"$stage/original.current"
printf '%s\n' "$old_previous" >"$stage/original.previous"
printf '%s\n' 'Preparing verified uv runtime'
phase uv.download
fetch() {
  curl -q -fLsS --connect-timeout 15 --max-time 180 --retry 0 "$1" -o "$2"
  printf '%s  %s\n' "$3" "$2" | sha256sum -c -
}
fetch "$AGENTDESK_UV_URL" "$stage/uv.tar.gz" "$AGENTDESK_UV_SHA"
phase uv.extract
mkdir -p "$base/host/uv" "$base/runtime" "$base/host/versions"
tar -xOzf "$stage/uv.tar.gz" "$AGENTDESK_UV_MEMBER" >"$stage/uv"
chmod 700 "$stage/uv"
"$stage/uv" --version
if test ! -x "$base/host/uv/uv"; then cp "$stage/uv" "$base/host/uv/uv"; fi
unset UV_INDEX UV_INDEX_URL UV_EXTRA_INDEX_URL UV_DEFAULT_INDEX UV_FIND_LINKS UV_CONFIG_FILE PIP_INDEX_URL PIP_EXTRA_INDEX_URL PIP_FIND_LINKS
export UV_NO_CONFIG=1 UV_NO_PROGRESS=1 UV_HTTP_RETRIES=1 UV_HTTP_TIMEOUT=45
export UV_PYTHON_INSTALL_DIR="$base/runtime"
export UV_PYTHON_BIN_DIR="$base/runtime/bin"
export UV_CACHE_DIR="$base/host/cache"
export UV_PYTHON_PREFERENCE=only-managed
printf '%s\n' 'Installing independent Python and locked dependencies'
phase python.install
printf '%s  %s\n' "$AGENTDESK_PYTHON_METADATA_SHA" "$stage/python-downloads.json" | sha256sum -c -
timeout 180 "$stage/uv" python install 3.12.15 --python-downloads-json-url "$stage/python-downloads.json"
mkdir "$version"
activating=0
restore_activation() {
  if test "$activating" = 1; then
    current=$(managed_link current)
    if test "$current" = "$version" || test "$current" = "$old_current"; then
      if test -n "$old_current"; then
        ln -s "$old_current" "$base/host/current.restore.$AGENTDESK_PLAN_ID"
        mv -Tf "$base/host/current.restore.$AGENTDESK_PLAN_ID" "$base/host/current"
      elif test -L "$base/host/current"; then rm -- "$base/host/current"; fi
      if test -n "$old_previous"; then
        ln -s "$old_previous" "$base/host/previous.restore.$AGENTDESK_PLAN_ID"
        mv -Tf "$base/host/previous.restore.$AGENTDESK_PLAN_ID" "$base/host/previous"
      elif test -L "$base/host/previous"; then rm -- "$base/host/previous"; fi
    fi
  fi
}
phase venv.create
timeout 30 "$stage/uv" venv --python 3.12.15 "$version/venv"
printf '%s  %s\n' "$AGENTDESK_WHEEL_SHA" "$stage/$AGENTDESK_WHEEL" | sha256sum -c -
printf '%s  %s\n' "$AGENTDESK_DEPS_SHA" "$stage/dependencies.txt" | sha256sum -c -
phase dependencies.install
timeout 240 "$stage/uv" pip install --python "$version/venv/bin/python" --default-index https://pypi.org/simple --only-binary :all: --require-hashes -r "$stage/dependencies.txt"
phase host.install
"$stage/uv" pip install --python "$version/venv/bin/python" --default-index https://pypi.org/simple --no-deps "$stage/$AGENTDESK_WHEEL"
phase database.check
check_host_database "$version/venv/bin/python"
printf '%s\n' 'Installing official Codex and matching sandbox resources'
mkdir -p "$version/bin/codex-resources"
phase codex.download
fetch "$AGENTDESK_CODEX_URL" "$stage/codex.tar.gz" "$AGENTDESK_CODEX_SHA"
phase bwrap.download
fetch "$AGENTDESK_BWRAP_URL" "$stage/bwrap.tar.gz" "$AGENTDESK_BWRAP_SHA"
phase codex.extract
tar -xOzf "$stage/codex.tar.gz" "$AGENTDESK_CODEX_MEMBER" >"$version/bin/codex"
tar -xOzf "$stage/bwrap.tar.gz" "$AGENTDESK_BWRAP_MEMBER" >"$version/bin/codex-resources/bwrap"
chmod 700 "$version/bin/codex" "$version/bin/codex-resources/bwrap"
phase host.import
"$version/venv/bin/python" -c 'import agentdesk_host, claude_agent_sdk; print("Host and SDK imports verified")'
phase codex.version
"$version/bin/codex" --version
phase codex.sandbox
"$version/bin/codex" sandbox -- /bin/true
printf '%s\n' "$AGENTDESK_WHEEL_SHA" >"$version/wheel.sha256"
printf '%s\n' 'Waiting for installation commit'
phase prepare.wait
: >"$stage/prepared"
printf '\nAGENTDESK_PREPARED:%s\n' "$AGENTDESK_PLAN_ID"
i=0
while test ! -f "$stage/commit"; do
  test ! -f "$stage/cancel" || exit 130
  test "$i" -lt 1200 || exit 75
  sleep 0.1
  i=$((i+1))
done
test ! -f "$stage/cancel" || exit 130
test "$(managed_link current)" = "$old_current" && test "$(managed_link previous)" = "$old_previous" || exit 77
printf '%s\n' 'Committing verified installation'
phase activate.links
ln -s "$version" "$base/host/current.$AGENTDESK_PLAN_ID"
if test -n "$old_current"; then ln -s "$old_current" "$base/host/previous.$AGENTDESK_PLAN_ID"; fi
activating=1
mv -Tf "$base/host/current.$AGENTDESK_PLAN_ID" "$base/host/current"
if test -n "$old_current"; then mv -Tf "$base/host/previous.$AGENTDESK_PLAN_ID" "$base/host/previous"; fi
activating=0
trap - EXIT HUP INT TERM
printf '%s\n' 'Host installation activated'
