"""pyserver runtime config, all from the environment (Rust injects it on spawn).

EZPDF_TOKEN overrides EZPDF_TOKEN_FILE (default <pyserver>/token.txt, used by server_docker.py);
EZPDF_MODELS_DIR defaults to <pyserver>/models; EZPDF_MAX_BATCH_PAGES (1..32, default 32) is
advertised via /health for the client's batch size (PROTOCOL.md §4/§6).

The OCR tuning block at the bottom is the single place for every layout/merge/OCR number — the
defaults are the calibrated ones, and each EZPDF_OCR_* env var overrides one knob for experiments
(a pyserver process reads them at import, so a change needs a service restart either way).
"""

from __future__ import annotations

import math
import os
import secrets
from pathlib import Path

from . import model_contract as _contract

# pyserver root (parent of app/)
ROOT = Path(__file__).resolve().parents[1]

TOKEN = os.environ.get("EZPDF_TOKEN", "")
TOKEN_FILE = Path(os.environ.get("EZPDF_TOKEN_FILE", str(ROOT / "token.txt")))
MODELS_DIR = Path(os.environ.get("EZPDF_MODELS_DIR", str(ROOT / "models")))


def resolve_token() -> tuple[str, bool]:
    """Server-form token resolution: env var > token file > generate and persist (0600), so restarts reuse it."""
    if TOKEN:
        return TOKEN, False
    try:
        existing = TOKEN_FILE.read_text(encoding="utf-8").strip()
    except OSError:
        existing = ""
    if existing:
        return existing, False
    token = secrets.token_urlsafe(24)
    try:
        TOKEN_FILE.parent.mkdir(parents=True, exist_ok=True)
        TOKEN_FILE.write_text(token + "\n", encoding="utf-8")
        os.chmod(TOKEN_FILE, 0o600)
    except OSError:
        pass
    return token, True


def _max_batch_pages() -> int:
    """Batch cap: malformed/out-of-range values fall back to the default (the client clamps to 1..32 again)."""
    raw = os.environ.get("EZPDF_MAX_BATCH_PAGES", "").strip()
    try:
        value = int(raw)
    except ValueError:
        return 32
    return value if 1 <= value <= 32 else 32


MAX_BATCH_PAGES = _max_batch_pages()

# Directory names and completeness rules live in model_contract.py (shared by probe/download); re-exported here.
LAYOUT_MODEL_DIR_NAME = _contract.LAYOUT_MODEL_DIR_NAME
VL_MODEL_DIR_NAME = _contract.VL_MODEL_DIR_NAME


def _env_float(name: str, default: float) -> float:
    """Env override for a tuning knob; malformed/non-finite values fall back (never break a spawn)."""
    try:
        value = float(os.environ.get(name, "").strip())
    except ValueError:
        return default
    return value if math.isfinite(value) else default


def _env_int(name: str, default: int) -> int:
    return int(_env_float(name, float(default)))


def _env_bool(name: str, default: bool) -> bool:
    raw = os.environ.get(name, "").strip().lower()
    if raw in {"1", "true", "yes", "on"}:
        return True
    if raw in {"0", "false", "no", "off"}:
        return False
    return default


# ==================================================================================================
# OCR tuning (read by engine.py). Pixel values are calibrated for the render scale 2.0 the frontend
# sends (src/lib/pageCapture.ts): a scale-2.0 A4 page is 1190x1684 px.
# ==================================================================================================

# --- pass 1, the main detection (no preprocessing) ---
LAYOUT_SCORE_THRESHOLD = _env_float("EZPDF_OCR_LAYOUT_THRESHOLD", 0.5)
# VRAM guard only: the layout processor resizes its input to 800x800 regardless, so this just needs to sit
# above the page the frontend renders (scale 2.0 A4 = 1190x1684) — below it every page is resampled twice
# (bilinear down to the cap, then to 800) and the detector sees a slightly softer page for no gain.
LAYOUT_MAX_LONG_SIDE = _env_int("EZPDF_OCR_MAX_LONG_SIDE", 2048)

# --- pass 2: sharpened full image, text family only (pdfjs text scores ~0.1-0.15 below pdfium) ---
FALLBACK_SCORE_THRESHOLD = _env_float("EZPDF_OCR_FALLBACK_THRESHOLD", 0.45)
FALLBACK_SHARPEN = (
    _env_int("EZPDF_OCR_SHARPEN_RADIUS", 2),
    _env_int("EZPDF_OCR_SHARPEN_PERCENT", 300),
    _env_int("EZPDF_OCR_SHARPEN_THRESHOLD", 1),
)
FALLBACK_IOU = _env_float("EZPDF_OCR_FALLBACK_IOU", 0.3)
FALLBACK_CONTAINMENT = _env_float("EZPDF_OCR_FALLBACK_CONTAINMENT", 0.6)

# --- pass 3: sharpened half-page tiles at a lower threshold (small text survives the squash) ---
TILE_SCORE_THRESHOLD = _env_float("EZPDF_OCR_TILE_THRESHOLD", 0.38)
TILE_OVERLAP = _env_float("EZPDF_OCR_TILE_OVERLAP", 0.08)
TILE_IOU = _env_float("EZPDF_OCR_TILE_IOU", 0.3)
TILE_CONTAINMENT = _env_float("EZPDF_OCR_TILE_CONTAINMENT", 0.6)
TILE_MIN_PAGE_HEIGHT = _env_int("EZPDF_OCR_TILE_MIN_PAGE_HEIGHT", 200)  # no halves below this
LAYOUT_FORWARD_BATCH = _env_int("EZPDF_OCR_LAYOUT_BATCH", 4)  # batch=8 hits a slow kernel (6.0s vs 0.22s)

# --- box filter / merge ---
# The merge gets more aggressive as these go down, and every threshold below is a knob: the defaults
# are calibrated on a paper page whose duplicates sat at 0.39-0.54 coverage of the smaller box.
DEDUP_IOU = _env_float("EZPDF_OCR_DEDUP_IOU", 0.5)
DEDUP_CONTAINMENT = _env_float("EZPDF_OCR_DEDUP_CONTAINMENT", 0.35)
# A text box inside a table/formula is that table's own text (its markdown already carries it), so it
# is merged on a lower overlap than text-to-text.
DEDUP_CONTAINMENT_STRUCTURED = _env_float("EZPDF_OCR_DEDUP_CONTAINMENT_STRUCTURED", 0.25)
# Pull every box tight around the ink inside it before merging. The detector's boxes carry a margin
# (median 5-9 px at scale 2.0, and up to 190 px on a spurious one) which is what makes two unrelated
# blocks overlap and their covers repaint each other. The margin left around the ink is the detector's
# own: a tighter box looks like a mis-detection and leaves the cover no room for the translated line
# to breathe. A box whose ink is under BOX_TRIM_MIN_AREA_RATIO of it keeps its rectangle: the reading
# is not trustworthy there, and shrinking onto a stray speck would cut real content (light text the
# ink mask cannot see).
BOX_TRIM_TO_INK = _env_bool("EZPDF_OCR_BOX_TRIM", True)
BOX_TRIM_MARGIN_PX = _env_float("EZPDF_OCR_BOX_TRIM_MARGIN", 6.0)
BOX_TRIM_MIN_AREA_RATIO = _env_float("EZPDF_OCR_BOX_TRIM_MIN_AREA_RATIO", 0.02)
BOX_MIN_SCORE = _env_float("EZPDF_OCR_BOX_MIN_SCORE", 0.35)
BOX_MIN_AREA = _env_float("EZPDF_OCR_BOX_MIN_AREA", 16 * 16)
BOX_EXPAND_PIXELS = _env_float("EZPDF_OCR_BOX_EXPAND_PIXELS", 2.0)
MAX_REGIONS = _env_int("EZPDF_OCR_MAX_REGIONS", 100)

# --- post-OCR text dedup (needs both an overlap and near-equal text) ---
DEDUP_TEXT_MIN_OVERLAP = _env_float("EZPDF_OCR_DEDUP_TEXT_MIN_OVERLAP", 0.15)
DEDUP_TEXT_RATIO = _env_float("EZPDF_OCR_DEDUP_TEXT_RATIO", 0.82)
DEDUP_TEXT_TOKEN_CONTAINMENT = _env_float("EZPDF_OCR_DEDUP_TEXT_TOKEN_CONTAINMENT", 0.7)
# OCR truncates word tails far more often than it invents words, so a shared head of this many
# characters counts as a match ("connota"/"connotation"). 0 disables the fuzzy path.
DEDUP_TEXT_FUZZY_PREFIX = _env_int("EZPDF_OCR_DEDUP_TEXT_FUZZY_PREFIX", 4)
# A text box that reads as one or two characters is a mis-detection of a paragraph-sized region, not
# content (raise this to be stricter; two keeps figure labels like "R3").
DEDUP_MIN_CONTENT_CHARS = _env_int("EZPDF_OCR_DEDUP_MIN_CONTENT_CHARS", 2)
# The VL's own "I could not read anything here" marker; a box full of it is not a block.
DEDUP_PLACEHOLDERS = ("[unlabeled]", "[n/a]", "[no text]")
# Per-page share of the text characters that may be removed as duplicates before a WARNING is logged
# (each merged box is listed there too). Removing duplicates is normal — a page that repeats itself
# loses a few percent by design — so this is a "have a look" line, not an error.
DEDUP_TEXT_MAX_LOSS = _env_float("EZPDF_OCR_DEDUP_TEXT_MAX_LOSS", 0.05)

# --- text inside figures (opt-in per figure: both signals must agree) ---
# The text half of the gate: does the figure's own OCR read like a body of text?
FIGURE_TEXT_MIN_CHARS = _env_int("EZPDF_OCR_FIGURE_TEXT_MIN_CHARS", 150)
FIGURE_TEXT_MIN_LINES = _env_int("EZPDF_OCR_FIGURE_TEXT_MIN_LINES", 3)
FIGURE_TEXT_MIN_ALNUM_RATIO = _env_float("EZPDF_OCR_FIGURE_TEXT_MIN_ALNUM_RATIO", 0.6)
# The geometry half: a morphological scan for the figure's text lines (services/textlines.py).
# merge_px joins the words of one line without bridging a diagram's columns; the ink band is what
# separates a line of glyphs from a rule, an arrow or a light backdrop; gap/x-overlap join the two
# lines of one boxed label into a block.
FIGURE_LINE_MERGE_PX = _env_int("EZPDF_OCR_FIGURE_LINE_MERGE_PX", 18)
FIGURE_LINE_MIN_HEIGHT = _env_int("EZPDF_OCR_FIGURE_LINE_MIN_HEIGHT", 12)
FIGURE_LINE_MAX_HEIGHT = _env_int("EZPDF_OCR_FIGURE_LINE_MAX_HEIGHT", 110)
FIGURE_LINE_MIN_WIDTH = _env_int("EZPDF_OCR_FIGURE_LINE_MIN_WIDTH", 16)
FIGURE_LINE_MIN_INK = _env_float("EZPDF_OCR_FIGURE_LINE_MIN_INK", 0.12)
FIGURE_LINE_MAX_INK = _env_float("EZPDF_OCR_FIGURE_LINE_MAX_INK", 0.5)
FIGURE_LINE_GAP_RATIO = _env_float("EZPDF_OCR_FIGURE_LINE_GAP_RATIO", 0.6)
FIGURE_LINE_MIN_X_OVERLAP = _env_float("EZPDF_OCR_FIGURE_LINE_MIN_X_OVERLAP", 0.3)
# Acceptance: enough separate lines, or one block covering this much of the figure (a figure that is
# itself one text block). Below either, the figure stays an image block and the lines are discarded.
FIGURE_INNER_MIN_BOXES = _env_int("EZPDF_OCR_FIGURE_INNER_MIN_BOXES", 3)
FIGURE_INNER_MIN_COVERAGE = _env_float("EZPDF_OCR_FIGURE_INNER_MIN_COVERAGE", 0.25)
FIGURE_MIN_SIDE_PX = _env_int("EZPDF_OCR_FIGURE_MIN_SIDE", 100)  # smaller figures are never re-scanned

# --- VL (PaddleOCR-VL-1.6) ---
VL_MAX_NEW_TOKENS = _env_int("EZPDF_OCR_VL_MAX_NEW_TOKENS", 512)  # 256 truncates long paragraphs
VL_MIN_PIXELS = _env_int("EZPDF_OCR_VL_MIN_PIXELS", 112896)
VL_MAX_PIXELS = _env_int("EZPDF_OCR_VL_MAX_PIXELS", 1280 * 28 * 28)
VL_FORWARD_BATCH = _env_int("EZPDF_OCR_VL_BATCH", 4)
VL_REPETITION_PENALTY = _env_float("EZPDF_OCR_VL_REPETITION_PENALTY", 1.15)


def ocr_tuning() -> dict:
    """OCRPipeline keyword args; keys match the constructor's parameter names."""
    return {
        "score_threshold": LAYOUT_SCORE_THRESHOLD,
        "max_long_side": LAYOUT_MAX_LONG_SIDE,
        "fallback_threshold": FALLBACK_SCORE_THRESHOLD,
        "fallback_sharpen": FALLBACK_SHARPEN,
        "fallback_iou": FALLBACK_IOU,
        "fallback_containment": FALLBACK_CONTAINMENT,
        "tile_threshold": TILE_SCORE_THRESHOLD,
        "tile_overlap": TILE_OVERLAP,
        "tile_iou": TILE_IOU,
        "tile_containment": TILE_CONTAINMENT,
        "tile_min_page_height": TILE_MIN_PAGE_HEIGHT,
        "layout_forward_batch": LAYOUT_FORWARD_BATCH,
        "box_iou_threshold": DEDUP_IOU,
        "box_containment_threshold": DEDUP_CONTAINMENT,
        "box_structured_containment_threshold": DEDUP_CONTAINMENT_STRUCTURED,
        "box_trim_to_ink": BOX_TRIM_TO_INK,
        "box_trim_margin": BOX_TRIM_MARGIN_PX,
        "box_trim_min_area_ratio": BOX_TRIM_MIN_AREA_RATIO,
        "box_min_score": BOX_MIN_SCORE,
        "box_min_area": BOX_MIN_AREA,
        "box_expand_pixels": BOX_EXPAND_PIXELS,
        "max_regions": MAX_REGIONS,
        "dedup_text_min_overlap": DEDUP_TEXT_MIN_OVERLAP,
        "dedup_text_ratio": DEDUP_TEXT_RATIO,
        "dedup_text_token_containment": DEDUP_TEXT_TOKEN_CONTAINMENT,
        "dedup_text_fuzzy_prefix": DEDUP_TEXT_FUZZY_PREFIX,
        "dedup_min_content_chars": DEDUP_MIN_CONTENT_CHARS,
        "dedup_placeholders": DEDUP_PLACEHOLDERS,
        "dedup_text_max_loss": DEDUP_TEXT_MAX_LOSS,
        "figure_text_min_chars": FIGURE_TEXT_MIN_CHARS,
        "figure_text_min_lines": FIGURE_TEXT_MIN_LINES,
        "figure_text_min_alnum_ratio": FIGURE_TEXT_MIN_ALNUM_RATIO,
        "figure_line_merge_px": FIGURE_LINE_MERGE_PX,
        "figure_line_min_height": FIGURE_LINE_MIN_HEIGHT,
        "figure_line_max_height": FIGURE_LINE_MAX_HEIGHT,
        "figure_line_min_width": FIGURE_LINE_MIN_WIDTH,
        "figure_line_min_ink": FIGURE_LINE_MIN_INK,
        "figure_line_max_ink": FIGURE_LINE_MAX_INK,
        "figure_line_gap_ratio": FIGURE_LINE_GAP_RATIO,
        "figure_line_min_x_overlap": FIGURE_LINE_MIN_X_OVERLAP,
        "figure_inner_min_boxes": FIGURE_INNER_MIN_BOXES,
        "figure_inner_min_coverage": FIGURE_INNER_MIN_COVERAGE,
        "figure_min_side_px": FIGURE_MIN_SIDE_PX,
        "max_new_tokens": VL_MAX_NEW_TOKENS,
        "vl_min_pixels": VL_MIN_PIXELS,
        "vl_max_pixels": VL_MAX_PIXELS,
        "vl_max_forward_batch": VL_FORWARD_BATCH,
        "vl_repetition_penalty": VL_REPETITION_PENALTY,
    }
