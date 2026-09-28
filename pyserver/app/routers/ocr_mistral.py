"""Mistral-shaped OCR: POST /v1/ocr (+ /ocr), the wire format other OCR services speak.

One document per request, 0-based absolute page numbers, per-page boxes in the rendered raster
(`dimensions`), blocks typed with Mistral's 13-type vocabulary, and `include_blocks` to ask for them.
Two document forms are accepted: `document_url` (a base64 PDF, rendered here at EZPDF_OCR_RENDER_DPI)
and `image_url` (one already-rendered page image).

Two deliberate departures from Mistral's document, both because they are what makes the answer usable:
the block's own PP-DocLayout label rides along in an extra `label` field (`type` is the Mistral type,
so the official clients still parse the payload), and a page whose VL output was cut at max_new_tokens
fails the whole request instead of answering with half a page.
"""

from __future__ import annotations

import base64
import binascii
import io
import logging
from typing import Any, Optional

from fastapi import APIRouter, HTTPException
from PIL import Image
from pydantic import BaseModel, ConfigDict
from starlette.concurrency import run_in_threadpool

from ..config import DOC_MAX_BYTES, DOC_MAX_PIXELS, DOC_MAX_SIDE_PX, RENDER_DPI
from ..services.engine import engine
from ..services.labels import mistral_block_type
from ..services.pdf_pages import DocumentError, parse_page_filter, render_pdf_pages

router = APIRouter()
logger = logging.getLogger("ezpdf.mistral")

MODEL_NAME = "ezpdf-pyserver"


class DocumentChunk(BaseModel):
    """Mistral's document union; unknown members are ignored (only the two real forms are read)."""

    model_config = ConfigDict(extra="allow")

    type: str = ""
    document_url: Optional[Any] = None
    image_url: Optional[Any] = None
    file: Optional[Any] = None
    mime_type: Optional[str] = None


class OcrRequest(BaseModel):
    # extra="allow": a compatible client must be able to add fields without the request failing
    model_config = ConfigDict(extra="allow")

    model: str = MODEL_NAME
    document: DocumentChunk
    pages: Optional[Any] = None
    include_blocks: bool = True
    include_image_base64: bool = False
    image_limit: Optional[int] = None
    image_min_size: Optional[int] = None


def _data_uri_payload(value: Any, field: str) -> str:
    """`data:...;base64,XXXX` → XXXX; a bare base64 string is accepted as-is."""
    if isinstance(value, dict):  # the TS client sends {"url": "..."}
        value = value.get("url")
    if not isinstance(value, str) or not value.strip():
        raise DocumentError(422, f"document.{field} must be a base64 data URI")
    if value.startswith("http://") or value.startswith("https://"):
        # We never fetch a URL the caller supplied (the service is reachable from other hosts)
        raise DocumentError(422, f"document.{field} must be a data: URI, not a remote URL")
    if value.startswith("data:"):
        _, sep, rest = value.partition(",")
        if not sep:
            raise DocumentError(422, f"malformed document.{field}")
        return rest
    return value


def _decode_base64(payload: str) -> bytes:
    try:
        return base64.b64decode(payload, validate=False)
    except (binascii.Error, ValueError) as exc:
        raise DocumentError(400, f"failed to decode base64 document: {exc}") from exc


def _decode_image(payload: str) -> Image.Image:
    raw = _decode_base64(payload)
    if len(raw) > DOC_MAX_BYTES:
        raise DocumentError(413, f"document too large: {len(raw)} bytes > {DOC_MAX_BYTES}")
    try:
        with Image.open(io.BytesIO(raw)) as image:
            width, height = image.size
            if max(width, height) > DOC_MAX_SIDE_PX or width * height > DOC_MAX_PIXELS:
                raise DocumentError(413, f"image too large: {width}x{height}")
            return image.convert("RGB")
    except DocumentError:
        raise
    except (OSError, ValueError, Image.DecompressionBombError) as exc:
        raise DocumentError(400, f"failed to decode image: {exc}") from exc


def _crop_data_uri(image: Image.Image, rect: tuple[int, int, int, int]) -> str:
    x1, y1, x2, y2 = rect
    crop = image.crop((max(0, x1), max(0, y1), max(0, x2), max(0, y2)))
    buffer = io.BytesIO()
    crop.save(buffer, format="PNG")
    return "data:image/png;base64," + base64.b64encode(buffer.getvalue()).decode()


def _page_json(
    index: int,
    result,
    image: Image.Image,
    req: OcrRequest,
    dpi: int,
) -> dict:
    """PageResult → Mistral page object (blocks in the pipeline's own order, boxes in our pixels).

    `dpi` is always an integer: Mistral's `OCRPageDimensions.dpi` is a required `int`, so a null there
    is not a valid response (verified against mistralai's own model). For an image document the raster
    has no defined physical scale, so the nominal render dpi is reported — the boxes are placed by the
    width/height ratio either way, which is what the coordinates actually mean.
    """
    blocks: list[dict] = []
    images: list[dict] = []
    for region in result.regions:
        x1, y1, x2, y2 = region.rect
        kind = mistral_block_type(region.label)
        block: dict = {
            "type": kind,
            "top_left_x": x1,
            "top_left_y": y1,
            "bottom_right_x": x2,
            "bottom_right_y": y2,
            "content": "",
            # Our own label, so an ezpdf client keeps the 21-class typing (Mistral's 13 are coarser)
            "label": region.label,
        }
        if kind == "image":
            if req.image_min_size and min(x2 - x1, y2 - y1) < req.image_min_size:
                continue
            if req.image_limit is not None and len(images) >= req.image_limit:
                continue
            image_id = f"img-{index}-{len(images)}"
            block["image_id"] = image_id
            entry = {
                "id": image_id,
                "top_left_x": x1,
                "top_left_y": y1,
                "bottom_right_x": x2,
                "bottom_right_y": y2,
            }
            if req.include_image_base64:
                entry["image_base64"] = _crop_data_uri(image, region.rect)
            images.append(entry)
        else:
            block["content"] = region.markdown
        blocks.append(block)

    markdown = "\n\n".join(b["content"] for b in blocks if b["content"]).strip()
    return {
        "index": index,
        "markdown": markdown,
        "dimensions": {"dpi": dpi, "width": result.width, "height": result.height},
        "images": images,
        "blocks": blocks if req.include_blocks else None,
    }


def _select_document(req: OcrRequest) -> tuple[list, list[int], int, int]:
    """→ (page images, their 0-based indexes, document bytes, render dpi) for the requested pages."""
    kind = (req.document.type or "").strip()
    if kind in ("", "document_url"):
        if req.document.document_url is None:
            raise DocumentError(422, "document.document_url is required for type=document_url")
        images, indices, size = _pdf_document(req)
        return images, indices, size, RENDER_DPI
    if kind == "image_url":
        if req.document.image_url is None:
            raise DocumentError(422, "document.image_url is required for type=image_url")
        payload = _data_uri_payload(req.document.image_url, "image_url")
        image = _decode_image(payload)
        # An image is a one-page document; Mistral numbers it page 0. `dpi` is nominal here (the raster
        # has no physical scale), but it must be an integer: the schema requires one.
        indices = parse_page_filter(req.pages, 1)
        if indices != [0]:
            raise DocumentError(422, f"an image document has only page 0 (requested {indices})")
        return [image], [0], 0, RENDER_DPI
    if kind == "file":
        raise DocumentError(422, "document type \"file\" is not supported: send a data: URI instead")
    raise DocumentError(422, f"unsupported document type: {kind!r}")


def _pdf_document(req: OcrRequest) -> tuple[list, list[int], int]:
    payload = _data_uri_payload(req.document.document_url, "document_url")
    data = _decode_base64(payload)
    try:
        import pypdfium2 as pdfium
    except ImportError as exc:  # pragma: no cover - only on an install without the renderer
        raise DocumentError(500, "pypdfium2 is not installed: document_url is unavailable") from exc
    try:
        # Page count first, so a bad `pages` filter is answered before anything is rendered
        document = pdfium.PdfDocument(data)
        try:
            total = len(document)
        finally:
            document.close()
    except Exception as exc:
        raise DocumentError(422, f"failed to open PDF: {exc}") from exc

    indices = parse_page_filter(req.pages, total)
    rendered = render_pdf_pages(data, indices, RENDER_DPI)
    return [image for _, image in rendered], [index for index, _ in rendered], len(data)


@router.post("/v1/ocr")
@router.post("/ocr")
async def ocr_mistral(req: OcrRequest) -> dict:
    try:
        images, indices, doc_size, dpi = await run_in_threadpool(_select_document, req)
        try:
            results = await run_in_threadpool(engine.recognize_batch, images)
        except Exception as exc:
            raise DocumentError(500, f"OCR inference failed: {exc}") from exc
        for index, result in zip(indices, results):
            if result.truncated:
                # Half a page is worse than a failed page: the caller cannot tell a truncated reading
                # from a short one, and its misses would be silent (PROTOCOL.md, "截断即失败").
                raise DocumentError(
                    502,
                    f"OCR page {index} failed: the recognizer hit max_new_tokens, its text is incomplete",
                )
        pages = [
            _page_json(index, result, image, req, dpi)
            for index, result, image in zip(indices, results, images)
        ]
    except DocumentError as exc:
        raise HTTPException(status_code=exc.status, detail=exc.detail) from exc

    logger.info("mistral /v1/ocr: %d page(s) of %d bytes", len(pages), doc_size)
    return {
        "pages": pages,
        "model": req.model or MODEL_NAME,
        "usage_info": {"pages_processed": len(pages), "doc_size_bytes": doc_size or None},
        "object": "ocr",
    }
