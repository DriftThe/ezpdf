"""pyserver runtime config: all values come from the environment (Rust injects them on spawn); no config file.

- EZPDF_TOKEN            session token; empty disables auth (manual debug / client-generated local mode)
- EZPDF_TOKEN_FILE       token file path (default <pyserver>/token.txt); server_docker.py only — the
                         deployed form has no client to generate a token, so it reads/creates this file
- EZPDF_MODELS_DIR       model root (default <pyserver>/models)
- EZPDF_MAX_BATCH_PAGES  max pages per /ocr/pages (default 32); the client reads it from /health to pick a
                         batch size (see PROTOCOL.md §4/§6)
"""

from __future__ import annotations

import os
import secrets
from pathlib import Path

from . import model_contract as _contract

# pyserver root (parent of app/)
ROOT = Path(__file__).resolve().parents[1]

TOKEN = os.environ.get("EZPDF_TOKEN", "")
TOKEN_FILE = Path(os.environ.get("EZPDF_TOKEN_FILE", str(ROOT / "token.txt")))
MODELS_DIR = Path(os.environ.get("EZPDF_MODELS_DIR", str(ROOT / "models")))


def resolve_token() -> tuple[str, bool]:
    """Server-form token resolution: env var > token file > generate and persist.

    Returns (token, generated). A generated token is written to TOKEN_FILE
    (0600 on POSIX) so restarts and container rebuilds reuse it.
    """
    if TOKEN:
        return TOKEN, False
    try:
        existing = TOKEN_FILE.read_text(encoding="utf-8").strip()
    except OSError:
        existing = ""
    if existing:
        return existing, False
    token = secrets.token_urlsafe(24)
    try:
        TOKEN_FILE.parent.mkdir(parents=True, exist_ok=True)
        TOKEN_FILE.write_text(token + "\n", encoding="utf-8")
        os.chmod(TOKEN_FILE, 0o600)
    except OSError:
        pass
    return token, True


def _max_batch_pages() -> int:
    """Batch cap: malformed/out-of-range values fall back to the default (the client clamps to 1..32 again)."""
    raw = os.environ.get("EZPDF_MAX_BATCH_PAGES", "").strip()
    try:
        value = int(raw)
    except ValueError:
        return 32
    return value if 1 <= value <= 32 else 32


MAX_BATCH_PAGES = _max_batch_pages()

# Directory names and completeness rules live in model_contract.py (shared by probe/download); re-exported here.
LAYOUT_MODEL_DIR_NAME = _contract.LAYOUT_MODEL_DIR_NAME
VL_MODEL_DIR_NAME = _contract.VL_MODEL_DIR_NAME
