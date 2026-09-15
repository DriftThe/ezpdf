"""pyserver 运行配置：全部经环境变量注入（Rust spawn 时传入），无配置文件。

- EZPDF_TOKEN        会话 token；为空则不做校验（仅手动调试）
- EZPDF_MODELS_DIR   模型根目录（默认 <pyserver>/models）
"""

from __future__ import annotations

import os
from pathlib import Path

# pyserver 根目录（app/ 的上一级）
ROOT = Path(__file__).resolve().parents[1]

TOKEN = os.environ.get("EZPDF_TOKEN", "")
MODELS_DIR = Path(os.environ.get("EZPDF_MODELS_DIR", str(ROOT / "models")))

LAYOUT_MODEL_DIR_NAME = "PP-DocLayoutV3"
VL_MODEL_DIR_NAME = "PaddleOCR-VL-1.6"
