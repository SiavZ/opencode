#!/usr/bin/env bash
# Purpose: capture golden help fixtures from the TypeScript baseline CLI.
# Scope: developer tool, run manually. Writes <name>.stdout/.stderr/.code beside this script.
# Uses: packages/opencode/src/index.ts through bun, with an isolated temporary HOME/XDG tree.
# Invariants: never touches the real user data directories.
# Usage: rust/bins/opencode/tests/fixtures/help/capture.sh "<name>" <args...>
set -u
out_dir="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$out_dir/../../../../../.." && pwd)"
name="$1"
shift
sandbox="${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/opencode-p09-help-home"
mkdir -p "$sandbox/d" "$sandbox/c" "$sandbox/k" "$sandbox/s"
cd "$repo/packages/opencode" || exit 1
env HOME="$sandbox" XDG_DATA_HOME="$sandbox/d" XDG_CONFIG_HOME="$sandbox/c" \
  XDG_CACHE_HOME="$sandbox/k" XDG_STATE_HOME="$sandbox/s" COLUMNS=100 NO_COLOR=1 \
  bun run ./src/index.ts "$@" >"$out_dir/$name.stdout" 2>"$out_dir/$name.stderr" </dev/null
echo $? >"$out_dir/$name.code"
