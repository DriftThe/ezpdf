"""Container/server entry: fixed host:port + always-on token-file auth, for Docker and bare-metal.
Unlike app.main (Rust-managed): binds EZPDF_HOST:EZPDF_PORT (default 0.0.0.0:9055) instead of an
ephemeral port, takes no stdin EOF (a container's stdin is /dev/null, so app.main's watchdog would
kill it at once), prints no EZPDF_READY (no Rust parent), and resolves its own token
(EZPDF_TOKEN > EZPDF_TOKEN_FILE > generate+persist, auth always on), printing it on every start
so it can be copied into the client's Service Token field.

    python -m app.server_docker   # EZPDF_PORT / EZPDF_TOKEN_FILE override the defaults
"""

from __future__ import annotations

import logging
import os
import sys

import uvicorn

from .config import MAX_BATCH_PAGES, MODELS_DIR, TOKEN_FILE, resolve_token
from .main import create_app

logger = logging.getLogger("ezpdf.pyserver")


def _host() -> str:
    return os.environ.get("EZPDF_HOST", "0.0.0.0").strip() or "0.0.0.0"


def _port() -> int:
    """Port: malformed/out-of-range values fall back to 9055 (matching docker-compose's exposed port)."""
    raw = os.environ.get("EZPDF_PORT", "").strip()
    try:
        value = int(raw)
    except ValueError:
        return 9055
    return value if 1 <= value <= 65535 else 9055


def serve() -> None:
    logging.basicConfig(
        stream=sys.stderr,
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s — %(message)s",
    )
    token, generated = resolve_token()
    host, port = _host(), _port()
    if generated:
        logger.info("generated auth token → %s", TOKEN_FILE)
    elif os.environ.get("EZPDF_TOKEN"):
        logger.info("auth token from EZPDF_TOKEN env")
    else:
        logger.info("auth token loaded from %s", TOKEN_FILE)
    # Always print in plaintext: the only place the deployer can read it
    logger.info("auth token: %s", token)
    logger.info("models dir: %s (page batch limit %d)", MODELS_DIR, MAX_BATCH_PAGES)
    logger.info("listening on http://%s:%d", host, port)
    uvicorn.run(create_app(token), host=host, port=port, log_level="info", access_log=False)


if __name__ == "__main__":
    serve()
