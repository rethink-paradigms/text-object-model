#!/usr/bin/env python3
"""Minimal MCP stdio handshake test for the text-runtime-mcp server."""
import json
import os
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
MCP_DIR = REPO_ROOT / "text-runtime-mcp"
RUNTIME_DIR = os.environ.get("TEXT_RUNTIME_DIR") or str(
    REPO_ROOT / "text-runtime" / ".textruntime"
)

BIN = [sys.executable, "-m", "text_runtime_mcp"]

proc = subprocess.Popen(
    BIN,
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    cwd=str(MCP_DIR),
)


def send(msg: dict) -> None:
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()


def recv(timeout: float = 30.0) -> dict:
    import select

    while True:
        r, _, _ = select.select([proc.stdout], [], [], timeout)
        if not r:
            raise TimeoutError("no response")
        line = proc.stdout.readline()
        if line == "":
            raise EOFError("server closed stdout")
        line = line.strip()
        if not line:
            continue
        try:
            return json.loads(line)
        except json.JSONDecodeError as e:
            print(f"NON-JSON LINE: {line[:200]!r}", file=sys.stderr)
            raise


send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
    "protocolVersion": "2024-11-05",
    "capabilities": {},
    "clientInfo": {"name": "smoke-test", "version": "0.1.0"},
}})
print("INIT:", json.dumps(recv())[:200])

send({"jsonrpc": "2.0", "method": "notifications/initialized"})
send({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
resp = recv()
tools = resp.get("result", {}).get("tools", [])
print("TOOLS:", [t["name"] for t in tools])

send({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
    "name": "list_documents",
    "arguments": {"runtime_dir": RUNTIME_DIR},
}})
resp = recv()
content = resp.get("result", {}).get("content", [])
print("CALL RESULT:", json.dumps(content)[:300])

proc.terminate()
proc.wait(timeout=10)
print("OK")
