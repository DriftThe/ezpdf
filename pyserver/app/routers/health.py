"""探活与优雅退出。"""

from __future__ import annotations

import asyncio
import os

from fastapi import APIRouter, Request

router = APIRouter()


@router.get("/health")
async def health() -> dict:
    return {"status": "ok", "pid": os.getpid()}


@router.post("/shutdown")
async def shutdown(request: Request) -> dict:
    server = getattr(request.app.state, "server", None)
    if server is None:
        return {"ok": False}
    asyncio.get_running_loop().create_task(_delayed_exit(server))
    return {"ok": True}


async def _delayed_exit(server) -> None:
    await asyncio.sleep(0.2)  # 先让响应回去
    server.should_exit = True
