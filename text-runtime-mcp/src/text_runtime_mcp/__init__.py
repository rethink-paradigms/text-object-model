from .server import mcp

__all__ = ["mcp"]


def main() -> None:
    mcp.run()
