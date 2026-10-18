#!/usr/bin/env python3
# Purpose: measure TypeScript (or any OPENCODE command) baseline startup, serve readiness, peak RSS
#          and SSE event latency/throughput. Writes raw samples as JSON. Does not touch real user data.
# Scope: Phase 15 baseline measurement script, stdlib only, invoked by baseline.sh.
# Uses:
# rust/benchmarks/
# `-- scripts/baseline.sh
# External:
# `-- python3 stdlib (subprocess, http.client, socket, statistics)
# Used by:
# rust/benchmarks/
# `-- scripts/baseline.sh
# Invariants: every run gets HOME/XDG_* inside a mktemp dir; cold runs use a fresh dir per sample,
#             warm runs reuse one primed dir; no provider credentials are read or needed.
# Contract tests: rust/contracts/phase-15/baselines.md (recorded runs)
import http.client
import json
import os
import shlex
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time


def temp_env(root):
    env = dict(os.environ)
    for k in list(env):
        if k.startswith("OPENCODE_") and k != "OPENCODE_BENCH_BIN":
            del env[k]
    env.update(
        HOME=root,
        XDG_DATA_HOME=f"{root}/data",
        XDG_CONFIG_HOME=f"{root}/config",
        XDG_CACHE_HOME=f"{root}/cache",
        XDG_STATE_HOME=f"{root}/state",
    )
    return env


def pct(samples, p):
    s = sorted(samples)
    if not s:
        return None
    k = (len(s) - 1) * p / 100
    lo, hi = int(k), min(int(k) + 1, len(s) - 1)
    return s[lo] + (s[hi] - s[lo]) * (k - lo)


def summary(samples):
    return {
        "n": len(samples),
        "p50": pct(samples, 50),
        "p95": pct(samples, 95),
        "p99": pct(samples, 99),
        "min": min(samples),
        "max": max(samples),
        "mean": statistics.fmean(samples),
        "stdev": statistics.pstdev(samples),
    }


def run_cli(cmd, args, env):
    t = time.perf_counter()
    r = subprocess.run(cmd + args, env=env, capture_output=True, cwd=CWD)
    dt = (time.perf_counter() - t) * 1000
    if r.returncode != 0:
        raise SystemExit(f"cli failed: {cmd + args}: {r.stderr.decode()[-400:]}")
    return dt


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def request(port, method, path, body=None, timeout=5):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    headers = {"content-type": "application/json"} if body is not None else {}
    c.request(method, path, body=json.dumps(body) if body is not None else None, headers=headers)
    r = c.getresponse()
    data = r.read()
    c.close()
    return r.status, data


def start_serve(cmd, env, port, rss=False):
    argv = cmd + ["serve", "--port", str(port), "--pure"]
    if rss:
        argv = ["/usr/bin/time", "-l"] + argv
    t = time.perf_counter()
    p = subprocess.Popen(argv, env=env, cwd=CWD, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, start_new_session=True)
    deadline = t + 60
    while time.perf_counter() < deadline:
        if p.poll() is not None:
            raise SystemExit(f"serve exited early: {p.stderr.read().decode()[-400:]}")
        try:
            status, _ = request(port, "GET", "/global/health", timeout=1)
            if status == 200:
                return p, (time.perf_counter() - t) * 1000
        except OSError:
            pass
        time.sleep(0.01)
    stop(p)
    raise SystemExit("serve did not become healthy in 60s")


def stop(p):
    # Under /usr/bin/time, signal only time's children so time(1) survives to print rusage.
    kids = subprocess.run(["pgrep", "-P", str(p.pid)], capture_output=True, text=True).stdout.split()
    timed = os.path.basename(p.args[0]) == "time"
    for pid in kids if timed and kids else [str(p.pid)]:
        try:
            os.kill(int(pid), 15)
        except ProcessLookupError:
            pass
    try:
        _, err = p.communicate(timeout=15)
    except subprocess.TimeoutExpired:
        os.killpg(p.pid, 9)
        _, err = p.communicate()
    return err.decode(errors="replace") if err else ""


def peak_rss_bytes(time_stderr):
    for line in time_stderr.splitlines():
        if "maximum resident set size" in line:
            return int(line.split()[0])
    return None


class Sse:
    """Minimal SSE reader on a raw socket recording arrival time per event type."""

    def __init__(self, port, path):
        self.events = []
        self.lock = threading.Lock()
        self.sock = socket.create_connection(("127.0.0.1", port))
        self.sock.sendall(f"GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n\r\n".encode())
        self.thread = threading.Thread(target=self.read, daemon=True)
        self.thread.start()

    def read(self):
        buf = b""
        while True:
            try:
                chunk = self.sock.recv(65536)
            except OSError:
                return
            if not chunk:
                return
            now = time.perf_counter()
            buf += chunk
            while b"\n\n" in buf:
                block, buf = buf.split(b"\n\n", 1)
                for line in block.split(b"\n"):
                    if line.startswith(b"data: "):
                        try:
                            ev = json.loads(line[6:])
                        except ValueError:
                            continue
                        with self.lock:
                            self.events.append((now, ev))

    def wait_for(self, pred, timeout=10):
        deadline = time.perf_counter() + timeout
        while time.perf_counter() < deadline:
            with self.lock:
                for t, ev in self.events:
                    if pred(ev):
                        return t
            time.sleep(0.001)
        return None

    def close(self):
        self.sock.close()


def sse_workload(port, workdir, latency_n, burst_n):
    q = f"?directory={workdir}"
    sse = Sse(port, f"/event{q}")
    if sse.wait_for(lambda e: e.get("type") == "server.connected") is None:
        raise SystemExit("no server.connected event")
    lat = []
    for _ in range(latency_n):
        t = time.perf_counter()
        status, body = request(port, "POST", f"/session{q}", {})
        assert status == 200, body
        sid = json.loads(body)["id"]
        arrived = sse.wait_for(lambda e, sid=sid: e.get("type") == "session.created" and e["properties"]["sessionID"] == sid)
        if arrived is None:
            raise SystemExit("session.created not observed")
        lat.append((arrived - t) * 1000)
    with sse.lock:
        before = len(sse.events)
    t = time.perf_counter()
    ids = set()
    for _ in range(burst_n):
        status, body = request(port, "POST", f"/session{q}", {})
        ids.add(json.loads(body)["id"])

    def all_seen():
        with sse.lock:
            seen = {e["properties"]["sessionID"] for _, e in sse.events[before:] if e.get("type") == "session.created"}
        return ids <= seen

    deadline = time.perf_counter() + 30
    while not all_seen() and time.perf_counter() < deadline:
        time.sleep(0.002)
    dt = time.perf_counter() - t
    with sse.lock:
        total = len(sse.events) - before
    sse.close()
    return lat, {"sessions": burst_n, "events_received": total, "seconds": dt, "events_per_sec": total / dt, "complete": all_seen()}


def main():
    global CWD
    cmd = shlex.split(os.environ.get("OPENCODE_BENCH_CMD", "bun run src/index.ts"))
    CWD = os.environ.get("OPENCODE_BENCH_CWD", os.getcwd())
    n = int(os.environ.get("BENCH_N", "10"))
    out = {"cmd": cmd, "cwd_basename": os.path.basename(CWD), "n": n, "results": {}}
    res = out["results"]

    for name, args in (("cli_version", ["--version"]), ("cli_help", ["--help"])):
        cold = []
        for _ in range(n):
            with tempfile.TemporaryDirectory() as root:
                cold.append(run_cli(cmd, args, temp_env(root)))
        with tempfile.TemporaryDirectory() as root:
            env = temp_env(root)
            run_cli(cmd, args, env)  # prime transpiler/data caches
            warm = [run_cli(cmd, args, env) for _ in range(n)]
        res[f"{name}_cold_ms"] = summary(cold)
        res[f"{name}_warm_ms"] = summary(warm)
        print(name, "cold p50/p95", res[f"{name}_cold_ms"]["p50"], res[f"{name}_cold_ms"]["p95"], "warm", res[f"{name}_warm_ms"]["p50"], res[f"{name}_warm_ms"]["p95"], file=sys.stderr)

    for label, fresh in (("cold", True), ("warm", False)):
        samples = []
        with tempfile.TemporaryDirectory() as shared:
            if not fresh:
                p, _ = start_serve(cmd, temp_env(shared), free_port())
                stop(p)
            for _ in range(n):
                with tempfile.TemporaryDirectory() as own:
                    p, dt = start_serve(cmd, temp_env(own if fresh else shared), free_port())
                    stop(p)
                    samples.append(dt)
        res[f"serve_ready_{label}_ms"] = summary(samples)
        print("serve", label, res[f"serve_ready_{label}_ms"]["p50"], res[f"serve_ready_{label}_ms"]["p95"], file=sys.stderr)

    idle, loaded, lat_all, thr = [], [], [], []
    for _ in range(n):
        with tempfile.TemporaryDirectory() as root, tempfile.TemporaryDirectory() as work:
            subprocess.run(["git", "init", "-q", work], check=True)
            env = temp_env(root)
            p, _ = start_serve(cmd, env, port := free_port(), rss=True)
            status, _ = request(port, "GET", "/global/health")
            idle.append(peak_rss_bytes(stop(p)))
            p, _ = start_serve(cmd, env, port := free_port(), rss=True)
            lat, t = sse_workload(port, work, 20, 100)
            loaded.append(peak_rss_bytes(stop(p)))
            lat_all += lat
            thr.append(t)
    res["serve_peak_rss_idle_bytes"] = summary(idle)
    res["serve_peak_rss_after_workload_bytes"] = summary(loaded)
    res["sse_session_created_latency_ms"] = summary(lat_all)
    res["sse_burst_events_per_sec"] = summary([t["events_per_sec"] for t in thr])
    res["sse_burst_complete"] = all(t["complete"] for t in thr)
    res["sse_burst_runs"] = thr
    json.dump(out, sys.stdout, indent=2)
    print()


if __name__ == "__main__":
    main()
