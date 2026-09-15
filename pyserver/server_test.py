#!/usr/bin/env python3
"""开发者用：在本机起一个"解析服务器"（默认 127.0.0.1:9055），供应用「在线服务」模式联调。

与 `python -m app.main`（应用托管的进程内模式）的区别：
- 固定端口、不打 EZPDF_READY、不读 stdin，Ctrl+C 停；
- 默认不校验 token（`--token` 可打开，用于验证鉴权路径）；
- 不做环境/模型就绪检查（缺件时首个请求会失败，日志里有原因）。

用法（用与服务同一套依赖的 python 跑，例如 pyserver/.venv）：

    python server_test.py                          # 127.0.0.1:9055
    python server_test.py --host 0.0.0.0 --port 9055
    python server_test.py --token secret           # 要求 x-ezpdf-token: secret
    python server_test.py --reload                 # 改代码自动重载（开发）

然后在应用里：设置 → OCR 服务 → 服务来源「在线服务」→ 服务地址 http://127.0.0.1:9055
→ 点「测试」探活 → 点「启动服务」登记为 OCR 目标。

HTTP 契约（端口、请求/响应字段、坐标换算、生命周期）见 pyserver/PROTOCOL.md。
"""

from __future__ import annotations

import argparse
import os
import sys

# 允许从任意 cwd 运行：pyserver 根（app 包所在目录）进 sys.path
ROOT = os.path.dirname(os.path.abspath(__file__))
if ROOT not in sys.path:
    sys.path.insert(0, ROOT)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run a local ezpdf parse service (development)")
    parser.add_argument("--host", default="127.0.0.1", help="bind address (default 127.0.0.1)")
    parser.add_argument("--port", type=int, default=9055, help="port (default 9055)")
    parser.add_argument("--token", default="", help="require this x-ezpdf-token (default: no auth)")
    parser.add_argument(
        "--max-batch",
        type=int,
        default=0,
        help="advertise this max batch size in /health (default: service default, 32)",
    )
    parser.add_argument("--models-dir", default="", help="override EZPDF_MODELS_DIR")
    parser.add_argument("--reload", action="store_true", help="auto-reload when the code changes")
    return parser.parse_args()


def warn_incomplete_env(models_dir: str) -> None:
    """缺件只提示，不阻止启动——方便先连上再看请求报错（正常由应用端一键安装服务）"""
    try:
        import torch  # noqa: F401
    except Exception as exc:  # pragma: no cover - 开发提示
        print(f"[server_test] warning: torch is not importable ({exc}); run 一键安装服务 first")
    for name in ("PP-DocLayoutV3", "PaddleOCR-VL-1.6"):
        if not os.path.isdir(os.path.join(models_dir, name)):
            print(f"[server_test] warning: model dir missing: {os.path.join(models_dir, name)}")


def main() -> int:
    args = parse_args()
    # 必须在导入 app.* 之前设置：app/config.py 在导入时读环境变量
    if args.token:
        os.environ["EZPDF_TOKEN"] = args.token
    if args.models_dir:
        os.environ["EZPDF_MODELS_DIR"] = args.models_dir
    if args.max_batch:
        os.environ["EZPDF_MAX_BATCH_PAGES"] = str(args.max_batch)

    import uvicorn

    from app.config import MAX_BATCH_PAGES, MODELS_DIR, TOKEN

    base = f"http://{args.host}:{args.port}"
    print(f"[server_test] parse service   : {base}  (health: {base}/health)")
    print(f"[server_test] models dir      : {MODELS_DIR}")
    print(f"[server_test] token           : {'required' if TOKEN else 'none (dev)'}")
    print(f"[server_test] max batch pages : {MAX_BATCH_PAGES}  (the app asks /health before each batch)")
    print("[server_test] app side        : Settings → OCR Service → Online service → Service URL")
    warn_incomplete_env(str(MODELS_DIR))

    if args.reload:
        # reload 需要 import string（uvicorn 自行 re-import app.main:create_app）
        uvicorn.run(
            "app.main:create_app",
            factory=True,
            host=args.host,
            port=args.port,
            log_level="info",
            access_log=True,
            reload=True,
        )
    else:
        from app.main import create_app

        uvicorn.run(create_app(), host=args.host, port=args.port, log_level="info", access_log=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
