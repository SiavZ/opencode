#!/usr/bin/env bash
# Purpose: capture golden help fixtures from the v2 TypeScript CLI (packages/cli, effect/unstable/cli).
# Scope: developer tool, run manually. Writes <name>.stdout/.stderr/.code beside this script.
# Uses: packages/cli/src/index.ts through bun, with an isolated temporary HOME/XDG tree.
# Invariants: never touches the real user data directories. Only help/error paths are captured.
# Usage: rust/bins/opencode/tests/fixtures/help/v2/capture.sh (captures the whole v2 tree)
set -u
out_dir="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$out_dir/../../../../../../.." && pwd)"
sandbox="${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/opencode-p09-help-home-v2"
mkdir -p "$sandbox/d" "$sandbox/c" "$sandbox/k" "$sandbox/s"
cap() {
  local name="$1"
  shift
  (cd "$repo/packages/cli" && env HOME="$sandbox" XDG_DATA_HOME="$sandbox/d" XDG_CONFIG_HOME="$sandbox/c" \
    XDG_CACHE_HOME="$sandbox/k" XDG_STATE_HOME="$sandbox/s" COLUMNS=100 NO_COLOR=1 \
    bun run ./src/index.ts "$@" >"$out_dir/$name.stdout" 2>"$out_dir/$name.stderr" </dev/null)
  echo $? >"$out_dir/$name.code"
}
cap root --help
cap api api --help
cap debug debug --help
cap debug_agents debug agents --help
cap migrate migrate --help
cap service service --help
cap service_start service start --help
cap service_restart service restart --help
cap service_status service status --help
cap service_stop service stop --help
cap service_password service password --help
cap serve serve --help
cap err_unknown_command bogus
cap err_unknown_flag serve --bogus
cap version --version
cap err_service_no_sub service
cap err_api_missing api
cap err_bad_integer serve --port abc
cap help_short service password -h
