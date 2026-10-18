#!/usr/bin/env python3
# Purpose: scriptable fake LSP server for integration-adapters lsp tests (fixture, not live).
# Scope: stdio JSON-RPC with Content-Length framing. Logs every inbound message as JSON lines.
# Uses:
# `-- none
# External:
# `-- python3 stdlib (json, os, sys, threading)
# Used by:
# rust/crates/
# |-- integration-adapters/tests/lsp_client.rs
# `-- integration-adapters/tests/lsp_service.rs
# Invariants: behavior is selected only through FAKE_* env vars, never touches files outside
# FAKE_LSP_LOG.
# Contract tests: integration-adapters/tests/lsp_client.rs
import json
import os
import sys
import threading
import time

LOG = os.environ.get("FAKE_LSP_LOG")
SYNC = int(os.environ.get("FAKE_SYNC", "1"))
PULL = os.environ.get("FAKE_PULL") == "1"
PUSH = os.environ.get("FAKE_PUSH", "1") == "1"
REGISTER = os.environ.get("FAKE_REGISTER")  # identifier for a dynamic document registration
WORKSPACE_REGISTER = os.environ.get("FAKE_WORKSPACE_REGISTER") == "1"
HANG_INIT = os.environ.get("FAKE_HANG_INIT") == "1"
PUSH_DELAY = float(os.environ.get("FAKE_PUSH_DELAY", "0"))
lock = threading.Lock()
out = sys.stdout.buffer
next_id = [1000]


def log(record):
    if not LOG:
        return
    with open(LOG, "a") as f:
        f.write(json.dumps(record) + "\n")


def send(msg):
    body = json.dumps(msg).encode()
    with lock:
        out.write(b"Content-Length: %d\r\n\r\n" % len(body))
        out.write(body)
        out.flush()


def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            if length is not None:
                break
            continue
        name, _, value = line.decode().partition(":")
        if name.lower() == "content-length":
            length = int(value.strip())
    return json.loads(sys.stdin.buffer.read(length))


def diag(message, line=0):
    return {"range": {"start": {"line": line, "character": 0}, "end": {"line": line, "character": 1}},
            "severity": 1, "message": message, "source": "fake"}


def publish(uri, version):
    if PUSH_DELAY:
        time.sleep(PUSH_DELAY)
    send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
          "params": {"uri": uri, "version": version, "diagnostics": [diag("push v%d" % version)]}})


def location(uri, line):
    return {"uri": uri, "range": {"start": {"line": line, "character": 0}, "end": {"line": line, "character": 3}}}


def handle(msg):
    method = msg.get("method")
    params = msg.get("params") or {}
    if method == "initialize":
        if HANG_INIT:
            return None
        caps = {"textDocumentSync": SYNC if SYNC != 99 else {"change": 2, "openClose": True}}
        if PULL:
            caps["diagnosticProvider"] = {"interFileDependencies": False, "workspaceDiagnostics": False}
        return {"capabilities": caps}
    uri = (params.get("textDocument") or {}).get("uri", "")
    pos = params.get("position") or {}
    if method == "textDocument/hover":
        return {"contents": "hover %d:%d" % (pos.get("line", 0), pos.get("character", 0))}
    if method == "textDocument/definition":
        return [location(uri, 1)]
    if method == "textDocument/references":
        return [location(uri, 2), location(uri, 3)] if params.get("context", {}).get("includeDeclaration") else []
    if method == "textDocument/implementation":
        return None
    if method == "textDocument/documentSymbol":
        r = location(uri, 0)["range"]
        return [{"name": "main", "kind": 12, "range": r, "selectionRange": r}]
    if method == "workspace/symbol":
        kinds = [1, 5, 12, 6, 11, 13, 14, 23, 10, 5, 12, 6, 2]
        return [{"name": "s%d" % i, "kind": k, "location": location("file:///x.rs", i)} for i, k in enumerate(kinds)]
    if method == "textDocument/prepareCallHierarchy":
        return [{"name": "f", "kind": 12, "uri": uri, "range": location(uri, 0)["range"],
                 "selectionRange": location(uri, 0)["range"]}]
    if method in ("callHierarchy/incomingCalls", "callHierarchy/outgoingCalls"):
        return [{"direction": method, "item": params.get("item")}]
    if method == "textDocument/diagnostic":
        ident = params.get("identifier")
        if os.environ.get("FAKE_PULL_EMPTY") == "1":
            return {"kind": "full", "items": []}
        related = {"file:///related/other.ts": {"kind": "full", "items": [diag("related")]}}
        return {"kind": "full", "items": [diag("pull %s" % (ident or "static")), diag("pull %s" % (ident or "static"))],
                "relatedDocuments": related}
    if method == "workspace/diagnostic":
        return {"items": [{"uri": "file:///ws/a.ts", "kind": "full", "items": [diag("workspace")]}]}
    if method == "shutdown":
        return None
    return "unhandled"


def server_request(method, params):
    next_id[0] += 1
    send({"jsonrpc": "2.0", "id": next_id[0], "method": method, "params": params})


def main():
    log({"event": "start", "pid": os.getpid(), "cwd": os.getcwd()})
    while True:
        msg = read()
        if msg is None:
            log({"event": "eof"})
            return
        log(msg)
        method = msg.get("method")
        params = msg.get("params") or {}
        if "id" in msg and method is None:
            continue  # response to one of our server requests (already logged)
        if method == "initialized":
            if REGISTER:
                server_request("client/registerCapability", {"registrations": [
                    {"id": "doc-reg", "method": "textDocument/diagnostic", "registerOptions": {"identifier": REGISTER}}]})
            if WORKSPACE_REGISTER:
                server_request("client/registerCapability", {"registrations": [
                    {"id": "ws-reg", "method": "textDocument/diagnostic",
                     "registerOptions": {"identifier": "ws", "workspaceDiagnostics": True}}]})
        elif method == "test/trigger":
            server_request(params["method"], params.get("params"))
        elif method in ("textDocument/didOpen", "textDocument/didChange") and PUSH:
            doc = params["textDocument"]
            threading.Thread(target=publish, args=(doc["uri"], doc.get("version", 0))).start()
        elif method == "exit":
            return
        if "id" in msg:
            result = handle(msg)
            if method == "initialize" and HANG_INIT:
                continue
            send({"jsonrpc": "2.0", "id": msg["id"], "result": result})


main()
