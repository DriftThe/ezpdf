"""模型目录契约：探测（bootstrap.py）与下载（app.fetch）共用，纯标准库。

完整性口径 = 目录里有 `config.json` + `preprocessor_config.json` + 任一权重文件。
两处判定必须一致，否则会出现「探测说缺、下载说齐」的口径分叉（两个入口都从这里取）。

放在 app/ 下而不是仓库根，是为了让 bootstrap.py（`sys.path[0]` = 脚本目录）和
`python -m app.*` 都能 import；app/__init__.py 是空的，所以这里不会拖进第三方依赖。
"""

from __future__ import annotations

from pathlib import Path

# 目录名 → HF 仓库的仓库名在 app/fetch.py（下载专用），这里只管目录名与完整性
LAYOUT_MODEL_DIR_NAME = "PP-DocLayoutV3"
VL_MODEL_DIR_NAME = "PaddleOCR-VL-1.6"

WEIGHT_EXTS = (".safetensors", ".bin", ".pth", ".pt", ".msgpack")


def model_dir_ok(target: Path) -> bool:
    """目录完整（config + preprocessor_config + 任一权重文件）→ True"""
    if not target.is_dir():
        return False
    if not (target / "config.json").is_file() or not (target / "preprocessor_config.json").is_file():
        return False
    return any(p.suffix.lower() in WEIGHT_EXTS for p in target.iterdir() if p.is_file())
