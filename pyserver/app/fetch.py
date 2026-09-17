"""Model downloader (`python -m app.fetch`): huggingface_hub snapshot into EZPDF_MODELS_DIR, hf-mirror by default.
Rust's ocr_download_models calls this (stdout/stderr → ocr://log; exit 0 = success); completeness comes
from app/model_contract.py, complete repos are skipped and incomplete ones resume.
"""

from __future__ import annotations

import os

# Must be set before importing huggingface_hub (the endpoint is read at import time)
os.environ.setdefault("HF_ENDPOINT", "https://hf-mirror.com")
# tqdm bars refresh with \r; forwarded line-by-line over IPC they just smear into one dirty log line
os.environ.setdefault("HF_HUB_DISABLE_PROGRESS_BARS", "1")

from app.config import LAYOUT_MODEL_DIR_NAME, MODELS_DIR, VL_MODEL_DIR_NAME  # noqa: E402
from app.model_contract import model_dir_ok  # noqa: E402

# dir name → hub repo; PP-DocLayoutV3's safetensors weights live in the _safetensors sub-repo (main repo is paddle format)
REPOS: dict[str, str] = {
    LAYOUT_MODEL_DIR_NAME: "PaddlePaddle/PP-DocLayoutV3_safetensors",
    VL_MODEL_DIR_NAME: "PaddlePaddle/PaddleOCR-VL-1.6",
}
def main() -> int:
    try:
        from huggingface_hub import snapshot_download
    except ImportError:
        print("[fetch] huggingface_hub not installed (ocr_download_models should install requirements-download.txt first)", flush=True)
        return 2

    missing = [name for name in REPOS if not model_dir_ok(MODELS_DIR / name)]
    if not missing:
        print("[fetch] both model directories complete, nothing to download", flush=True)
        return 0

    for name in missing:
        target = MODELS_DIR / name
        repo = REPOS[name]
        target.mkdir(parents=True, exist_ok=True)
        print(f"[fetch] {repo} → {target}", flush=True)
        try:
            snapshot_download(repo_id=repo, local_dir=target, max_workers=4)
        except Exception as exc:
            print(f"[fetch] {repo} download failed: {type(exc).__name__}: {exc}", flush=True)
            return 1
        if not model_dir_ok(target):
            print(f"[fetch] {repo} download finished but verification failed (missing config/weight file)", flush=True)
            return 1
        print(f"[fetch] {repo} complete", flush=True)

    print("[fetch] all models ready", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
