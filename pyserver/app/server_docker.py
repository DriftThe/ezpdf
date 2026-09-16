"""容器/服务端部署入口：固定 host:port + token 文件鉴权，供 Docker 与裸机部署使用。

与 app.main（Rust 托管形态）的区别：
    - 绑定 EZPDF_HOST:EZPDF_PORT（默认 0.0.0.0:9055），不从 stdin 收 EOF 退出信号
      （容器里 stdin 是 /dev/null，app.main 的看门狗会立刻自杀），也不打 EZPDF_READY
      行（那是给 Rust 父进程读的就绪协议，这里没有父进程）。
    - token 由服务端自己解析：EZPDF_TOKEN 环境变量 > EZPDF_TOKEN_FILE（默认
      <pyserver>/token.txt）> 生成随机令牌并落盘。**该入口一律开启校验**。
    - 每次启动都把 token 打到终端，方便复制到客户端的「服务令牌」输入框。

用法：
    python -m app.server_docker
    EZPDF_PORT=9055 EZPDF_TOKEN_FILE=/data/token.txt python -m app.server_docker
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
    """端口：畸形/越界值回落 9055（与 docker-compose 暴露的端口一致）"""
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
    # 每次都打印明文：部署方唯一的取值处
    logger.info("auth token: %s", token)
    logger.info("models dir: %s (page batch limit %d)", MODELS_DIR, MAX_BATCH_PAGES)
    logger.info("listening on http://%s:%d", host, port)
    uvicorn.run(create_app(token), host=host, port=port, log_level="info", access_log=False)


if __name__ == "__main__":
    serve()
