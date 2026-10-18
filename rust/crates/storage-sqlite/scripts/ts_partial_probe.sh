#!/bin/sh
# Purpose: debug helper. Builds partially migrated databases with the real TS migration code under an
# isolated temp HOME/XDG and prints their tables and journal counts.
# Scope: manual tool, never touches real user data. Usage: scripts/ts_partial_probe.sh <tmpdir> <count>...
set -eu
dir="$1"
shift
mkdir -p "$dir"
cd "$(dirname "$0")/../../../../packages/core"
for n in "$@"; do
  env HOME="$dir" XDG_DATA_HOME="$dir/d" XDG_CONFIG_HOME="$dir/c" XDG_STATE_HOME="$dir/s" XDG_CACHE_HOME="$dir/k" \
    bun ../../rust/crates/storage-sqlite/tests/ts/create_db.ts partial "$dir/p$n.db" "$n"
  echo "== $n"
  sqlite3 "$dir/p$n.db" "select group_concat(name, ' ') from sqlite_master where type='table'; select count(*) from migration;"
done
