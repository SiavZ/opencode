#!/usr/bin/env bash
# Purpose: capture help fixtures for the whole baseline command tree by walking "Commands:" sections.
# Scope: developer tool, run manually. Calls capture.sh per command path.
# Uses: capture.sh in this directory.
# Invariants: names are command paths joined by "_" ("root" for the top level).
set -u
dir="$(cd "$(dirname "$0")" && pwd)"
walk() {
  local name="$1"
  shift
  "$dir/capture.sh" "$name" "$@" --help
  # Subcommand lines look like "  opencode mcp add  description".
  local depth=$(($# + 1))
  local subs
  subs=$(awk -v d="$depth" '/^Commands:/{c=1;next} c&&/^$/{c=0} c&&$1=="opencode"{print $(d+1)}' \
    "$dir/$name.stderr" "$dir/$name.stdout" | grep -v '^\[' | grep -v '^<' | sort -u)
  for sub in $subs; do
    [ "$sub" = "completion" ] && [ "$name" = "root" ] && continue
    [ -z "$sub" ] && continue
    local child="${name}_$sub"
    [ "$name" = "root" ] && child="$sub"
    walk "$child" "$@" "$sub"
  done
}
walk root
"$dir/capture.sh" completion completion
"$dir/capture.sh" err_unknown_flag --definitely-unknown
"$dir/capture.sh" err_unknown_command session bogus
"$dir/capture.sh" err_bad_choice --log-level LOUD
"$dir/capture.sh" err_missing_positional attach
"$dir/capture.sh" version --version
