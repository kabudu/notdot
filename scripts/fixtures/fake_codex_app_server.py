#!/usr/bin/env python3
"""Disposable stand-in for `codex app-server` used only for native UI QA.

It answers the handful of requests notdot makes at start-up with synthetic
data, never contacts any service, and lets the tester inject server requests
by dropping JSON files into $FAKE_CODEX_DIR/ctl. Every request notdot sends,
and every reply to an injected request, is logged to $FAKE_CODEX_DIR.
"""
import json, os, sys, threading, time
from pathlib import Path

base = Path(os.environ["FAKE_CODEX_DIR"])
ctl = base / "ctl"
ctl.mkdir(parents=True, exist_ok=True)
lock = threading.Lock()


def log(name, value):
    with open(base / name, "a") as f:
        f.write(json.dumps({"t": round(time.time(), 3), **value}) + "\n")


def send(message):
    with lock:
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()


THREAD = {"id": "qa-thread", "preview": "", "createdAt": 0, "updatedAt": 0,
          "name": "notdot · companion", "turns": [], "cwd": "/", "status": {"type": "idle"}}
MODEL = {"id": "gpt-6.1-terra", "model": "gpt-6.1-terra", "displayName": "Terra",
         "description": "Synthetic", "isDefault": True, "hidden": False,
         "defaultReasoningEffort": "medium",
         "supportedReasoningEfforts": [{"reasoningEffort": "medium", "description": "Medium"}]}


def result_for(method):
    if method == "initialize":
        return {"userAgent": "fake-codex/0", "platformFamily": "unix", "platformOs": "macos"}
    if method == "account/read":
        return {"account": {"type": "chatgpt", "email": "qa@example.invalid", "planType": "plus"},
                "requiresOpenaiAuth": True}
    if method == "model/list":
        return {"data": [MODEL], "nextCursor": None}
    if method == "config/read":
        return {"config": {}, "origins": {}}
    if method in ("thread/start", "thread/resume"):
        return {"thread": THREAD, "model": "gpt-6.1-terra", "modelProvider": "openai",
                "cwd": "/", "approvalPolicy": "untrusted", "sandbox": {"type": "readOnly"},
                "reasoningEffort": "medium"}
    if method == "thread/list":
        return {"data": [], "nextCursor": None}
    return {}


def reader():
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if "method" in message and "id" in message:
            log("requests.jsonl", {"method": message["method"],
                                   "params": json.dumps(message.get("params"))[:600]})
            send({"id": message["id"], "result": result_for(message["method"])})
        elif "method" in message:
            log("notifications.jsonl", {"method": message["method"]})
        else:
            log("answers.jsonl", {"id": message.get("id"), "result": message.get("result"),
                                  "error": message.get("error")})
    log("lifecycle.jsonl", {"event": "stdin closed"})
    os._exit(0)


threading.Thread(target=reader, daemon=True).start()
log("lifecycle.jsonl", {"event": "started", "pid": os.getpid()})
while True:
    for path in sorted(ctl.glob("*.json")):
        try:
            command = json.loads(path.read_text())
        except ValueError:
            continue
        path.unlink()
        log("injected.jsonl", command)
        op = command["op"]
        if op == "ask":
            send({"id": command["id"], "method": "mcpServer/elicitation/request", "params": {
                "threadId": "qa-thread", "turnId": command.get("turn", "qa-turn-1"),
                "serverName": command.get("server", "notdot_host"), "mode": "form",
                "message": command["message"],
                "requestedSchema": {"type": "object", "properties": command.get("properties", {})},
                "_meta": command.get("meta")}})
        elif op == "complete":
            send({"method": "turn/completed", "params": {"threadId": "qa-thread", "turn": {
                "id": command["turn"], "status": command.get("status", "interrupted"), "items": []}}})
        elif op == "exit":
            log("lifecycle.jsonl", {"event": "exit requested"})
            os._exit(0)
    time.sleep(0.2)
