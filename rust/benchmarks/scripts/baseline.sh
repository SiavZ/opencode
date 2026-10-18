#!/usr/bin/env bash
# Purpose: reproducible Phase 15 baseline run of the TypeScript product (or any command via
#          OPENCODE_BENCH_CMD), recording machine/toolchain metadata plus raw samples.
# Scope: Phase 15 measurement entrypoint. Does not install packages or touch real user data.
# Uses:
# rust/benchmarks/
# `-- scripts/baseline.py
# External:
# `-- bun, python3, sysctl, git, /usr/bin/time
# Used by:
# rust/contracts/
# `-- phase-15/baselines.md
# Invariants: HOME/XDG_* are mktemp dirs per sample (set in baseline.py), N >= 10 by default.
# Contract tests: rust/contracts/phase-15/baselines.md (recorded runs)
#
# Usage (from repo root or anywhere):
#   rust/benchmarks/scripts/baseline.sh [out.json]
#   BENCH_N=20 OPENCODE_BENCH_CMD="/path/to/rust/opencode" OPENCODE_BENCH_CWD=/tmp rust/benchmarks/scripts/baseline.sh
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
out="${1:-$(mktemp -t opencode-baseline).json}"
export BENCH_N="${BENCH_N:-10}"
export OPENCODE_BENCH_CMD="${OPENCODE_BENCH_CMD:-bun run src/index.ts}"
export OPENCODE_BENCH_CWD="${OPENCODE_BENCH_CWD:-$repo/packages/opencode}"

meta="$(mktemp)"
{
  echo "date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "commit=$(git -C "$repo" rev-parse HEAD)"
  echo "cpu=$(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -p)"
  echo "ncpu=$(sysctl -n hw.ncpu 2>/dev/null || nproc)"
  echo "memsize=$(sysctl -n hw.memsize 2>/dev/null || true)"
  echo "os=$(sw_vers -productVersion 2>/dev/null || uname -r)"
  echo "arch=$(uname -m)"
  echo "bun=$(bun --version 2>/dev/null || echo none)"
  echo "loadavg=$(sysctl -n vm.loadavg 2>/dev/null || cat /proc/loadavg)"
  echo "cmd=$OPENCODE_BENCH_CMD"
  echo "n=$BENCH_N"
} >"$meta"
cat "$meta" >&2

python3 "$here/baseline.py" >"$out.samples"
python3 - "$meta" "$out.samples" "$out" <<'EOF'
import json, sys
meta = dict(l.rstrip("\n").split("=", 1) for l in open(sys.argv[1]) if "=" in l)
data = json.load(open(sys.argv[2]))
data["meta"] = meta
json.dump(data, open(sys.argv[3], "w"), indent=2)
EOF
rm -f "$meta" "$out.samples"
echo "wrote $out" >&2
