"""单页 / 批量 OCR：POST /ocr/page {image_b64, scale} 与 POST /ocr/pages。

- /ocr/page  → {blocks, width, height, elapsed}（curl 冒烟/调试用）；
- /ocr/pages → {elapsed, pages: [{blocks, width, height}]}（Rust parse_pdf 正路，
  前端 parse_append 每批 ≤4 页同书提交，PyService 持有 token 后经此批量推理）；
- 图片解码用 PIL（Wise-Paddle 的 cv2 路径服务于文件上传，这里不需要）；
- bbox_px 为原图像素坐标（含 unclip 扩框），px→PDF pt 换算（pt = px/scale）由
  Rust 写绑定 JSON 时做，本服务不关心 scale；
- 引擎懒加载 + 单锁串行，首个请求耗时以分钟计（VL 模型 ~1.8GB）。
"""

from __future__ import annotations

import base64
import binascii
import io

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, Field
from PIL import Image
from starlette.concurrency import run_in_threadpool

from ..services.engine import engine
from ..services.pipeline import RegionResult

router = APIRouter()


class PageRequest(BaseModel):
    image_b64: str
    scale: float = 2.0


class PagesBatchRequest(BaseModel):
    pages: list[PageRequest] = Field(min_length=1, max_length=32)


def _decode_image(payload: str) -> Image.Image:
    """base64（可带 data: URI 前缀）→ PIL RGB Image。"""
    if payload.startswith("data:"):
        payload = payload.split(",", 1)[1]
    try:
        raw = base64.b64decode(payload)
        image = Image.open(io.BytesIO(raw))
        return image.convert("RGB")
    except (binascii.Error, ValueError, OSError) as exc:
        raise HTTPException(status_code=400, detail=f"failed to decode image: {exc}") from exc


def _region_json(r: RegionResult) -> dict:
    """RegionResult → 响应块 dict（/ocr/page 与 /ocr/pages 共用同一契约）。"""
    return {
        "label": r.label,
        "score": round(r.score, 4),
        "bbox_px": [r.rect[0], r.rect[1], r.rect[2], r.rect[3]],
        "markdown": r.markdown,
    }


@router.post("/ocr/page")
async def ocr_page(req: PageRequest) -> dict:
    image = _decode_image(req.image_b64)
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
    images = [_decode_image(p.image_b64) for p in req.pages]
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
