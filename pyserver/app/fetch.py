"""模型下载器（python -m app.fetch）：huggingface_hub 快照下载，默认走 hf-mirror 镜像（国内免梯）。

Rust 的 ocr_download_models 调用；stdout/stderr 逐行 → ocr://log，退出码 0 = 成功。

- 目标目录：EZPDF_MODELS_DIR（config.py，默认 <pyserver>/models）
- 完整性判定与 bootstrap.py 同约定：config.json + preprocessor_config.json + 任一权重文件
- 已完整的仓库直接跳过；未完整的由 huggingface_hub 断点续传
- HF_ENDPOINT / HF_HUB_DISABLE_PROGRESS_BARS 可用环境变量覆盖
"""

from __future__ import annotations

import os
from pathlib import Path

# 必须在 import huggingface_hub 之前设置（镜像端点在导入期读取）
os.environ.setdefault("HF_ENDPOINT", "https://hf-mirror.com")
# tqdm 进度条以 \r 刷新，经 IPC 管道逐行转发不可见，反而会把整段帧挤成一条脏日志
os.environ.setdefault("HF_HUB_DISABLE_PROGRESS_BARS", "1")

from app.config import LAYOUT_MODEL_DIR_NAME, MODELS_DIR, VL_MODEL_DIR_NAME  # noqa: E402

# 目录名 → hub 仓库；PP-DocLayoutV3 的 safetensors 权重在 _safetensors 子仓（主仓是 paddle 格式）
REPOS: dict[str, str] = {
    LAYOUT_MODEL_DIR_NAME: "PaddlePaddle/PP-DocLayoutV3_safetensors",
    VL_MODEL_DIR_NAME: "PaddlePaddle/PaddleOCR-VL-1.6",
}
WEIGHT_EXTS = (".safetensors", ".bin", ".pth", ".pt", ".msgpack")  # 与 bootstrap.py 一致


def complete(target: Path) -> bool:
    """与 bootstrap.py _model_ok 同口径，避免"探测说缺、下载说齐"的口径分叉。"""
    if not (target / "config.json").is_file() or not (target / "preprocessor_config.json").is_file():
        return False
    return any(p.suffix.lower() in WEIGHT_EXTS for p in target.iterdir() if p.is_file())


def main() -> int:
    try:
        from huggingface_hub import snapshot_download
    except ImportError:
        print("[fetch] huggingface_hub 未安装（应由 ocr_download_models 先装 requirements-download.txt）", flush=True)
        return 2

    missing = [name for name in REPOS if not complete(MODELS_DIR / name)]
    if not missing:
        print("[fetch] 两模型目录均完整，无需下载", flush=True)
        return 0

    for name in missing:
        target = MODELS_DIR / name
        repo = REPOS[name]
        target.mkdir(parents=True, exist_ok=True)
        print(f"[fetch] {repo} → {target}", flush=True)
        try:
            snapshot_download(repo_id=repo, local_dir=target, max_workers=4)
        except Exception as exc:
            print(f"[fetch] {repo} 下载失败: {type(exc).__name__}: {exc}", flush=True)
            return 1
        if not complete(target):
            print(f"[fetch] {repo} 下载完成但校验不通过（缺 config/权重文件）", flush=True)
            return 1
        print(f"[fetch] {repo} 完成", flush=True)

    print("[fetch] 全部模型就绪", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
