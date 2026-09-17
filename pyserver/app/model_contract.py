"""Model directory contract, shared by the probe (bootstrap.py) and the downloader (app.fetch); stdlib only.

Completeness = `config.json` + `preprocessor_config.json` + any weight file. Both callers must agree,
else the probe reports missing while the downloader reports complete.

Lives under app/ (not the repo root) so both bootstrap.py (`sys.path[0]` = script dir) and
`python -m app.*` can import it; app/__init__.py is empty, so no third-party deps leak in.
"""

from __future__ import annotations

from pathlib import Path

# dir name → HF repo mapping is in app/fetch.py (download only); this module owns dir names and completeness
LAYOUT_MODEL_DIR_NAME = "PP-DocLayoutV3"
VL_MODEL_DIR_NAME = "PaddleOCR-VL-1.6"

WEIGHT_EXTS = (".safetensors", ".bin", ".pth", ".pt", ".msgpack")


def model_dir_ok(target: Path) -> bool:
    """True when the dir is complete (config + preprocessor_config + any weight file)."""
    if not target.is_dir():
        return False
    if not (target / "config.json").is_file() or not (target / "preprocessor_config.json").is_file():
        return False
    return any(p.suffix.lower() in WEIGHT_EXTS for p in target.iterdir() if p.is_file())
