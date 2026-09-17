"""Single-page / batch OCR: POST /ocr/page {image_b64} and POST /ocr/pages.

Both decode with PIL (not Wise-Paddle's cv2 file-upload path), in the threadpool and with size
bounded (see _decode_image / MAX_BODY_BYTES). bbox_px is source-image pixels including the unclip
expansion; the px→pt conversion happens in Rust, so this service never sees scale.
The engine is lazy-loaded and serialized by one lock; the first request takes minutes (VL ~1.8GB).
"""

from __future__ import annotations

import base64
import binascii
import io

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, Field
from PIL import Image
from starlette.concurrency import run_in_threadpool

from ..config import MAX_BATCH_PAGES
from ..services.engine import engine
from ..services.pipeline import RegionResult

router = APIRouter()


class PageRequest(BaseModel):
    image_b64: str


class PagesBatchRequest(BaseModel):
    # Cap = the max_batch_pages advertised by /health (the client picks each batch size from it)
    pages: list[PageRequest] = Field(min_length=1, max_length=MAX_BATCH_PAGES)


# Decompression-bomb / oversized-render guard: limits on both side length and total pixels (a normal scale-2.0 page is far below these)
MAX_SIDE_PX = 12_000
MAX_PIXELS = 40_000_000


def _decode_image(payload: str) -> Image.Image:
    if payload.startswith("data:"):
        # partition, not split[1]: a malformed data: string cannot raise IndexError
        _, sep, rest = payload.partition(",")
        payload = rest if sep else ""
    try:
        raw = base64.b64decode(payload)
        with Image.open(io.BytesIO(raw)) as image:
            width, height = image.size
            if max(width, height) > MAX_SIDE_PX or width * height > MAX_PIXELS:
                raise HTTPException(
                    status_code=413,
                    detail=f"image too large: {width}x{height}",
                )
            return image.convert("RGB")
    except HTTPException:
        raise
    except (binascii.Error, ValueError, OSError, Image.DecompressionBombError) as exc:
        raise HTTPException(status_code=400, detail=f"failed to decode image: {exc}") from exc


def _decode_images(pages: list[PageRequest]) -> list[Image.Image]:
    """Batch decode (CPU-bound: run in the threadpool, off the event loop)."""
    return [_decode_image(p.image_b64) for p in pages]


def _region_json(r: RegionResult) -> dict:
    """RegionResult → response block dict (the same contract for /ocr/page and /ocr/pages)."""
    return {
        "label": r.label,
        "score": round(r.score, 4),
        "bbox_px": [r.rect[0], r.rect[1], r.rect[2], r.rect[3]],
        "markdown": r.markdown,
    }


@router.post("/ocr/page")
async def ocr_page(req: PageRequest) -> dict:
    image = await run_in_threadpool(_decode_image, req.image_b64)
    try:
        result = await run_in_threadpool(engine.recognize, image)
    except Exception as exc:
        raise HTTPException(status_code=500, detail=f"OCR inference failed: {exc}") from exc
    return {
        "width": result.width,
        "height": result.height,
        "elapsed": round(result.elapsed_seconds, 3),
        "blocks": [_region_json(r) for r in result.regions],
    }


@router.post("/ocr/pages")
async def ocr_pages(req: PagesBatchRequest) -> dict:
    images = await run_in_threadpool(_decode_images, req.pages)
    try:
        results = await run_in_threadpool(engine.recognize_batch, images)
    except Exception as exc:
        raise HTTPException(status_code=500, detail=f"OCR batch inference failed: {exc}") from exc
    return {
        "elapsed": round(results[0].elapsed_seconds, 3),
        "pages": [
            {
                "width": r.width,
                "height": r.height,
                "elapsed": round(r.elapsed_seconds, 3),
                "blocks": [_region_json(b) for b in r.regions],
            }
            for r in results
        ],
    }
