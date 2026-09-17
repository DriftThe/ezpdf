"""pyserver runtime config, all from the environment (Rust injects it on spawn).

EZPDF_TOKEN overrides EZPDF_TOKEN_FILE (default <pyserver>/token.txt, used by server_docker.py);
EZPDF_MODELS_DIR defaults to <pyserver>/models; EZPDF_MAX_BATCH_PAGES (1..32, default 32) is
advertised via /health for the client's batch size (PROTOCOL.md §4/§6).
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
    """Server-form token resolution: env var > token file > generate and persist (0600), so restarts reuse it."""
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
