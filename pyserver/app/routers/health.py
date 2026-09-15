"""探活：Rust 侧据此判定服务可用（停止服务走 stdin EOF 自退，不走 HTTP）。

响应里带 `max_batch_pages`：客户端**每次 OCR 请求前**都会先握手读它，据此决定一批发几页
（用户 2026-09-15：在线模式下批大小由服务端说了算，见 PROTOCOL.md §4）。所以这里必须
是纯内存应答——不要在 /health 里做任何重活。
"""

from __future__ import annotations

import os

from fastapi import APIRouter

from ..config import MAX_BATCH_PAGES

router = APIRouter()


@router.get("/health")
async def health() -> dict:
    return {"status": "ok", "pid": os.getpid(), "max_batch_pages": MAX_BATCH_PAGES}
