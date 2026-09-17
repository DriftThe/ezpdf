#!/usr/bin/env python3
"""Developer tool: run a parse "service" locally (default 127.0.0.1:9055) for the app's Online service mode.

Differences from `python -m app.main` (the app-managed in-process mode):
- fixed port, no EZPDF_READY line, does not read stdin, stops on Ctrl+C;
- no token check by default (`--token` enables it to exercise the auth path);
- no env/model readiness check (a missing piece fails the first request; the log says why).

Usage (run with a python that has the same deps as the service, e.g. pyserver/.venv):

    python server_test.py                          # 127.0.0.1:9055
    python server_test.py --host 0.0.0.0 --port 9055
    python server_test.py --token secret           # require x-ezpdf-token: secret
    python server_test.py --reload                 # auto-reload on code change (dev)

Then in the app: Settings → OCR Service → Service Source "Online service" → Service URL http://127.0.0.1:9055
→ click "Test" to probe → click "Start service" to register it as the OCR target.

HTTP contract (ports, request/response fields, coordinate conversion, lifecycle) is in pyserver/PROTOCOL.md.
"""

from __future__ import annotations

import argparse
import os
import sys

# Allow running from any cwd: put the pyserver root (the app package's dir) on sys.path
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
    """Missing pieces only warn; startup is not blocked so you can connect and read the request error."""
    try:
        import torch  # noqa: F401
    except Exception as exc:  # pragma: no cover - dev hint
        print(f"[server_test] warning: torch is not importable ({exc}); run 一键安装服务 first")
    for name in (LAYOUT_MODEL_DIR_NAME, VL_MODEL_DIR_NAME):
        if not os.path.isdir(os.path.join(models_dir, name)):
            print(f"[server_test] warning: model dir missing: {os.path.join(models_dir, name)}")


def main() -> int:
    args = parse_args()
    # Must be set before importing app.*: app/config.py reads the environment at import time
    if args.token:
        os.environ["EZPDF_TOKEN"] = args.token
    if args.models_dir:
        os.environ["EZPDF_MODELS_DIR"] = args.models_dir
    if args.max_batch:
        os.environ["EZPDF_MAX_BATCH_PAGES"] = str(args.max_batch)

    import uvicorn

    from app.config import (
        LAYOUT_MODEL_DIR_NAME,
        MAX_BATCH_PAGES,
        MODELS_DIR,
        TOKEN,
        VL_MODEL_DIR_NAME,
    )

    base = f"http://{args.host}:{args.port}"
    print(f"[server_test] parse service   : {base}  (health: {base}/health)")
    print(f"[server_test] models dir      : {MODELS_DIR}")
    print(f"[server_test] token           : {'required' if TOKEN else 'none (dev)'}")
    print(f"[server_test] max batch pages : {MAX_BATCH_PAGES}  (the app asks /health before each batch)")
    print("[server_test] app side        : Settings → OCR Service → Online service → Service URL")
    warn_incomplete_env(str(MODELS_DIR))

    if args.reload:
        # reload needs an import string (uvicorn re-imports app.main:create_app itself)
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
