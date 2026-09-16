"""镜像冒烟测试用的最小推理：合成一页图 → 跑一次真实 OCR → 断言拿到内容块。

CI（.github/workflows/pyserver-images.yml）把它挂进容器执行，也就是「镜像能不能真的推理」
这件事的判据 —— 只探 /health 只能证明服务起来了。**从 pyserver 目录运行**（容器里的 cwd 就是
/app，脚本目录本身不在 sys.path 上，所以下面显式补上 cwd）：

    cd pyserver && ./.venv/Scripts/python.exe ../scripts/smoke-inference.py
    docker run --rm -v "$PWD/scripts/smoke-inference.py:/smoke.py:ro" ezpdf-pyserver:cpu python /smoke.py

退出码 0 = 通过；失败打印原因并返回 1。
"""

from __future__ import annotations

import os
import sys
import time

# 按路径执行脚本时 sys.path[0] 是脚本所在目录（scripts/），容器里则是 /，都要 cwd 才能 import app
sys.path.insert(0, os.getcwd())


def main() -> int:
    try:
        from PIL import Image, ImageDraw
        from app.services.engine import engine
    except ImportError as exc:
        print(f"[smoke] import failed: {type(exc).__name__}: {exc}")
        return 1

    img = Image.new("RGB", (900, 700), "white")
    draw = ImageDraw.Draw(img)
    draw.rectangle([60, 60, 840, 240], outline="black", width=3)
    draw.text((90, 110), "Design outdoor temperature 2026", fill="black")
    draw.text((90, 320), "Second line of text for layout detection", fill="black")

    started = time.time()
    result = engine.recognize_batch([img])[0]
    elapsed = time.time() - started

    if result.width != 900 or result.height != 700:
        print(f"[smoke] unexpected page size {result.width}x{result.height}")
        return 1
    if not result.regions:
        print("[smoke] inference returned no regions")
        return 1

    print(f"[smoke] inference ok: {len(result.regions)} region(s) in {elapsed:.1f}s")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
