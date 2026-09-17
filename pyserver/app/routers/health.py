"""Liveness; Rust uses it to gate startup (stopping goes through stdin EOF, not HTTP).
`max_batch_pages` is the batch size the client handshakes before every OCR batch (PROTOCOL.md §4);
keep this a pure in-memory reply and never do heavy work in /health.
"""

from __future__ import annotations

import os

from fastapi import APIRouter

from ..config import MAX_BATCH_PAGES

router = APIRouter()


@router.get("/health")
async def health() -> dict:
    return {"status": "ok", "pid": os.getpid(), "max_batch_pages": MAX_BATCH_PAGES}
