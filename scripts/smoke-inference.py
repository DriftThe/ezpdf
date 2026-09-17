"""Minimal inference for image smoke tests: draw a page -> run real OCR -> assert regions.

CI (.github/workflows/pyserver-images.yml) runs this inside the container to prove the image
can actually infer, since /health only proves the service started. Run from the pyserver
directory (cwd is /app in the container and the script dir is not on sys.path, so add cwd):

    cd pyserver && ./.venv/Scripts/python.exe ../scripts/smoke-inference.py
    docker run --rm -v "$PWD/scripts/smoke-inference.py:/smoke.py:ro" ezpdf-pyserver:cpu python /smoke.py

Exit code 0 = pass; failures print a reason and return 1.
"""

from __future__ import annotations

import os
import sys
import time

# sys.path[0] is the script dir (scripts/) or / in the container; add cwd so app imports work
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
