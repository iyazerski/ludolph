#!/usr/bin/env python3
"""A stand-in for `pi --mode rpc` that answers a few commands with canned records.

With FAKE_PI_SESSIONS set, it keeps a session file like pi's: a new session
goes in that folder, and `--session <file>` resumes one. FAKE_PI_LOG gets a
line when it starts and when it exits.
"""

import json
import os
import signal
import subprocess
import sys
import time


def emit(record, end="\n"):
    sys.stdout.write(json.dumps(record) + end)
    sys.stdout.flush()


def respond(command, data=None):
    emit({"id": command.get("id"), "type": "response", "command": command["type"], "success": True, "data": data})


def update(event):
    emit({"type": "message_update", "assistantMessageEvent": event})


def log(line):
    if os.environ.get("FAKE_PI_LOG"):
        with open(os.environ["FAKE_PI_LOG"], "a") as f:
            f.write(line + "\n")


def session():
    """The session's id and file, from the arguments."""
    args = sys.argv[1:]
    if "--session" in args:
        path = args[args.index("--session") + 1]
        with open(path) as f:
            return json.loads(f.readline())["id"], path
    id = args[args.index("--session-id") + 1]
    folder = os.environ.get("FAKE_PI_SESSIONS")
    return id, folder and os.path.join(folder, "--fake--", f"{id}.jsonl")


SESSION_ID, SESSION_FILE = session()


def write(message):
    """Append a message entry to the session file, after a header if it's
    new, as a child of the last entry, like pi."""
    if not SESSION_FILE:
        return
    os.makedirs(os.path.dirname(SESSION_FILE), exist_ok=True)
    lines = []
    parent = None
    if os.path.exists(SESSION_FILE):
        with open(SESSION_FILE) as f:
            parent = json.loads(f.readlines()[-1]).get("id") or None
    else:
        lines.append({"type": "session", "version": 3, "id": SESSION_ID,
                      "timestamp": "2026-05-01T10:00:00.000Z", "cwd": os.getcwd()})
    lines.append({"type": "message", "id": f"e{time.monotonic_ns()}", "parentId": parent,
                  "timestamp": "2026-05-01T10:00:00.000Z", "message": message})
    with open(SESSION_FILE, "a") as f:
        f.writelines(json.dumps(line) + "\n" for line in lines)


def now():
    """Epoch milliseconds, as pi stamps messages."""
    return time.time_ns() // 1_000_000


def prompt(command):
    respond(command, {"disposition": "started"})
    user = {"role": "user", "content": command["message"], "timestamp": now()}
    emit({"type": "agent_start"})
    emit({"type": "message_start", "message": user})
    emit({"type": "message_end", "message": user})
    write(user)
    if command["message"] == "crash":
        log("crash")
        os._exit(1)
    started = now()
    emit({"type": "message_start", "message": {"role": "assistant", "content": [], "stopReason": "pending", "timestamp": started}})
    update({"type": "text_start", "contentIndex": 0})
    update({"type": "text_delta", "contentIndex": 0, "delta": "Hel"})
    update({"type": "text_delta", "contentIndex": 0, "delta": "lo"})
    update({"type": "text_end", "contentIndex": 0, "content": "Hello!"})
    call = {"type": "toolCall", "id": "c1", "name": "bash", "arguments": {"command": "ls"}}
    update({"type": "toolcall_start", "contentIndex": 1, "id": "c1", "toolName": "bash"})
    update({"type": "toolcall_delta", "contentIndex": 1, "delta": '{"command":'})
    update({"type": "toolcall_end", "contentIndex": 1, "toolCall": call})
    assistant = {"role": "assistant", "content": [{"type": "text", "text": "Hello!"}, call],
                 "stopReason": "toolUse", "timestamp": started}
    emit({"type": "message_end", "message": assistant})
    write(assistant)
    emit({"type": "tool_execution_start", "toolCallId": "c1", "toolName": "bash", "args": {"command": "ls"}})
    emit({"type": "tool_execution_update", "toolCallId": "c1", "toolName": "bash", "args": {}, "partialResult": "a.txt"})
    emit({"type": "tool_execution_end", "toolCallId": "c1", "toolName": "bash", "result": {}, "isError": False})
    result = {"role": "toolResult", "toolCallId": "c1", "content": [{"type": "text", "text": "a.txt"}],
              "isError": False, "timestamp": now()}
    emit({"type": "message_end", "message": result})
    write(result)
    done = {"role": "assistant", "content": [{"type": "text", "text": "Done."}], "stopReason": "stop",
            "timestamp": now()}
    emit({"type": "message_end", "message": done})
    write(done)
    emit({"type": "compaction_start", "reason": "threshold"})
    emit({"type": "agent_settled", "aborted": False})


def main():
    if os.environ.get("FAKE_PI_STUBBORN"):
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
    log("start " + " ".join(sys.argv[1:]))
    sys.stdout.write("not json\n")
    for line in sys.stdin:
        command = json.loads(line)
        kind = command["type"]
        if kind == "get_state":
            # pi accepts CRLF too.
            emit({"id": command["id"], "type": "response", "command": kind, "success": True,
                  "data": {"args": sys.argv[1:], "cwd": os.getcwd(), "sessionId": SESSION_ID,
                           "sessionFile": SESSION_FILE}}, end="\r\n")
        elif kind == "prompt":
            prompt(command)
        elif kind == "spawn_child":
            respond(command, {"pid": subprocess.Popen(["sleep", "60"]).pid})
        elif kind in ("abort", "clear_queue"):
            respond(command)
        elif kind == "exit":
            sys.exit(3)
        else:
            emit({"id": command["id"], "type": "response", "command": kind, "success": False,
                  "error": f"Unknown command: {kind}"})
    if os.environ.get("FAKE_PI_STUBBORN"):
        time.sleep(60)
    log("exit")


main()
