"""服务入口：uvicorn --port 0 + READY 协议 + token 中间件 + stdin-EOF 防孤儿。

Rust 侧（唯一正常调用方）：
    spawn .venv 的 python -m app.main（cwd = pyserver 根，EZPDF_TOKEN 必传），
    逐行读 stdout 直到 `EZPDF_READY {...}` 行，从 JSON 取实际端口。

就绪协议：绑定 ephemeral 端口后、开始 serve 前向 stdout 打一行
    EZPDF_READY {"port": ..., "pid": ...}

手动调试：python -m app.main（无 token 时不校验；Ctrl+C 退出）。
"""

from __future__ import annotations

import json
import logging
import os
import sys
import threading

import uvicorn
from fastapi import FastAPI
from fastapi.responses import JSONResponse

from .config import TOKEN
from .routers import env, health, ocr

logger = logging.getLogger("ezpdf.pyserver")


def create_app() -> FastAPI:
    app = FastAPI(title="ezpdf-pyserver", lifespan=None)
    app.include_router(health.router)
    app.include_router(env.router)
    app.include_router(ocr.router)

    if TOKEN:
        @app.middleware("http")
        async def _token_guard(request, call_next):
            if request.headers.get("x-ezpdf-token") != TOKEN:
                return JSONResponse(status_code=403, content={"detail": "forbidden"})
            return await call_next(request)

    return app


def _watch_stdin(server: uvicorn.Server) -> None:
    """父进程（Rust）退出 → stdin EOF → 优雅退出，5s 兜底硬退（防孤儿）。

    用 os.read 裸读 fd 0 而非 sys.stdin：buffered reader 的锁会在解释器
    收尾时与仍阻塞在读上的守护线程相撞（Fatal Python error）。
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
