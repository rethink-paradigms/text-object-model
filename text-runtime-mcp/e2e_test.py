#!/usr/bin/env python3
"""End-to-end round trip through the text-runtime MCP server:
ingest -> list -> read (markers) -> annotate -> search."""
import json
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
MCP_DIR = REPO_ROOT / "text-runtime-mcp"

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


def recv(timeout: float = 120.0) -> dict:
    import select

    while True:
        r, _, _ = select.select([proc.stdout], [], [], timeout)
        if not r:
            raise TimeoutError("no response")
        line = proc.stdout.readline().strip()
        if not line:
            continue
        return json.loads(line)


def call(name: str, args: dict) -> str:
    send({"jsonrpc": "2.0", "id": 99, "method": "tools/call",
          "params": {"name": name, "arguments": args}})
    resp = recv()
    content = resp.get("result", {}).get("content", [])
    return "".join(c.get("text", "") for c in content if c.get("type") == "text")


send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
    "protocolVersion": "2024-11-05",
    "capabilities": {},
    "clientInfo": {"name": "e2e", "version": "0.1"},
}})
recv()
send({"jsonrpc": "2.0", "method": "notifications/initialized"})

# 1. Ingest a fresh document (into the default ~/.textruntime corpus)
out = call("ingest_document", {"path": str(REPO_ROOT / "text-runtime" / "demo_document.md")})
print("INGEST:", out.strip())
m = re.search(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", out)
doc_id = m.group(0)

# 2. List documents
print("LIST:", call("list_documents", {}).strip().splitlines()[0])

# 3. Read with markers
out = call("read_document", {"doc_id": doc_id, "markers": True})
print("READ:", out.strip()[:180].replace("\n", " | "))

# 4. Get sentence anchors, then annotate the first sentence
out = call("document_sentences", {"doc_id": doc_id})
print("SENTENCES:", out.strip().splitlines()[0][:110])
m2 = re.search(r"\| ([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}) \|", out)
if m2:
    sentence_uuid = m2.group(1)
    out = call("annotate_sentence", {"doc_id": doc_id, "sentence_uuid": sentence_uuid,
                                     "body": "First interface test annotation.", "motivation": "commenting"})
    print("ANNOTATE:", out.strip())
else:
    print("ANNOTATE: skipped (no sentence found)")
    print("FULL SENTENCES OUTPUT:", out)

# 5. Search
print("SEARCH:", call("search_corpus", {"query": "demo"}).strip().splitlines()[0])

proc.terminate()
proc.wait(timeout=10)
print("E2E OK")
