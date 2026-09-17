"""Liveness: Rust uses it to decide the service is up (stopping goes through stdin EOF, not HTTP).

The response carries `max_batch_pages`: the client handshakes before every OCR batch to pick the batch
size (in online mode the server dictates it; see PROTOCOL.md §4). Keep this a pure in-memory reply —
never do heavy work in /health.
"""

from __future__ import annotations

import os

from fastapi import APIRouter

from ..config import MAX_BATCH_PAGES

router = APIRouter()


@router.get("/health")
async def health() -> dict:
    return {"status": "ok", "pid": os.getpid(), "max_batch_pages": MAX_BATCH_PAGES}
