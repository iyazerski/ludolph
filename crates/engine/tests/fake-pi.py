#!/usr/bin/env python3
"""A stand-in for `pi --mode rpc` that answers a few commands with canned records."""

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


def prompt(command):
    respond(command, {"disposition": "started"})
    user = {"role": "user", "content": command["message"], "timestamp": 1}
    emit({"type": "agent_start"})
    emit({"type": "message_start", "message": user})
    emit({"type": "message_end", "message": user})
    emit({"type": "message_start", "message": {"role": "assistant", "content": [], "stopReason": "pending"}})
    update({"type": "text_start", "contentIndex": 0})
    update({"type": "text_delta", "contentIndex": 0, "delta": "Hel"})
    update({"type": "text_delta", "contentIndex": 0, "delta": "lo"})
    update({"type": "text_end", "contentIndex": 0, "content": "Hello!"})
    call = {"type": "toolCall", "id": "c1", "name": "bash", "arguments": {"command": "ls"}}
    update({"type": "toolcall_start", "contentIndex": 1, "id": "c1", "toolName": "bash"})
    update({"type": "toolcall_delta", "contentIndex": 1, "delta": '{"command":'})
    update({"type": "toolcall_end", "contentIndex": 1, "toolCall": call})
    assistant = {"role": "assistant", "content": [{"type": "text", "text": "Hello!"}, call], "stopReason": "toolUse"}
    emit({"type": "message_end", "message": assistant})
    emit({"type": "tool_execution_start", "toolCallId": "c1", "toolName": "bash", "args": {"command": "ls"}})
    emit({"type": "tool_execution_update", "toolCallId": "c1", "toolName": "bash", "args": {}, "partialResult": "a.txt"})
    emit({"type": "tool_execution_end", "toolCallId": "c1", "toolName": "bash", "result": {}, "isError": False})
    emit({"type": "message_end", "message": {"role": "toolResult", "toolCallId": "c1", "content": [{"type": "text", "text": "a.txt"}]}})
    emit({"type": "compaction_start", "reason": "threshold"})
    emit({"type": "agent_settled", "aborted": False})


def main():
    if os.environ.get("FAKE_PI_STUBBORN"):
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
    sys.stdout.write("not json\n")
    for line in sys.stdin:
        command = json.loads(line)
        kind = command["type"]
        if kind == "get_state":
            # pi accepts CRLF too.
            emit({"id": command["id"], "type": "response", "command": kind, "success": True,
                  "data": {"args": sys.argv[1:], "cwd": os.getcwd()}}, end="\r\n")
        elif kind == "prompt":
            prompt(command)
        elif kind == "spawn_child":
            respond(command, {"pid": subprocess.Popen(["sleep", "60"]).pid})
        elif kind == "exit":
            sys.exit(3)
        else:
            emit({"id": command["id"], "type": "response", "command": kind, "success": False,
                  "error": f"Unknown command: {kind}"})
    if os.environ.get("FAKE_PI_STUBBORN"):
        time.sleep(60)


main()
