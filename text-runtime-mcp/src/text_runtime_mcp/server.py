"""MCP server exposing the text-runtime engine to goose agents.

Each tool shells out to the `text-runtime` CLI binary (release build) and
returns its output. This keeps the interface thin: the engine stays the
single source of truth, and the MCP layer is just the agent-facing surface.

Configuration (environment variables):
  TEXT_RUNTIME_BIN  — path to the text-runtime binary
                      (default: <repo>/text-runtime/target/release/text-runtime)
  TEXT_RUNTIME_DIR  — default runtime directory (.textruntime/) used when a
                      tool call does not pass `runtime_dir`
                      (default: ~/.textruntime)
"""

from __future__ import annotations

import atexit
import json
import os
import subprocess
import time
import urllib.request
from pathlib import Path

from mcp.server.fastmcp import FastMCP
from mcp.shared.exceptions import McpError
from mcp.types import ErrorData, INTERNAL_ERROR, INVALID_PARAMS

REPO_ROOT = Path(__file__).resolve().parents[3]

DEFAULT_BIN = REPO_ROOT / "text-runtime" / "target" / "release" / "text-runtime"
DEFAULT_DIR = Path.home() / ".textruntime"

# The MCP server owns a persistent pandoc-server on a dedicated port so CLI
# invocations attach to a healthy server instead of racing for the default
# port 8472 (where orphaned servers from killed processes wedge health checks).
PANDOC_PORT = int(os.environ.get("TEXT_RUNTIME_PANDOC_PORT", "8499"))

mcp = FastMCP("text-runtime")

_pandoc_proc: subprocess.Popen | None = None


def _pandoc_healthy(timeout: float = 0.5) -> bool:
    try:
        with urllib.request.urlopen(
            f"http://127.0.0.1:{PANDOC_PORT}/version", timeout=timeout
        ) as r:
            return r.status == 200
    except Exception:
        return False


def _ensure_pandoc() -> None:
    """Make sure a healthy pandoc-server is listening on our dedicated port.

    Spawns it as our child (killed on exit), restarts it if wedged.
    """
    global _pandoc_proc
    if _pandoc_proc is not None and _pandoc_proc.poll() is None:
        if _pandoc_healthy():
            return
        # Alive but not responding — kill and respawn
        _pandoc_proc.kill()
        try:
            _pandoc_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
        _pandoc_proc = None

    if _pandoc_proc is None or _pandoc_proc.poll() is not None:
        _pandoc_proc = subprocess.Popen(
            ["pandoc", "server", "--port", str(PANDOC_PORT)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

    for _ in range(50):  # up to ~10s for first startup
        if _pandoc_healthy(timeout=1.0):
            return
        time.sleep(0.2)


def _ensure_config(runtime_dir: str) -> None:
    """Point the runtime dir's config.json at our dedicated pandoc port."""
    d = Path(runtime_dir)
    d.mkdir(parents=True, exist_ok=True)
    cfg_path = d / "config.json"
    cfg: dict = {}
    if cfg_path.exists():
        try:
            cfg = json.loads(cfg_path.read_text())
        except Exception:
            cfg = {}
    if cfg.get("pandoc_port") != PANDOC_PORT:
        cfg["pandoc_port"] = PANDOC_PORT
        cfg_path.write_text(json.dumps(cfg, indent=2) + "\n")


def _shutdown_pandoc() -> None:
    global _pandoc_proc
    if _pandoc_proc is not None and _pandoc_proc.poll() is None:
        _pandoc_proc.terminate()
        try:
            _pandoc_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            _pandoc_proc.kill()


atexit.register(_shutdown_pandoc)


def _bin() -> Path:
    return Path(os.environ.get("TEXT_RUNTIME_BIN", DEFAULT_BIN)).expanduser()


def _run(args: list[str], runtime_dir: str | None = None) -> str:
    """Run the text-runtime CLI and return stdout.

    Raises McpError(INTERNAL_ERROR) with stderr content on failure.
    """
    dir = _default_dir(runtime_dir)
    _ensure_config(dir)
    _ensure_pandoc()
    cmd = [str(_bin())]
    if dir:
        cmd += ["--runtime-dir", dir]
    cmd += args

    proc = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        timeout=120,
    )
    if proc.returncode != 0:
        detail = proc.stderr.strip() or proc.stdout.strip() or f"exit {proc.returncode}"
        raise McpError(
            ErrorData(
                code=INTERNAL_ERROR,
                message=f"text-runtime failed: {detail}",
            )
        )
    return proc.stdout


def _default_dir(runtime_dir: str | None) -> str:
    return runtime_dir or os.environ.get("TEXT_RUNTIME_DIR", str(DEFAULT_DIR))


@mcp.tool()
def list_documents(runtime_dir: str | None = None) -> str:
    """List all documents in the text-runtime store (uuid, title, format, ingested_at, version).

    Args:
        runtime_dir: Optional path to a .textruntime directory. Defaults to TEXT_RUNTIME_DIR or ~/.textruntime.
    """
    return _run(["list"], runtime_dir)


@mcp.tool()
def ingest_document(path: str, format: str | None = None, title: str | None = None, runtime_dir: str | None = None) -> str:
    """Ingest a text file (markdown, txt, html, org, rst, etc.) into the store and return its document UUID.

    Args:
        path: Absolute path to the file to ingest. Format is auto-detected from the extension.
        format: Optional explicit input format (e.g. "markdown", "rst", "html").
        title: Optional document title (defaults to filename).
        runtime_dir: Optional path to a .textruntime directory. Defaults to TEXT_RUNTIME_DIR or ~/.textruntime.
    """
    args = ["ingest", path]
    if format:
        args += ["--format", format]
    if title:
        args += ["--title", title]
    return _run(args, runtime_dir)


@mcp.tool()
def read_document(doc_id: str, format: str = "markdown", markers: bool = True, runtime_dir: str | None = None) -> str:
    """Project a stored document back to readable text, optionally with §N sentence markers.

    Use this to read a document before annotating: with markers=True the output
    includes a '--- markers ---' block mapping each §N to its sentence UUID.

    Args:
        doc_id: Document UUID (from list_documents or ingest_document).
        format: Output format: markdown (default), html, plain.
        markers: Inject §N sentence markers (default true).
        runtime_dir: Optional path to a .textruntime directory. Defaults to TEXT_RUNTIME_DIR or ~/.textruntime.
    """
    args = ["read", doc_id, "--format", format]
    if markers:
        args.append("--markers")
    return _run(args, runtime_dir)


@mcp.tool()
def document_sentences(doc_id: str, runtime_dir: str | None = None) -> str:
    """List the sentence anchors of a document: index (1-based §N), sentence UUID, and text.

    Use this to get the exact sentence_uuid to pass to annotate_sentence.
    Note: sentence UUIDs are stable across re-ingests (anchor identity),
    while §N indices are positional and may shift.

    Args:
        doc_id: Document UUID.
        runtime_dir: Optional path to a .textruntime directory. Defaults to TEXT_RUNTIME_DIR or ~/.textruntime.
    """
    return _run(["sentences", doc_id], runtime_dir)


@mcp.tool()
def annotate_sentence(doc_id: str, sentence_uuid: str, quote: str | None = None, body: str | None = None, motivation: str | None = None, runtime_dir: str | None = None) -> str:
    """Attach a W3C annotation to a specific sentence of a document. Returns the annotation UUID.

    Args:
        doc_id: Document UUID.
        sentence_uuid: Sentence UUID from read_document(..., markers=True).
        quote: Optional exact quote text (enables text-quote anchoring).
        body: Optional annotation body/comment text.
        motivation: Optional motivation (default "commenting", e.g. "assessing", "questioning").
        runtime_dir: Optional path to a .textruntime directory. Defaults to TEXT_RUNTIME_DIR or ~/.textruntime.
    """
    args = ["annotate", doc_id, "--sentence-uuid", sentence_uuid]
    if quote:
        args += ["--quote", quote]
    if body:
        args += ["--body", body]
    if motivation:
        args += ["--motivation", motivation]
    return _run(args, runtime_dir)


@mcp.tool()
def search_corpus(query: str, doc_id: str | None = None, runtime_dir: str | None = None) -> str:
    """Full-text search across the corpus. Returns ranked hits with node type, snippet, and UUIDs.

    Args:
        query: Search query (FTS5 syntax; quote phrases for exact matches).
        doc_id: Optional document UUID to scope the search to one document.
        runtime_dir: Optional path to a .textruntime directory. Defaults to TEXT_RUNTIME_DIR or ~/.textruntime.
    """
    args = ["search", query]
    if doc_id:
        args += ["--doc-id", doc_id]
    return _run(args, runtime_dir)


def main() -> None:
    mcp.run()


if __name__ == "__main__":
    main()
