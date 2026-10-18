#!/usr/bin/env python3
# Purpose: capture HTTP compatibility fixtures from the real TypeScript server (packages/opencode
#   `serve`) for every operation in packages/sdk/openapi.json plus negative cases (auth, schema,
#   404, CORS, SSE first frames, /doc). Writes one JSON file per route family. Does not edit the
#   TypeScript product.
# Scope: developer tool, run by hand. Python 3 stdlib only.
# Uses:
# packages/
# |-- opencode/src/index.ts (serve command, started with `bun run` from packages/opencode)
# `-- sdk/openapi.json (operation list)
# External:
# `-- bun
# Used by:
# rust/crates/
# `-- server/tests/replay_baseline.rs (reads the written fixtures)
# Invariants: every server runs with HOME and XDG_* inside a fresh temp root, no provider
#   credentials, HTTP(S)_PROXY pointed at a dead local port so nothing reaches the network, and
#   the cwd is packages/opencode. Every request carries x-opencode-directory pointing at a temp git
#   project, so the real repo and ~/.local/share/opencode are never touched. Output is sanitized:
#   temp roots -> {TMP}, project -> {PROJ}, repo cwd -> {CWD}, ids/timestamps/refs -> placeholders
#   (rules in fixtures/README.md). Credentials are never written: requests record an `auth` marker.
# Contract tests: rust/crates/server/tests/replay_baseline.rs
#
# Usage (from repo root or anywhere):
#   python3 rust/contracts/phase-06/capture/capture.py
import base64
import hashlib
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.parse
import http.client

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
PKG = os.path.join(REPO, "packages", "opencode")
OUT = os.path.join(REPO, "rust", "crates", "server", "tests", "fixtures", "baseline")
OPENAPI = os.path.join(REPO, "packages", "sdk", "openapi.json")
PASSWORD = "fixture-password"  # not a secret: never written to fixtures, replay uses the marker
USERNAME = "opencode"
BODY_INLINE_LIMIT = 48 * 1024
METHODS = ("get", "post", "put", "patch", "delete")

ID_PREFIXES = "ses|msg|per|que|prt|pty|evt|wrk|job|tool|prj|cpy|att|cred|int|tkt|ref|req"
ID_RE = re.compile(r"\b(" + ID_PREFIXES + r")_[0-9A-Za-z]{26}\b")
UUID_RE = re.compile(r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
SHA_RE = re.compile(r"\b[0-9a-f]{40}\b")
ERR_RE = re.compile(r"\berr_[0-9a-f]{8}\b")
ISO_RE = re.compile(r"\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z\b")
# Random human-readable values: legacy session slugs, generated copy names, PTY tickets.
RANDOM_KEYS = {"slug": "{SLUG}", "ticket": "{TICKET}"}


class _Seq:
    """Global capture order: replay runs cases in this order because later cases reuse ids."""

    def __init__(self):
        self.n = 0

    def next(self):
        self.n += 1
        return self.n


SEQ = _Seq()


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


class Server:
    def __init__(self, name, password):
        self.name = name
        self.root = os.path.realpath(tempfile.mkdtemp(prefix=f"p06fx-{name}-"))
        for d in ("home", "data", "config", "state", "cache", "proj"):
            os.makedirs(os.path.join(self.root, d))
        self.proj = os.path.join(self.root, "proj")
        git = ["git", "-c", "user.email=fixture@example.com", "-c", "user.name=fixture"]
        subprocess.run(["git", "init", "-q", "-b", "main", self.proj], check=True)
        with open(os.path.join(self.proj, "README.md"), "w") as f:
            f.write("# fixture project\n\nhello fixture\n")
        os.makedirs(os.path.join(self.proj, "src"))
        with open(os.path.join(self.proj, "src", "main.ts"), "w") as f:
            f.write("export function hello() {\n  return 'fixture'\n}\n")
        subprocess.run(git + ["-C", self.proj, "add", "."], check=True)
        subprocess.run(git + ["-C", self.proj, "commit", "-q", "-m", "init"], check=True)
        with open(os.path.join(self.proj, "README.md"), "a") as f:
            f.write("modified\n")
        self.port = free_port()
        dead = self.dead = free_port()
        env = {
            "PATH": os.environ["PATH"],
            "HOME": os.path.join(self.root, "home"),
            "XDG_DATA_HOME": os.path.join(self.root, "data"),
            "XDG_CONFIG_HOME": os.path.join(self.root, "config"),
            "XDG_STATE_HOME": os.path.join(self.root, "state"),
            "XDG_CACHE_HOME": os.path.join(self.root, "cache"),
            "TMPDIR": os.path.join(self.root, "cache"),
            "OPENCODE_DISABLE_MODELS_FETCH": "1",
            "OPENCODE_DISABLE_AUTOUPDATE": "1",
            "OPENCODE_DISABLE_LSP_DOWNLOAD": "1",
            "HTTP_PROXY": f"http://127.0.0.1:{dead}",
            "HTTPS_PROXY": f"http://127.0.0.1:{dead}",
            "http_proxy": f"http://127.0.0.1:{dead}",
            "https_proxy": f"http://127.0.0.1:{dead}",
            "NO_PROXY": "127.0.0.1,localhost",
            "no_proxy": "127.0.0.1,localhost",
            "TERM": "dumb",
            "SHELL": "/bin/sh",
        }
        # Offline fixture provider so noReply prompts can resolve a default model. The base URL
        # is a dead local port and the key is a literal placeholder, not a credential.
        env["OPENCODE_CONFIG_CONTENT"] = json.dumps({
            "share": "disabled",
            "autoupdate": False,
            "model": "fixture/fixture-model",
            "provider": {"fixture": {
                "name": "Fixture", "npm": "@ai-sdk/openai-compatible",
                "options": {"baseURL": f"http://127.0.0.1:{dead}/v1", "apiKey": "fixture-placeholder"},
                "models": {"fixture-model": {"name": "Fixture Model"}},
            }},
        })
        if password:
            env["OPENCODE_SERVER_PASSWORD"] = password
        self.log = open(os.path.join(self.root, "server.log"), "w")
        self.proc = subprocess.Popen(
            ["bun", "run", "./src/index.ts", "serve", "--port", str(self.port), "--hostname", "127.0.0.1"],
            cwd=PKG, env=env, stdout=self.log, stderr=subprocess.STDOUT,
        )
        for _ in range(120):
            time.sleep(0.5)
            try:
                c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=2)
                c.request("GET", "/global/health", headers=self.auth_headers("valid"))
                if c.getresponse().status in (200, 401):
                    return
            except OSError:
                pass
        raise RuntimeError(f"server {name} did not start; see {self.root}/server.log")

    def auth_headers(self, auth):
        if auth == "valid" and self.name == "pass":
            return {"Authorization": "Basic " + base64.b64encode(f"{USERNAME}:{PASSWORD}".encode()).decode()}
        if auth == "invalid":
            return {"Authorization": "Basic " + base64.b64encode(b"opencode:wrong").decode()}
        return {}

    def stop(self):
        self.proc.terminate()
        try:
            self.proc.wait(10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        self.log.close()


class Normalizer:
    """Per-family placeholder numbering, shared by request and response so replay can map ids."""

    def __init__(self, servers):
        self.ids = {}
        self.counts = {}
        self.paths = []
        for s in servers:
            self.paths.append((s.proj, "{PROJ}"))
            self.paths.append((s.root, "{TMP}"))
        self.paths.append((os.path.realpath(PKG), "{CWD}"))
        self.paths.append((os.path.realpath(REPO), "{REPO}"))
        home = os.path.realpath(os.path.expanduser("~"))
        self.paths.append((home, "{USER_HOME}"))
        self.paths.sort(key=lambda p: -len(p[0]))
        self.ports = [(f"127.0.0.1:{s.dead}", "127.0.0.1:{DEAD_PORT}") for s in servers]

    def id(self, raw, prefix):
        if raw not in self.ids:
            self.counts[prefix] = self.counts.get(prefix, 0) + 1
            self.ids[raw] = "{" + prefix + "_" + str(self.counts[prefix]) + "}"
        return self.ids[raw]

    def text(self, s):
        for p, r in self.paths:
            s = s.replace(p, r)
            if p.startswith("/private/"):
                s = s.replace(p[len("/private"):], r)
            s = s.replace(urllib.parse.quote(p, safe=""), urllib.parse.quote(r, safe=""))
        for p, r in self.ports:
            s = s.replace(p, r)
        s = ID_RE.sub(lambda m: self.id(m.group(0), m.group(1)), s)
        s = UUID_RE.sub("{UUID}", s)
        s = SHA_RE.sub("{SHA}", s)
        s = ERR_RE.sub("{ERR_REF}", s)
        s = ISO_RE.sub("{ISO}", s)
        return s

    def value(self, v, key=None):
        if isinstance(v, dict):
            return {self.text(k): self.value(x, k) for k, x in v.items()}
        if isinstance(v, list):
            return [self.value(x, key) for x in v]
        if isinstance(v, str):
            if key in RANDOM_KEYS:
                return RANDOM_KEYS[key]
            if key in ("previous", "next") and v.startswith("ey"):
                # Opaque cursors are base64url JSON: keep the decoded structure, normalized.
                try:
                    decoded = json.loads(base64.urlsafe_b64decode(v + "=" * (-len(v) % 4)))
                    return "{CURSOR:" + json.dumps(self.value(decoded), separators=(",", ":")) + "}"
                except ValueError:
                    pass
            return self.text(v)
        if isinstance(v, (int, float)) and not isinstance(v, bool):
            if 1_500_000_000_000 <= v <= 2_500_000_000_000:
                return "{TS}"
            if key in ("pid",):
                return "{PID}"
        return v


def request(server, method, path, query=None, headers=None, body=None, raw_body=None, auth="valid", timeout=15, sse=False):
    url = path
    if query:
        url += "?" + urllib.parse.urlencode(query)
    h = {"x-opencode-directory": server.proj}
    h.update(server.auth_headers(auth))
    if headers:
        h.update(headers)
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        h.setdefault("content-type", "application/json")
    if raw_body is not None:
        data = raw_body.encode()
    if auth == "query-valid" and server.name == "pass":
        token = base64.b64encode(f"{USERNAME}:{PASSWORD}".encode()).decode()
        url += ("&" if "?" in url else "?") + "auth_token=" + urllib.parse.quote(token)
    c = http.client.HTTPConnection("127.0.0.1", server.port, timeout=timeout)
    try:
        c.request(method, url, body=data, headers=h)
        r = c.getresponse()
        headers_out = {k.lower(): v for k, v in r.getheaders()}
        if "text/event-stream" in headers_out.get("content-type", ""):
            c.sock.settimeout(2.5)
            chunks = b""
            start = time.time()
            try:
                while time.time() - start < 2.5 and len(chunks) < 16384:
                    b = r.read1(4096) if hasattr(r, "read1") else r.read(1)
                    if not b:
                        break
                    chunks += b
            except (socket.timeout, TimeoutError, OSError):
                pass
            return r.status, headers_out, chunks, "sse"
        return r.status, headers_out, r.read(), None
    except (socket.timeout, TimeoutError) as e:
        return None, {}, str(e).encode(), "timeout"
    finally:
        c.close()


KEEP_HEADERS = (
    "content-type", "www-authenticate", "access-control-allow-origin", "access-control-allow-methods",
    "access-control-allow-headers", "access-control-allow-credentials", "access-control-max-age",
    "access-control-expose-headers", "vary", "content-encoding", "cache-control", "location",
    "content-security-policy", "x-opencode-directory", "x-opencode-workspace",
)


def record(cases, norm, server, case_id, op, method, path, query=None, headers=None, body=None, raw_body=None, auth="valid", note=None, sse=False, timeout=15):
    status, rh, raw, kind = request(server, method, path, query, headers, body, raw_body, auth, timeout, sse)
    resp = {"status": status, "headers": {k: norm.text(v) for k, v in rh.items() if k in KEEP_HEADERS}}
    parsed = None
    if kind == "timeout":
        resp["kind"] = "timeout"
    elif kind == "sse":
        resp["kind"] = "sse"
        frames = [f for f in raw.decode("utf-8", "replace").split("\n\n") if f.strip()]
        out = []
        for f in frames[:5]:
            lines = {}
            for line in f.split("\n"):
                if ":" in line:
                    k, v = line.split(":", 1)
                    lines.setdefault(k, []).append(v[1:] if v.startswith(" ") else v)
            data = "\n".join(lines.get("data", []))
            try:
                data_v = norm.value(json.loads(data))
            except ValueError:
                data_v = norm.text(data)
            shape = "\n".join("data: {DATA}" if l.startswith("data:") else norm.text(l) for l in f.split("\n"))
            out.append({"shape": shape, "fields": sorted(lines.keys()), "data": data_v})
        resp["frames"] = out
    elif not raw:
        resp["kind"] = "empty"
    else:
        try:
            parsed = json.loads(raw)
            resp["kind"] = "json"
        except ValueError:
            resp["kind"] = "text"
        if len(raw) > BODY_INLINE_LIMIT:
            resp["kind"] = resp["kind"] + "-digest"
            resp["digest"] = {"bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}
            if isinstance(parsed, dict):
                resp["digest"]["keys"] = list(parsed.keys())
                if "paths" in parsed:
                    resp["digest"]["paths"] = sorted(parsed["paths"].keys())
            elif isinstance(parsed, list):
                resp["digest"]["length"] = len(parsed)
        elif parsed is not None:
            resp["body"] = norm.value(parsed)
            if op == "experimental.projectCopy.generateName" and isinstance(resp["body"], dict) and "name" in resp["body"]:
                resp["body"]["name"] = "{NAME}"  # random adjective-noun
        else:
            resp["body"] = norm.text(raw.decode("utf-8", "replace"))
    req = {"method": method, "path": norm.text(path), "auth": auth}
    if query:
        req["query"] = [[k, norm.text(str(v))] for k, v in (query.items() if isinstance(query, dict) else query)]
    hdr = {k.lower(): norm.text(v) for k, v in (headers or {}).items()}
    if hdr:
        req["headers"] = hdr
    if body is not None:
        req["body"] = norm.value(body)
    if raw_body is not None:
        req["raw_body"] = norm.text(raw_body)
    case = {"seq": SEQ.next(), "id": case_id, "operationId": op, "server": server.name, "request": req, "response": resp}
    if note:
        case["note"] = note
    cases.append(case)
    print(f"  {case_id}: {status}", file=sys.stderr)
    return status, parsed


def family_of(path):
    parts = [p for p in path.split("/") if p]
    if not parts:
        return "root"
    if parts[0] == "api":
        return "api-" + (parts[1] if len(parts) > 1 else "root")
    if parts[0] == "experimental" and len(parts) > 1:
        return "legacy-experimental"
    return "legacy-" + parts[0]


def main():
    spec = json.load(open(OPENAPI))
    ops = []
    for path, item in spec["paths"].items():
        for m, o in item.items():
            if m in METHODS:
                ops.append((m.upper(), path, o))
    nopass = Server("nopass", None)
    passwd = Server("pass", PASSWORD)
    try:
        families = run(spec, ops, nopass, passwd)
    finally:
        nopass.stop()
        passwd.stop()
    os.makedirs(OUT, exist_ok=True)
    for name, cases in families.items():
        with open(os.path.join(OUT, name + ".json"), "w") as f:
            json.dump({"family": name, "baseline": "packages/opencode serve (TypeScript)", "cases": cases}, f, indent=2, ensure_ascii=False)
            f.write("\n")
    leftover = []
    for name in families:
        text = open(os.path.join(OUT, name + ".json")).read()
        for needle in (os.path.realpath(os.path.expanduser("~")), "/private/var", "/var/folders", PASSWORD, "p06fx-"):
            if needle in text:
                leftover.append((name, needle))
    if leftover:
        print("SANITIZE FAILURE", leftover, file=sys.stderr)
        sys.exit(1)
    for s in (nopass, passwd):
        if os.environ.get("P06_KEEP"):
            print(f"kept {s.root}", file=sys.stderr)
        else:
            shutil.rmtree(s.root, ignore_errors=True)
    print(f"wrote {sum(len(c) for c in families.values())} cases in {len(families)} families", file=sys.stderr)


def run(spec, ops, nopass, passwd):
    norm = Normalizer([nopass, passwd])
    fam = {}

    def rec(family, *a, **k):
        return record(fam.setdefault(family, []), norm, *a, **k)

    S = nopass
    # --- setup resources (recorded) ---
    _, project = rec("legacy-project", S, "setup:project.current", "project.current", "GET", "/project/current")
    project_id = (project or {}).get("id", "global")
    _, legacy_ses = rec("legacy-session", S, "setup:session.create", "session.create", "POST", "/session", body={"title": "fixture"})
    lsid = (legacy_ses or {}).get("id", "ses_fixture0000000000000000000")
    _, msg = rec("legacy-session", S, "setup:session.prompt.noReply", "session.prompt", "POST", f"/session/{lsid}/message",
                 body={"noReply": True, "parts": [{"type": "text", "text": "hello fixture"}]})
    mid = ((msg or {}).get("info") or {}).get("id", "msg_fixture0000000000000000000")
    pid = (((msg or {}).get("parts") or [{}])[0]).get("id", "prt_fixture0000000000000000000")
    def data(v):
        return v.get("data", v) if isinstance(v, dict) else {}

    _, v2ses = rec("api-session", S, "setup:v2.session.create", "v2.session.create", "POST", "/api/session",
                   body={"location": {"directory": S.proj}})
    vsid = data(v2ses).get("id", "ses_fixture0000000000000000001")
    _, pty = rec("api-pty", S, "setup:v2.pty.create", "v2.pty.create", "POST", "/api/pty",
                 body={"command": "/bin/sh", "args": ["-c", "sleep 30"], "title": "fixture"})
    ptyid = data(pty).get("id", "pty_fixture0000000000000000000")
    _, lpty = rec("legacy-pty", S, "setup:pty.create", "pty.create", "POST", "/pty",
                  body={"command": "/bin/sh", "args": ["-c", "sleep 30"], "title": "fixture"})
    lptyid = (lpty or {}).get("id", "pty_fixture0000000000000000001")

    missing = {
        "sessionID": "ses_missing000000000000000000", "messageID": "msg_missing000000000000000000",
        "partID": "prt_missing000000000000000000", "ptyID": "pty_missing000000000000000000",
        "requestID": "per_missing000000000000000000", "permissionID": "per_missing000000000000000000",
        "projectID": "prj_missing", "providerID": "missing-provider", "name": "missing-mcp",
        "integrationID": "missing-integration", "attemptID": "att_missing", "credentialID": "cred_missing",
        "id": "wrk_missing000000000000000000",
    }

    def params(path, o, current):
        values = {
            "sessionID": vsid if current else lsid, "messageID": mid, "partID": pid,
            "ptyID": ptyid if current else lptyid, "projectID": project_id,
            "requestID": "per_missing000000000000000000" if "permission" in path else "que_missing000000000000000000",
            "permissionID": "per_missing000000000000000000", "providerID": "missing-provider",
            "name": "fixture-mcp", "integrationID": "missing-integration", "attemptID": "att_missing",
            "credentialID": "cred_missing", "id": "wrk_missing000000000000000000",
        }
        if current and "/message/{messageID}" in path:
            values["messageID"] = "msg_missing000000000000000000"
        return values

    BODIES = {
        "auth.set": {"type": "api", "key": "fixture-not-a-key"},
        "app.log": {"service": "fixture", "level": "info", "message": "fixture log"},
        "experimental.controlPlane.moveSession": {"sessionID": "ses_missing000000000000000000", "destination": {"directory": nopass.proj}},
        "global.config.update": {"share": "disabled"},
        "global.upgrade": None,  # never run a real upgrade: only the schema rejection is captured
        "config.update": {"share": "disabled"},
        "experimental.console.switchOrg": {"accountID": "missing", "orgID": "missing"},
        "worktree.create": None,
        "worktree.remove": {"directory": "{PROJ}/missing-worktree"},
        "worktree.reset": {"directory": "{PROJ}/missing-worktree"},
        "vcs.apply": {"patch": "not a patch"},
        "mcp.add": {"name": "fixture-mcp", "config": {"type": "remote", "url": "http://127.0.0.1:9/mcp", "enabled": False}},
        "mcp.auth.callback": {"code": "fixture"},
        "project.update": {"name": "fixture project"},
        "experimental.projectCopy.generateName": {"context": "fixture"},
        "pty.update": {"title": "renamed", "size": {"rows": 24, "cols": 80}},
        "question.reply": {"answers": [["a"]]},
        "permission.reply": {"reply": "once"},
        "provider.oauth.authorize": {"method": 0},
        "provider.oauth.callback": {"method": 0, "code": "fixture"},
        "session.create": {"title": "fixture 2"},
        "session.update": {"title": "renamed"},
        "session.prompt": {"noReply": True, "parts": [{"type": "text", "text": "second"}]},
        "session.fork": {},
        "session.init": {"modelID": "missing", "providerID": "missing", "messageID": "msg_missing000000000000000000"},
        "session.summarize": {"providerID": "missing", "modelID": "missing"},
        "session.prompt_async": {"noReply": True, "parts": [{"type": "text", "text": "async"}]},
        "session.command": {"command": "missing-command", "arguments": ""},
        "session.shell": None,  # needs a model to run; only the schema rejection is captured
        "session.revert": {"messageID": "msg_missing000000000000000000"},
        "permission.respond": {"response": "once"},
        "part.update": {"id": "prt_missing000000000000000000", "sessionID": "ses_missing000000000000000000", "messageID": "msg_missing000000000000000000", "type": "text", "text": "x"},
        "sync.replay": {"directory": "{PROJ}", "events": []},
        "sync.steal": {"sessionID": "ses_missing000000000000000000"},
        "sync.history.list": {},
        "tui.appendPrompt": {"text": "fixture"},
        "tui.executeCommand": {"command": "fixture"},
        "tui.showToast": {"message": "fixture", "variant": "info"},
        "tui.publish": {"type": "tui.prompt.append", "properties": {"text": "fixture"}},
        "tui.selectSession": {"sessionID": "ses_missing000000000000000000"},
        "tui.control.response": {"ok": True},
        "experimental.workspace.create": None,
        "experimental.workspace.warp": {"id": None, "sessionID": "ses_missing000000000000000000"},
        "v2.session.create": {"location": {"directory": "{PROJ}"}},
        "v2.session.switchAgent": {"agent": "build"},
        "v2.session.switchModel": {"model": {"id": "missing", "providerID": "missing"}},
        "v2.session.prompt": {"prompt": {"text": "hello"}, "resume": False},
        "v2.session.revert.stage": {"messageID": "msg_missing000000000000000000"},
        "v2.integration.connect.key": {"key": "fixture-not-a-key"},
        "v2.integration.connect.oauth": {"methodID": "missing", "inputs": {}},
        "v2.integration.attempt.complete": {"code": "fixture"},
        "v2.credential.update": {"label": "fixture"},
        "v2.session.permission.create": {"action": "read", "resources": ["README.md"]},
        "v2.session.permission.reply": {"reply": "once"},
        "v2.pty.update": {"title": "renamed", "size": {"rows": 24, "cols": 80}},
        "v2.session.question.reply": {"answers": [["a"]]},
        "v2.projectCopy.create": None,
        "v2.projectCopy.remove": {"directory": "{PROJ}/missing-copy", "force": False},
        "pty.create": None, "v2.pty.create": None,  # covered by setup
    }
    QUERIES = {
        "experimental.session.list": {"limit": "5"},
        "tool.list": {"provider": "missing", "model": "missing"},
        "find.text": {"pattern": "fixture"},
        "find.files": {"query": "main", "limit": "5"},
        "find.symbols": {"query": "hello"},
        "file.list": {"path": "."},
        "file.read": {"path": "README.md"},
        "vcs.diff": {"mode": "git"},
        "session.list": {"limit": "5"},
        "session.messages": {"limit": "5"},
        "v2.session.list": {"limit": "5"},
        "v2.session.history": {"limit": "5"},
        "v2.session.messages": {"limit": "5"},
        "v2.fs.list": {"path": "."},
        "v2.fs.find": {"query": "main", "limit": "5"},
        "v2.permission.saved.list": {"projectID": "{PROJECT}"},
    }
    SKIP_HAPPY = {
        "global.dispose": "run last (disposes every instance)",
        "instance.dispose": "run last (disposes the instance)",
        "tui.control.next": "long-poll; captured with a short timeout",
        "event.subscribe": "SSE; captured in sse family", "global.event": "SSE; captured in sse family",
        "v2.event.subscribe": "SSE; captured in sse family", "v2.session.events": "SSE; captured in sse family",
        "v2.pty.connect": "WebSocket; plain GET captured", "pty.connect": "WebSocket; plain GET captured",
    }
    LAST = ("pty.remove", "v2.pty.remove", "session.delete", "session.deleteMessage", "part.delete")

    def subst(v):
        if isinstance(v, dict):
            return {k: subst(x) for k, x in v.items()}
        if isinstance(v, list):
            return [subst(x) for x in v]
        if isinstance(v, str):
            return v.replace("{PROJ}", S.proj).replace("{PROJECT}", project_id)
        return v

    def fill(path, values):
        out = path
        for k, v in values.items():
            out = out.replace("{" + k + "}", urllib.parse.quote(v, safe=""))
        if out.endswith("/*"):
            out = out[:-1] + "README.md"
        return out

    ordered = sorted(ops, key=lambda t: t[2].get("operationId") in LAST)
    for method, path, o in ordered:
        op = o.get("operationId")
        current = path.startswith("/api/")
        family = family_of(path)
        values = params(path, o, current)
        body = BODIES.get(op, {} if "requestBody" in o else None)
        if op in SKIP_HAPPY:
            pass
        elif "requestBody" in o and body is None:
            pass
        else:
            q = subst(QUERIES.get(op)) if op in QUERIES else None
            hdr = {"x-opencode-ticket": "1"} if op in ("pty.connectToken", "v2.pty.connectToken") else None
            rec(family, S, f"{op}#ok", op, method, fill(path, values), query=q, body=subst(body) if body is not None else None, headers=hdr)
        if "requestBody" in o:
            rec(family, S, f"{op}#bad-body", op, method, fill(path, values), raw_body="{\"__invalid\":", headers={"content-type": "application/json"},
                note="malformed JSON body")
            rec(family, S, f"{op}#wrong-type", op, method, fill(path, values), body=[1, 2, 3], note="schema rejection: array body")
        if "{sessionID}" in path and op not in SKIP_HAPPY:
            mv = dict(values)
            mv["sessionID"] = missing["sessionID"]
            rec(family, S, f"{op}#missing-session", op, method, fill(path, mv), body=subst(body) if body is not None else None,
                headers={"x-opencode-ticket": "1"} if "connect-token" in path else None)
        if "{ptyID}" in path and op not in SKIP_HAPPY:
            mv = dict(values)
            mv["ptyID"] = missing["ptyID"]
            rec(family, S, f"{op}#missing-pty", op, method, fill(path, mv), body=subst(body) if body is not None else None,
                headers={"x-opencode-ticket": "1"} if "connect-token" in path else None)
        if op in QUERIES and any(p.get("required") for p in o.get("parameters", []) if p["in"] == "query"):
            rec(family, S, f"{op}#missing-query", op, method, fill(path, values), note="required query omitted")

    # --- schema / query rejections that are not body-shaped ---
    rec("api-session", S, "v2.session.list#bad-limit", "v2.session.list", "GET", "/api/session", query={"limit": "abc"})
    rec("api-session", S, "v2.session.list#bad-order", "v2.session.list", "GET", "/api/session", query={"order": "sideways"})
    rec("api-session", S, "v2.session.list#bad-cursor", "v2.session.list", "GET", "/api/session", query={"cursor": "not-a-cursor"})
    rec("api-session", S, "v2.session.get#bad-id", "v2.session.get", "GET", "/api/session/not-an-id")
    rec("legacy-session", S, "session.get#bad-id", "session.get", "GET", "/session/not-an-id")
    rec("api-session", S, "v2.session.prompt#text-plain", "v2.session.prompt", "POST", f"/api/session/{vsid}/prompt",
        raw_body="hello", headers={"content-type": "text/plain"})
    rec("api-session", S, "v2.session.compact#ok-running", "v2.session.compact", "POST", f"/api/session/{vsid}/compact")
    rec("api-session", S, "v2.session.wait#ok-running", "v2.session.wait", "POST", f"/api/session/{vsid}/wait", timeout=10)

    # --- 404 envelopes ---
    for p in ("/api/does-not-exist", "/api/session/x/y/z", "/does-not-exist-route"):
        rec("negative-notfound", S, f"notfound:GET {p}", None, "GET", p)
    rec("negative-notfound", S, "notfound:DELETE /api/health", None, "DELETE", "/api/health")

    # --- auth (password server) ---
    P = passwd
    for auth in ("none", "invalid", "valid", "query-valid"):
        for op, method, path in (("v2.health.get", "GET", "/api/health"), ("v2.session.list", "GET", "/api/session"),
                                 ("global.health", "GET", "/global/health"), ("session.list", "GET", "/session"),
                                 (None, "GET", "/doc"), ("v2.session.create", "POST", "/api/session")):
            if path == "/doc" and auth in ("valid", "query-valid"):
                continue
            body = {"location": {"directory": P.proj}} if method == "POST" else None
            rec("negative-auth", P, f"auth:{auth}:{method} {path}", op, method, path, auth=auth, body=body)
    rec("negative-auth", P, "auth:none:GET /", None, "GET", "/", auth="none")
    rec("negative-auth", P, "auth:none:GET /api/pty/x/connect?ticket", "v2.pty.connect", "GET",
        "/api/pty/pty_missing000000000000000000/connect", query={"ticket": "bogus"}, auth="none")
    rec("negative-auth", P, "auth:none:OPTIONS preflight", None, "OPTIONS", "/api/session", auth="none",
        headers={"origin": "http://localhost:3000", "access-control-request-method": "GET"})

    # --- CORS (no password server) ---
    for origin in ("http://localhost:3000", "http://127.0.0.1:5173", "https://opencode.ai", "https://app.opencode.ai",
                   "tauri://localhost", "oc://renderer", "https://evil.example.com", "https://opencode.ai.evil.com", "http://localhost"):
        rec("negative-cors", S, f"cors:preflight:{origin}", None, "OPTIONS", "/api/session",
            headers={"origin": origin, "access-control-request-method": "POST", "access-control-request-headers": "content-type,x-opencode-directory"})
        rec("negative-cors", S, f"cors:simple:{origin}", "v2.health.get", "GET", "/api/health", headers={"origin": origin})
    rec("negative-cors", S, "cors:preflight:legacy", None, "OPTIONS", "/session",
        headers={"origin": "http://localhost:3000", "access-control-request-method": "GET"})

    # --- SSE first frames ---
    rec("sse", S, "sse:/api/event", "v2.event.subscribe", "GET", "/api/event", sse=True)
    rec("sse", S, "sse:/event", "event.subscribe", "GET", "/event", sse=True)
    rec("sse", S, "sse:/global/event", "global.event", "GET", "/global/event", sse=True)
    rec("sse", S, "sse:/api/session/:id/event", "v2.session.events", "GET", f"/api/session/{vsid}/event", sse=True)
    rec("sse", S, "sse:/api/session/:id/event#missing", "v2.session.events", "GET", f"/api/session/{missing['sessionID']}/event", sse=True)
    rec("sse", S, "sse:/api/session/:id/event#bad-after", "v2.session.events", "GET", f"/api/session/{vsid}/event", query={"after": "x"}, sse=True)
    rec("sse", P, "sse:/api/event#unauthorized", "v2.event.subscribe", "GET", "/api/event", auth="none", sse=True)
    rec("api-pty", S, "v2.pty.connect#plain-get", "v2.pty.connect", "GET", f"/api/pty/{ptyid}/connect")
    rec("legacy-pty", S, "pty.connect#plain-get", "pty.connect", "GET", f"/pty/{lptyid}/connect")
    rec("legacy-tui", S, "tui.control.next#timeout", "tui.control.next", "GET", "/tui/control/next", timeout=3)

    # --- docs ---
    rec("docs", S, "doc:/doc", None, "GET", "/doc")
    rec("docs", S, "doc:/openapi.json", None, "GET", "/openapi.json")

    # --- disposal last ---
    rec("legacy-instance", S, "instance.dispose#ok", "instance.dispose", "POST", "/instance/dispose")
    rec("legacy-global", S, "global.dispose#ok", "global.dispose", "POST", "/global/dispose")
    return fam


if __name__ == "__main__":
    main()
