"""Managed service entry: uvicorn on port 0 + READY protocol + token middleware + stdin-EOF orphan guard.

Rust side (the only normal caller):
    spawns .venv's python -m app.main (cwd = pyserver root, EZPDF_TOKEN always passed),
    reads stdout line by line until the `EZPDF_READY {...}` line, and takes the real port from it.

Ready protocol: after binding an ephemeral port and before serving, print one stdout line
    EZPDF_READY {"port": ..., "pid": ...}

Manual debug: python -m app.main (no token = no auth; Ctrl+C to exit).
"""

from __future__ import annotations

import hmac
import json
import logging
import os
import sys
import threading

import uvicorn
from fastapi import FastAPI
from fastapi.responses import JSONResponse

from .config import TOKEN
from .routers import health, ocr

logger = logging.getLogger("ezpdf.pyserver")

# Per-request body cap (a batch is ≤4 pages, 32 is the hard cap; leaves plenty of headroom)
MAX_BODY_BYTES = 64 * 1024 * 1024


def create_app(token: str | None = None) -> FastAPI:
    """token=None reads the environment (Rust-managed form); an explicit token wins (server_docker.py).

    Auth covers every route including /health: a client with a token must send `x-ezpdf-token` to health-probe too.
    """
    expected = TOKEN if token is None else token
    app = FastAPI(title="ezpdf-pyserver")
    app.include_router(health.router)
    app.include_router(ocr.router)

    if expected:
        @app.middleware("http")
        async def _token_guard(request, call_next):
            # Constant-time compare (a local process could otherwise byte-probe via response timing)
            supplied = (request.headers.get("x-ezpdf-token") or "").encode("utf-8", "ignore")
            if not hmac.compare_digest(supplied, expected.encode()):
                return JSONResponse(status_code=403, content={"detail": "forbidden"})
            return await call_next(request)

    @app.middleware("http")
    async def _body_limit(request, call_next):
        # 32 pages of base64 PNG sit far below this; Pydantic's max_length only applies after the body is
        # read into memory, so reject on Content-Length before reading — an oversized body can OOM first
        declared = request.headers.get("content-length")
        if declared and declared.isdigit() and int(declared) > MAX_BODY_BYTES:
            logger.warning("request body too large: %s bytes", declared)
            return JSONResponse(status_code=413, content={"detail": "request body too large"})
        return await call_next(request)

    return app


def _watch_stdin(server: uvicorn.Server) -> None:
    """Parent (Rust) exit → stdin EOF → graceful exit, with a 5s hard-exit fallback against orphans.

    Reads fd 0 with os.read rather than sys.stdin: the buffered reader's lock collides with the
    still-blocked daemon thread during interpreter shutdown (Fatal Python error).
    """
    try:
        while os.read(0, 4096):
            pass
    except OSError:
        pass
    logger.info("stdin EOF — parent gone, shutting down")
    server.should_exit = True
    threading.Timer(5.0, os._exit, args=(0,)).start()

def serve() -> None:
    logging.basicConfig(
        stream=sys.stderr, level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s — %(message)s",
    )
    app = create_app()
    config = uvicorn.Config(app, host="127.0.0.1", port=0, log_level="info", access_log=False)
    server = uvicorn.Server(config)
    app.state.server = server

    original_startup = server.startup

    async def _startup(sockets=None) -> None:
        await original_startup(sockets=sockets)
        port = server.servers[0].sockets[0].getsockname()[1]
        print("EZPDF_READY " + json.dumps({"port": port, "pid": os.getpid()}), flush=True)

    server.startup = _startup  # type: ignore[method-assign]

    threading.Thread(target=_watch_stdin, args=(server,), daemon=True).start()
    server.run()


if __name__ == "__main__":
    serve()
