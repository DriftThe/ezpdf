"""pyserver 运行配置：全部经环境变量注入（Rust spawn 时传入），无配置文件。

- EZPDF_TOKEN        会话 token；为空则不做校验（仅手动调试）
- EZPDF_MODELS_DIR   模型根目录（默认 <pyserver>/models）
- EZPDF_MAX_BATCH_PAGES
                    单次 /ocr/pages 允许的最大页数（默认 32）。客户端请求前会先读
                    /health 里的这个值来决定一批发几页（见 PROTOCOL.md §4/§6）
"""

from __future__ import annotations

import os
from pathlib import Path

# pyserver 根目录（app/ 的上一级）
ROOT = Path(__file__).resolve().parents[1]

TOKEN = os.environ.get("EZPDF_TOKEN", "")
MODELS_DIR = Path(os.environ.get("EZPDF_MODELS_DIR", str(ROOT / "models")))


def _max_batch_pages() -> int:
    """批大小上限：畸形/越界值一律回落默认（协议里客户端会再夹一次 1..32）"""
    raw = os.environ.get("EZPDF_MAX_BATCH_PAGES", "").strip()
    try:
        value = int(raw)
    except ValueError:
        return 32
    return value if 1 <= value <= 32 else 32


MAX_BATCH_PAGES = _max_batch_pages()

LAYOUT_MODEL_DIR_NAME = "PP-DocLayoutV3"
VL_MODEL_DIR_NAME = "PaddleOCR-VL-1.6"
