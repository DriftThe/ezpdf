"""pyserver 运行配置：全部经环境变量注入（Rust spawn 时传入），无配置文件。

- EZPDF_TOKEN        会话 token；为空则不做校验（仅手动调试 / 本地托管由客户端生成）
- EZPDF_TOKEN_FILE   token 文件路径（默认 <pyserver>/token.txt）；仅 server_docker.py 用，
                     服务端部署形态下没有客户端生成 token，改为读/生成这个文件
- EZPDF_MODELS_DIR   模型根目录（默认 <pyserver>/models）
- EZPDF_MAX_BATCH_PAGES
                    单次 /ocr/pages 允许的最大页数（默认 32）。客户端请求前会先读
                    /health 里的这个值来决定一批发几页（见 PROTOCOL.md §4/§6）
"""

from __future__ import annotations

import os
import secrets
from pathlib import Path

from . import model_contract as _contract

# pyserver 根目录（app/ 的上一级）
ROOT = Path(__file__).resolve().parents[1]

TOKEN = os.environ.get("EZPDF_TOKEN", "")
TOKEN_FILE = Path(os.environ.get("EZPDF_TOKEN_FILE", str(ROOT / "token.txt")))
MODELS_DIR = Path(os.environ.get("EZPDF_MODELS_DIR", str(ROOT / "models")))


def resolve_token() -> tuple[str, bool]:
    """服务端形态的 token 解析：环境变量 > token 文件 > 生成并落盘。

    返回 (token, generated)。生成时写入 TOKEN_FILE（POSIX 下 0600），供部署方重启复用；
    客户端记下的令牌不会因为重启/重建容器而失效。
    """
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
    """批大小上限：畸形/越界值一律回落默认（协议里客户端会再夹一次 1..32）"""
    raw = os.environ.get("EZPDF_MAX_BATCH_PAGES", "").strip()
    try:
        value = int(raw)
    except ValueError:
        return 32
    return value if 1 <= value <= 32 else 32


MAX_BATCH_PAGES = _max_batch_pages()

# 目录名与完整性口径在 model_contract.py（探测/下载共用同一份），这里转出便于既有调用点
LAYOUT_MODEL_DIR_NAME = _contract.LAYOUT_MODEL_DIR_NAME
VL_MODEL_DIR_NAME = _contract.VL_MODEL_DIR_NAME
