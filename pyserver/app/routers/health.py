"""探活：Rust 侧据此判定服务可用（停止服务走 stdin EOF 自退，不走 HTTP）。"""

from __future__ import annotations

import os

from fastapi import APIRouter

router = APIRouter()


@router.get("/health")
async def health() -> dict:
    return {"status": "ok", "pid": os.getpid()}
