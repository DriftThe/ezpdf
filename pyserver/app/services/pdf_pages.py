"""PDF bytes → page rasters for the Mistral-shaped /v1/ocr route.

The client's own renders (scale 2.0) are what the layout/merge tuning is calibrated on, so a
document_url request is rendered at EZPDF_OCR_RENDER_DPI (default 144 = 72*2.0) and reported back in
`dimensions`; the caller converts our pixels into its own page space with that ratio.

pypdfium2 is imported lazily: the legacy image routes must keep working on an install that predates it.
"""

from __future__ import annotations

import logging
from typing import Iterable

from PIL import Image

from ..config import DOC_MAX_BYTES, DOC_MAX_PAGES, DOC_MAX_PIXELS, DOC_MAX_SIDE_PX, RENDER_DPI

logger = logging.getLogger("ezpdf.mistral")


class DocumentError(Exception):
    """Client-visible document failure (status is the HTTP code to answer with)."""

    def __init__(self, status: int, detail: str) -> None:
        super().__init__(detail)
        self.status = status
        self.detail = detail


def parse_page_filter(raw: object, total: int) -> list[int]:
    """Mistral `pages`: None = the whole document, a list of ints, or "0,2-4" (0-based, inclusive).

    Out-of-range or malformed input is a 422 rather than a silent clamp: the caller's page ↔ result
    mapping depends on it.
    """
    if raw is None:
        return list(range(total))
    indices: list[int] = []
    if isinstance(raw, str):
        for chunk in raw.split(","):
            part = chunk.strip()
            if not part:
                continue
            if "-" in part:
                left, _, right = part.partition("-")
                try:
                    start, end = int(left), int(right)
                except ValueError as exc:
                    raise DocumentError(422, f"invalid pages filter: {part!r}") from exc
                if end < start:
                    raise DocumentError(422, f"invalid pages range: {part!r}")
                indices.extend(range(start, end + 1))
            else:
                try:
                    indices.append(int(part))
                except ValueError as exc:
                    raise DocumentError(422, f"invalid pages filter: {part!r}") from exc
    elif isinstance(raw, Iterable):
        for value in raw:
            if not isinstance(value, int) or isinstance(value, bool):
                raise DocumentError(422, f"invalid pages entry: {value!r}")
            indices.append(value)
    else:
        raise DocumentError(422, "pages must be a list of integers or a string like \"0,2-4\"")

    deduped = sorted(set(indices))
    if not deduped:
        raise DocumentError(422, "pages filter selects no page")
    for index in deduped:
        if index < 0 or index >= total:
            raise DocumentError(422, f"page {index} out of range (document has {total} pages)")
    return deduped


def render_pdf_pages(data: bytes, indices: list[int], dpi: int = RENDER_DPI) -> list[tuple[int, Image.Image]]:
    """Render the selected 0-based pages; returns (index, RGB image) in the requested order."""
    if not data:
        raise DocumentError(422, "empty document")
    if len(data) > DOC_MAX_BYTES:
        raise DocumentError(413, f"document too large: {len(data)} bytes > {DOC_MAX_BYTES}")
    try:
        import pypdfium2 as pdfium
    except ImportError as exc:  # pragma: no cover - only on an install without the renderer
        raise DocumentError(500, "pypdfium2 is not installed: document_url is unavailable") from exc

    try:
        document = pdfium.PdfDocument(data)
    except Exception as exc:
        raise DocumentError(422, f"failed to open PDF: {exc}") from exc

    try:
        total = len(document)
        if total == 0:
            raise DocumentError(422, "PDF has no pages")
        if total > DOC_MAX_PAGES:
            raise DocumentError(413, f"PDF has too many pages: {total} > {DOC_MAX_PAGES}")
        scale = dpi / 72.0
        out: list[tuple[int, Image.Image]] = []
        for index in indices:
            try:
                page = document[index]
            except Exception as exc:
                raise DocumentError(422, f"failed to load page {index}: {exc}") from exc
            try:
                bitmap = page.render(scale=scale)
                image = bitmap.to_pil().convert("RGB")
            except Exception as exc:
                raise DocumentError(422, f"failed to render page {index}: {exc}") from exc
            width, height = image.size
            if max(width, height) > DOC_MAX_SIDE_PX or width * height > DOC_MAX_PIXELS:
                raise DocumentError(413, f"page {index} renders too large: {width}x{height}")
            out.append((index, image))
        logger.info("rendered %d page(s) at %d dpi from a %d-page document", len(out), dpi, total)
        return out
    finally:
        document.close()
